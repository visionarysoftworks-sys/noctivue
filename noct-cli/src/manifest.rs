//! Manifest + lockfile support (`nestpkg.nvpm`, `nestpkg.lock`).
//!
//! Implements stdlib/PROPOSALS.md P-003 §§1–5: the line-oriented,
//! indentation-significant grammar, the v1 schema, version requirements,
//! tier encoding with the foreign-runtime opt-in rules, and the
//! canonical lockfile form. Deliberately dependency-free (ADR-017: a
//! dedicated hand-written parser — the full compiler frontend is NOT
//! involved, so this works before/without a compilable project).
//!
//! Out of scope here: dependency RESOLUTION (Deferred, TOOLCHAIN.md
//! §3), registry I/O, and signature verification (P-003 §§6–8 —
//! shapes for hashes/key-ids are parsed and carried, not verified).

use std::collections::{BTreeMap, HashSet};

// ── Public schema types ─────────────────────────────────────────────────────

/// A parsed + validated package manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub package: Package,
    pub dependencies: Vec<Dependency>,
    pub dev_dependencies: Vec<Dependency>,
}

/// The `[package]` section (P-003 §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: SemVer,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: Option<String>,
    /// Opaque to v1 tools (recorded, echoed in errors).
    pub edition: Option<String>,
}

/// Strict `MAJOR.MINOR.PATCH`, numeric only, no pre-release (P-003 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

/// Version requirement (P-003 §3). Bare versions are caret requirements;
/// compound ranges are v2 (loud error, never a misparse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionReq {
    /// `1.2.3` means `>=1.2.3, <2.0.0` (full caret semantics).
    Caret(SemVer),
    /// `=1.2.3` pins exactly.
    Exact(SemVer),
}

/// Dependency trust tier (ADR-015, P-003 §4). Every entry declares one;
/// the scalar dependency form declares `Native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Native,
    CShim,
    CxxShim,
    WasmComponent,
    ForeignRuntime,
}

/// Dependency source (P-003 §4). Default is `Registry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Registry,
    /// A sibling tree (`path: ../foo`). Version still checked;
    /// hash/signature skipped — it is your own tree.
    Path(String),
    /// A VCS checkout. The lock pins the commit.
    Git {
        url: String,
        rev: Option<String>,
    },
}

/// One dependency entry (P-003 §2, §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub req: VersionReq,
    pub tier: Tier,
    /// Per-entry foreign-runtime opt-in. `true` is REQUIRED with
    /// `ForeignRuntime` and an ERROR with any other tier (a stray
    /// opt-in that silently stops meaning something is how audit
    /// tiers rot).
    pub opt_in: bool,
    pub source: Source,
}

/// A parsed lockfile (P-003 §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub lock_version: u64,
    pub packages: Vec<LockedPackage>,
}

/// One locked package. Sorted by name in canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: SemVer,
    pub source: Source,
    /// `sha256:<hex>` over the canonical package bytes. `None` only
    /// for `Path` sources (your tree — nothing to verify).
    pub content: Option<String>,
    pub tier: Tier,
    /// Publisher key-id that signed this version. Absent only for
    /// `Path` sources. Recorded now, verified at fetch (P-003 §7).
    pub signed_by: Option<String>,
    /// Set for `cxx-shim` entries so `audit` never re-derives it.
    pub experimental: bool,
}

/// A manifest/lockfile parse or validation failure. `line` is 1-based;
/// 0 means end-of-input (e.g. a block that never closed properly is
/// still attributable — every error names a line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ManifestError {}

// ── Entry points ────────────────────────────────────────────────────────────

/// Parse + validate a `nestpkg.nvpm` manifest.
pub fn parse_manifest(text: &str) -> Result<Manifest, ManifestError> {
    let tree = parse_tree(text)?;
    manifest_of(&tree)
}

/// Parse + validate a `nestpkg.lock` lockfile.
pub fn parse_lockfile(text: &str) -> Result<Lockfile, ManifestError> {
    let tree = parse_tree(text)?;
    lockfile_of(&tree)
}

/// Serialize a lockfile in canonical form: sections and keys sorted
/// byte-wise, exact versions, LF endings (P-003 §5). Byte-reproducible
/// from the same resolution — `serialize(parse(x)) == x` for canonical
/// `x` (covered by round-trip tests).
pub fn serialize_lockfile(lock: &Lockfile) -> String {
    let mut out = String::new();
    out.push_str("lock_version: ");
    out.push_str(&lock.lock_version.to_string());
    out.push('\n');
    out.push_str("packages:\n");
    let mut pkgs: Vec<&LockedPackage> = lock.packages.iter().collect();
    pkgs.sort_by(|a, b| a.name.cmp(&b.name));
    for p in pkgs {
        out.push_str("    ");
        out.push_str(&p.name);
        out.push_str(":\n");
        out.push_str("        version: ");
        out.push_str(&p.version.to_string());
        out.push('\n');
        match &p.source {
            Source::Registry => out.push_str("        source: registry\n"),
            Source::Path(path) => {
                out.push_str("        source:\n");
                out.push_str("            path: ");
                out.push_str(&quote_if_needed(path));
                out.push('\n');
            }
            Source::Git { url, rev } => {
                out.push_str("        source:\n");
                out.push_str("            git: ");
                out.push_str(&quote_if_needed(url));
                out.push('\n');
                if let Some(rev) = rev {
                    out.push_str("            rev: ");
                    out.push_str(&quote_if_needed(rev));
                    out.push('\n');
                }
            }
        }
        if let Some(content) = &p.content {
            out.push_str("        content: ");
            out.push_str(content);
            out.push('\n');
        }
        out.push_str("        tier: ");
        out.push_str(p.tier.as_str());
        out.push('\n');
        if let Some(key) = &p.signed_by {
            out.push_str("        signed_by: ");
            out.push_str(&quote_if_needed(key));
            out.push('\n');
        }
        if p.experimental {
            out.push_str("        experimental: true\n");
        }
    }
    out
}

/// Does `version` satisfy `req`? Full caret semantics (`^0.2.3` means
/// `>=0.2.3, <0.3.0`; `^0.0.3` means exactly `0.0.3`).
pub fn req_satisfied(req: VersionReq, version: SemVer) -> bool {
    match req {
        VersionReq::Exact(pinned) => version == pinned,
        VersionReq::Caret(base) => {
            if version < base {
                return false;
            }
            if base.major > 0 {
                version.major == base.major
            } else if base.minor > 0 {
                version.major == 0 && version.minor == base.minor
            } else {
                version == base
            }
        }
    }
}

// ── Tier helpers ────────────────────────────────────────────────────────────

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Native => "native",
            Tier::CShim => "c-shim",
            Tier::CxxShim => "cxx-shim",
            Tier::WasmComponent => "wasm-component",
            Tier::ForeignRuntime => "foreign-runtime",
        }
    }

    pub fn parse(s: &str, line: usize) -> Result<Tier, ManifestError> {
        match s {
            "native" => Ok(Tier::Native),
            "c-shim" => Ok(Tier::CShim),
            "cxx-shim" => Ok(Tier::CxxShim),
            "wasm-component" => Ok(Tier::WasmComponent),
            "foreign-runtime" => Ok(Tier::ForeignRuntime),
            other => Err(ManifestError {
                line,
                message: format!(
                    "unknown tier `{other}` (expected native, c-shim, \
                     cxx-shim, wasm-component, or foreign-runtime)"
                ),
            }),
        }
    }
}

impl SemVer {
    pub fn parse(s: &str, line: usize) -> Result<SemVer, ManifestError> {
        let parts: Vec<&str> = s.split('.').collect();
        let bad = || ManifestError {
            line,
            message: format!("bad version `{s}` (expected strict MAJOR.MINOR.PATCH, digits only)"),
        };
        if parts.len() != 3 {
            return Err(bad());
        }
        let mut nums = [0u64; 3];
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            // Strict SemVer: no leading zeros.
            if part.len() > 1 && part.starts_with('0') {
                return Err(bad());
            }
            nums[i] = part.parse().map_err(|_| bad())?;
        }
        Ok(SemVer {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
        })
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl VersionReq {
    pub fn parse(s: &str, line: usize) -> Result<VersionReq, ManifestError> {
        if let Some(pinned) = s.strip_prefix('=') {
            return Ok(VersionReq::Exact(SemVer::parse(pinned, line)?));
        }
        // Anything that is not a bare version is either garbage or a
        // v2 range — both are loud errors, never misparses (P-003 §3).
        if s.contains([' ', ',', '<', '>', '*', '^', '~', '|', '&']) || s.is_empty() {
            return Err(ManifestError {
                line,
                message: format!(
                    "bad version requirement `{s}` (v1 supports `1.2.3` \
                     for caret and `=1.2.3` for exact; compound ranges \
                     are v2)"
                ),
            });
        }
        Ok(VersionReq::Caret(SemVer::parse(s, line)?))
    }
}

// ── Generic tree parser (shared by manifest + lockfile) ────────────────────

/// A parsed block structure, before schema validation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Scalar(String),
    Seq(Vec<(String, usize)>),
    Map(BTreeMap<String, (Node, usize)>),
}

/// One physical line with its indent level and 1-based number.
struct PhysLine<'a> {
    indent: usize,
    text: &'a str,
    line: usize,
}

/// Split text into physical lines. Tabs in leading whitespace are a
/// loud error (P-003 §1 — never a guess). Returns lines with `\r`
/// stripped (manifest files are LF-canonical, P-004 D2).
fn physical_lines(text: &str) -> Result<Vec<PhysLine<'_>>, ManifestError> {
    let mut out = Vec::new();
    for (idx, raw) in text.split('\n').enumerate() {
        let line_no = idx + 1;
        let stripped = raw.strip_suffix('\r').unwrap_or(raw);
        let indent = stripped.len() - stripped.trim_start_matches(' ').len();
        let rest = &stripped[indent..];
        if rest.starts_with('\t') || stripped[..indent].contains('\t') {
            return Err(ManifestError {
                line: line_no,
                message: "tabs are not allowed (use spaces)".to_string(),
            });
        }
        // Blank lines (and pure-whitespace lines) carry no structure.
        if rest.trim().is_empty() {
            continue;
        }
        out.push(PhysLine {
            indent,
            text: rest,
            line: line_no,
        });
    }
    Ok(out)
}

/// Parse physical lines into a top-level map. Duplicate keys are
/// errors; dedent must land exactly on an enclosing level (Python
/// rule — a dedent to a level that matches nothing is an error, the
/// same discipline as LANGUAGE_SPEC.md §2).
fn parse_tree(text: &str) -> Result<BTreeMap<String, (Node, usize)>, ManifestError> {
    let lines = physical_lines(text)?;
    let (map, next) = parse_block(&lines, 0, 0)?;
    if next != lines.len() {
        // Unreachable if parse_block is correct (it consumes to a
        // dedent or EOF, and top level has nothing to dedent to) —
        // kept as a loud invariant rather than an assumption.
        return Err(ManifestError {
            line: lines[next].line,
            message: "unexpected content after top-level block".to_string(),
        });
    }
    Ok(map)
}

/// Parse a block starting at `pos` whose entries sit at `indent`.
/// Returns the map plus the first unconsumed position.
fn parse_block(
    lines: &[PhysLine<'_>],
    mut pos: usize,
    indent: usize,
) -> Result<(BTreeMap<String, (Node, usize)>, usize), ManifestError> {
    let mut map: BTreeMap<String, (Node, usize)> = BTreeMap::new();
    while pos < lines.len() {
        let current = &lines[pos];
        if current.indent < indent {
            break;
        }
        if current.indent > indent {
            return Err(ManifestError {
                line: current.line,
                message: format!(
                    "unexpected indent (expected {indent} spaces, found {})",
                    current.indent
                ),
            });
        }
        // Sequence items never appear directly in a map block.
        if let Some(item) = current.text.strip_prefix("- ") {
            return Err(ManifestError {
                line: current.line,
                message: format!("unexpected sequence item `{item}` (expected `key:` entry)"),
            });
        }
        if current.text.starts_with('-') {
            return Err(ManifestError {
                line: current.line,
                message: "malformed sequence item (expected `- value`)".to_string(),
            });
        }
        let (key, value) = split_entry(current)?;
        if map.contains_key(key) {
            return Err(ManifestError {
                line: current.line,
                message: format!("duplicate key `{key}`"),
            });
        }
        pos += 1;
        if value.is_empty() {
            // Nested block or sequence: the next line decides (it must
            // be MORE indented — a missing body is an error, not an
            // empty map, so `deps:` followed by nothing fails loudly).
            let Some(child) = lines.get(pos) else {
                return Err(ManifestError {
                    line: current.line,
                    message: format!("`{key}:` has no body (expected indented block)"),
                });
            };
            if child.indent <= indent {
                return Err(ManifestError {
                    line: current.line,
                    message: format!("`{key}:` has no body (expected indented block)"),
                });
            }
            if child.text.starts_with("- ") || child.text == "-" {
                let (seq, next) = parse_sequence(lines, pos, child.indent)?;
                map.insert(key.to_string(), (Node::Seq(seq), current.line));
                pos = next;
            } else {
                let (child_map, next) = parse_block(lines, pos, child.indent)?;
                map.insert(key.to_string(), (Node::Map(child_map), current.line));
                pos = next;
            }
        } else {
            map.insert(
                key.to_string(),
                (
                    Node::Scalar(parse_scalar(value, current.line)?),
                    current.line,
                ),
            );
        }
    }
    Ok((map, pos))
}

/// Parse a `- ` sequence starting at `pos` (all items at `indent`).
fn parse_sequence(
    lines: &[PhysLine<'_>],
    mut pos: usize,
    indent: usize,
) -> Result<(Vec<(String, usize)>, usize), ManifestError> {
    let mut seq = Vec::new();
    while pos < lines.len() {
        let current = &lines[pos];
        if current.indent < indent {
            break;
        }
        if current.indent > indent {
            return Err(ManifestError {
                line: current.line,
                message: format!(
                    "unexpected indent in sequence (expected {indent} spaces, found {})",
                    current.indent
                ),
            });
        }
        let Some(item) = current.text.strip_prefix("- ") else {
            if current.text == "-" {
                return Err(ManifestError {
                    line: current.line,
                    message: "empty sequence item (expected `- value`)".to_string(),
                });
            }
            // A `key:` line ends the sequence (belongs to the parent).
            break;
        };
        seq.push((parse_scalar(item, current.line)?, current.line));
        pos += 1;
    }
    Ok((seq, pos))
}

/// Split `key: value` (value may be empty). A missing colon, an empty
/// key, or a `- ` line reaching here is a loud error.
fn split_entry<'a>(line: &PhysLine<'a>) -> Result<(&'a str, &'a str), ManifestError> {
    let Some(colon) = line.text.find(':') else {
        return Err(ManifestError {
            line: line.line,
            message: format!("expected `key: value`, found `{}`", line.text),
        });
    };
    let key = line.text[..colon].trim();
    let value = line.text[colon + 1..].trim();
    if key.is_empty() {
        return Err(ManifestError {
            line: line.line,
            message: "empty key (expected `key: value`)".to_string(),
        });
    }
    if key.contains([' ', '\t']) {
        return Err(ManifestError {
            line: line.line,
            message: format!("bad key `{key}` (keys are single words)"),
        });
    }
    Ok((key, value))
}

/// Parse one scalar: `"…"` quoted string (with `\"`, `\\`, `\n`
/// escapes), `true`/`false`, all-digit integer, or a bare word.
/// Returned uniformly as String — schema validation interprets types
/// (a mistyped value fails THERE with a named key, not here).
fn parse_scalar(s: &str, line: usize) -> Result<String, ManifestError> {
    if let Some(rest) = s.strip_prefix('"') {
        // Scan to the first UNESCAPED quote: that closes the string.
        // Anything non-blank after it is an error (`"a" "b"`), and no
        // closer at all is unterminated. Interior bare quotes can only
        // surface here — a closer followed by junk.
        let mut out = String::new();
        let mut chars = rest.chars();
        let mut closed = false;
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    closed = true;
                    break;
                }
                '\\' => match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some(other) => {
                        return Err(ManifestError {
                            line,
                            message: format!(
                                "bad escape `\\{other}` (expected \\\", \\\\, or \\n)"
                            ),
                        })
                    }
                    None => {
                        return Err(ManifestError {
                            line,
                            message: "dangling backslash in quoted string".to_string(),
                        })
                    }
                },
                c => out.push(c),
            }
        }
        if !closed {
            return Err(ManifestError {
                line,
                message: "unterminated quoted string".to_string(),
            });
        }
        let tail: String = chars.collect();
        if !tail.trim().is_empty() {
            return Err(ManifestError {
                line,
                message: "unexpected content after quoted string".to_string(),
            });
        }
        return Ok(out);
    }
    if s.contains('"') {
        return Err(ManifestError {
            line,
            message: "stray quote (quote the whole value to use quotes)".to_string(),
        });
    }
    Ok(s.to_string())
}

// ── Schema: manifest ────────────────────────────────────────────────────────

fn take_scalar(
    map: &BTreeMap<String, (Node, usize)>,
    key: &str,
    required: bool,
    context: &str,
) -> Result<Option<(String, usize)>, ManifestError> {
    match map.get(key) {
        None => {
            if required {
                Err(ManifestError {
                    line: 0,
                    message: format!("{context} is missing required key `{key}`"),
                })
            } else {
                Ok(None)
            }
        }
        Some((Node::Scalar(value), line)) => Ok(Some((value.clone(), *line))),
        Some((_, line)) => Err(ManifestError {
            line: *line,
            message: format!("{context} key `{key}` must be a scalar value"),
        }),
    }
}

fn take_map<'a>(
    map: &'a BTreeMap<String, (Node, usize)>,
    key: &str,
    required: bool,
    context: &str,
) -> Result<Option<(&'a BTreeMap<String, (Node, usize)>, usize)>, ManifestError> {
    match map.get(key) {
        None => {
            if required {
                Err(ManifestError {
                    line: 0,
                    message: format!("{context} is missing required key `{key}`"),
                })
            } else {
                Ok(None)
            }
        }
        Some((Node::Map(child), line)) => Ok(Some((child, *line))),
        Some((_, line)) => Err(ManifestError {
            line: *line,
            message: format!("{context} key `{key}` must be a block"),
        }),
    }
}

/// Unknown keys are loud errors (typo protection); `x-`-prefixed keys
/// are the forward-compat escape hatch and ignored (P-003 §1).
fn reject_unknown(
    map: &BTreeMap<String, (Node, usize)>,
    known: &[&str],
    context: &str,
) -> Result<(), ManifestError> {
    for (key, (_, line)) in map {
        if !known.contains(&key.as_str()) && !key.starts_with("x-") {
            return Err(ManifestError {
                line: *line,
                message: format!("unknown key `{key}` in {context}"),
            });
        }
    }
    Ok(())
}

fn valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Validate a project/package name against the authoritative rules
/// (create spec §4: `create` consumes these, never redefines them).
/// Covers both shape AND the reserved prefixes (`std*`, `noct*`,
/// `core`) so callers get one verdict.
pub fn validate_project_name(name: &str) -> Result<(), String> {
    if !valid_package_name(name) {
        return Err(format!(
            "bad project name `{name}` (lowercase snake_case, start with a letter, max 64 chars)"
        ));
    }
    if name.starts_with("std") || name.starts_with("noct") || name == "core" {
        return Err(format!(
            "project name `{name}` is reserved (`std*`, `noct*`, `core`)"
        ));
    }
    Ok(())
}

fn manifest_of(tree: &BTreeMap<String, (Node, usize)>) -> Result<Manifest, ManifestError> {
    reject_unknown(
        tree,
        &["package", "dependencies", "dev_dependencies"],
        "manifest",
    )?;
    let (package_map, _) = take_map(tree, "package", true, "manifest")?.ok_or(ManifestError {
        line: 0,
        message: "manifest is missing required section `package:`".to_string(),
    })?;
    reject_unknown(
        package_map,
        &[
            "name",
            "version",
            "description",
            "authors",
            "license",
            "edition",
        ],
        "package:",
    )?;

    let (name, name_line) =
        take_scalar(package_map, "name", true, "package:")?.ok_or(ManifestError {
            line: 0,
            message: "package: is missing required key `name`".to_string(),
        })?;
    if let Err(message) = validate_project_name(&name) {
        return Err(ManifestError {
            line: name_line,
            message,
        });
    }
    let (version_s, version_line) =
        take_scalar(package_map, "version", true, "package:")?.ok_or(ManifestError {
            line: 0,
            message: "package: is missing required key `version`".to_string(),
        })?;
    let version = SemVer::parse(&version_s, version_line)?;
    let description = take_scalar(package_map, "description", false, "package:")?.map(|(v, _)| v);
    let license = take_scalar(package_map, "license", false, "package:")?.map(|(v, _)| v);
    let edition = take_scalar(package_map, "edition", false, "package:")?.map(|(v, _)| v);
    let mut authors = Vec::new();
    if let Some((Node::Seq(items), _)) = package_map.get("authors") {
        for (author, _) in items {
            authors.push(author.clone());
        }
    } else if package_map.contains_key("authors") {
        let line = package_map["authors"].1;
        return Err(ManifestError {
            line,
            message: "package: key `authors` must be a `- ` sequence".to_string(),
        });
    }

    let mut dependencies = Vec::new();
    if let Some((deps_map, _)) = take_map(tree, "dependencies", false, "manifest")? {
        for (dep_name, (node, line)) in deps_map {
            dependencies.push(dependency_of(dep_name, node, *line, "dependencies:")?);
        }
    }
    let mut dev_dependencies = Vec::new();
    if let Some((deps_map, _)) = take_map(tree, "dev_dependencies", false, "manifest")? {
        for (dep_name, (node, line)) in deps_map {
            dev_dependencies.push(dependency_of(dep_name, node, *line, "dev_dependencies:")?);
        }
    }
    // A dependency in both tables is an error (which table wins would
    // be a silent guess — fail loudly instead).
    let main_names: HashSet<&str> = dependencies.iter().map(|d| d.name.as_str()).collect();
    for dev in &dev_dependencies {
        if main_names.contains(dev.name.as_str()) {
            return Err(ManifestError {
                line: 0,
                message: format!(
                    "`{}` is in both dependencies: and dev_dependencies: \
                     (pick one table)",
                    dev.name
                ),
            });
        }
    }

    Ok(Manifest {
        package: Package {
            name,
            version,
            description,
            authors,
            license,
            edition,
        },
        dependencies,
        dev_dependencies,
    })
}

fn dependency_of(
    name: &str,
    node: &Node,
    line: usize,
    context: &str,
) -> Result<Dependency, ManifestError> {
    if !valid_package_name(name) {
        return Err(ManifestError {
            line,
            message: format!("bad dependency name `{name}` (lowercase snake_case)"),
        });
    }
    match node {
        // Scalar form: a version requirement, native tier, registry.
        Node::Scalar(req_s) => Ok(Dependency {
            name: name.to_string(),
            req: VersionReq::parse(req_s, line)?,
            tier: Tier::Native,
            opt_in: false,
            source: Source::Registry,
        }),
        Node::Map(map) => {
            reject_unknown(
                map,
                &["version", "tier", "opt_in", "source", "path", "git", "rev"],
                context,
            )?;
            let (req_s, req_line) =
                take_scalar(map, "version", true, context)?.ok_or(ManifestError {
                    line,
                    message: format!("dependency `{name}` is missing required key `version`"),
                })?;
            let req = VersionReq::parse(&req_s, req_line)?;
            let tier = match take_scalar(map, "tier", false, context)? {
                Some((tier_s, tier_line)) => Tier::parse(&tier_s, tier_line)?,
                None => Tier::Native,
            };
            let opt_in = match take_scalar(map, "opt_in", false, context)? {
                Some((flag, flag_line)) => match flag.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(ManifestError {
                            line: flag_line,
                            message: format!(
                                "dependency `{name}` key `opt_in` must be true or false"
                            ),
                        })
                    }
                },
                None => false,
            };
            // The opt-in rule (P-003 §4): REQUIRED with foreign-runtime,
            // ERROR with anything else.
            if tier == Tier::ForeignRuntime && !opt_in {
                return Err(ManifestError {
                    line,
                    message: format!(
                        "dependency `{name}` has tier foreign-runtime \
                         without `opt_in: true` (explicit opt-in required)"
                    ),
                });
            }
            if tier != Tier::ForeignRuntime && opt_in {
                return Err(ManifestError {
                    line,
                    message: format!(
                        "dependency `{name}` sets `opt_in: true` without \
                         tier foreign-runtime (stray opt-ins are errors)"
                    ),
                });
            }
            let source = source_of(map, name, line, context)?;
            Ok(Dependency {
                name: name.to_string(),
                req,
                tier,
                opt_in,
                source,
            })
        }
        Node::Seq(_) => Err(ManifestError {
            line,
            message: format!("dependency `{name}` must be a version or a block"),
        }),
    }
}

fn source_of(
    map: &BTreeMap<String, (Node, usize)>,
    dep_name: &str,
    line: usize,
    context: &str,
) -> Result<Source, ManifestError> {
    // Long form: `source:` block with exactly one of path/git.
    if let Some((Node::Map(src_map), src_line)) = map.get("source") {
        reject_unknown(src_map, &["path", "git", "rev"], context)?;
        let has_path = src_map.contains_key("path");
        let has_git = src_map.contains_key("git");
        if has_path == has_git {
            return Err(ManifestError {
                line: *src_line,
                message: format!(
                    "dependency `{dep_name}` source: needs exactly one of \
                     `path:` or `git:`"
                ),
            });
        }
        if has_path {
            let (path, _) = take_scalar(src_map, "path", true, context)?.ok_or(ManifestError {
                line: *src_line,
                message: format!("dependency `{dep_name}` source: missing `path:`"),
            })?;
            if take_scalar(src_map, "rev", false, context)?.is_some() {
                return Err(ManifestError {
                    line: *src_line,
                    message: format!(
                        "dependency `{dep_name}` source: `rev:` needs `git:`, not `path:`"
                    ),
                });
            }
            return Ok(Source::Path(path));
        }
        let (url, _) = take_scalar(src_map, "git", true, context)?.ok_or(ManifestError {
            line: *src_line,
            message: format!("dependency `{dep_name}` source: missing `git:`"),
        })?;
        let rev = take_scalar(src_map, "rev", false, context)?.map(|(v, _)| v);
        return Ok(Source::Git { url, rev });
    } else if map.contains_key("source") {
        // Scalar form: only the default is spellable (`source:
        // registry` — this is what the lockfile serializer emits,
        // P-003 §5). Anything else must use the block form.
        let src_line = map["source"].1;
        match &map["source"].0 {
            Node::Scalar(value) if value == "registry" => return Ok(Source::Registry),
            _ => {
                return Err(ManifestError {
                    line: src_line,
                    message: format!(
                        "dependency `{dep_name}` key `source` must be a block \
                         (or exactly `source: registry`)"
                    ),
                })
            }
        }
    }
    // Short forms: top-level `path:` / `git:` inside the dep block.
    let has_path = map.contains_key("path");
    let has_git = map.contains_key("git");
    if has_path && has_git {
        return Err(ManifestError {
            line,
            message: format!("dependency `{dep_name}` has both `path:` and `git:` (pick one)"),
        });
    }
    if has_path {
        let (path, _) = take_scalar(map, "path", true, context)?.ok_or(ManifestError {
            line,
            message: format!("dependency `{dep_name}` is missing `path:` value"),
        })?;
        return Ok(Source::Path(path));
    }
    if has_git {
        let (url, _) = take_scalar(map, "git", true, context)?.ok_or(ManifestError {
            line,
            message: format!("dependency `{dep_name}` is missing `git:` value"),
        })?;
        let rev = take_scalar(map, "rev", false, context)?.map(|(v, _)| v);
        return Ok(Source::Git { url, rev });
    }
    if map.contains_key("rev") {
        return Err(ManifestError {
            line,
            message: format!("dependency `{dep_name}` key `rev:` needs `git:`"),
        });
    }
    Ok(Source::Registry)
}

// ── Schema: lockfile ────────────────────────────────────────────────────────

fn lockfile_of(tree: &BTreeMap<String, (Node, usize)>) -> Result<Lockfile, ManifestError> {
    reject_unknown(tree, &["lock_version", "packages"], "lockfile")?;
    let (version_s, _) =
        take_scalar(tree, "lock_version", true, "lockfile")?.ok_or(ManifestError {
            line: 0,
            message: "lockfile is missing required key `lock_version`".to_string(),
        })?;
    if version_s != "1" {
        return Err(ManifestError {
            line: 0,
            message: format!("unsupported lock_version `{version_s}` (this tool reads 1)"),
        });
    }
    let mut packages = Vec::new();
    if let Some((pkgs_map, _)) = take_map(tree, "packages", false, "lockfile")? {
        for (name, (node, line)) in pkgs_map {
            let Node::Map(map) = node else {
                return Err(ManifestError {
                    line: *line,
                    message: format!("locked package `{name}` must be a block"),
                });
            };
            reject_unknown(
                map,
                &[
                    "version",
                    "source",
                    "path",
                    "git",
                    "rev",
                    "content",
                    "tier",
                    "signed_by",
                    "experimental",
                ],
                "lockfile packages:",
            )?;
            if !valid_package_name(name) {
                return Err(ManifestError {
                    line: *line,
                    message: format!("bad package name `{name}` in lockfile"),
                });
            }
            let (version_s, version_line) =
                take_scalar(map, "version", true, "lockfile packages:")?.ok_or(ManifestError {
                    line: *line,
                    message: format!("locked package `{name}` is missing `version`"),
                })?;
            let version = SemVer::parse(&version_s, version_line)?;
            let source = source_of(map, name, *line, "lockfile packages:")?;
            let content = take_scalar(map, "content", false, "lockfile packages:")?.map(|(v, _)| v);
            if let Some(hash) = &content {
                if !valid_content_hash(hash) {
                    return Err(ManifestError {
                        line: *line,
                        message: format!(
                            "locked package `{name}` has bad content hash \
                             (expected `sha256:<64 hex chars>`)"
                        ),
                    });
                }
            }
            if matches!(source, Source::Registry | Source::Git { .. }) && content.is_none() {
                return Err(ManifestError {
                    line: *line,
                    message: format!(
                        "locked package `{name}` needs `content:` \
                         (only path sources skip hashes)"
                    ),
                });
            }
            let tier = match take_scalar(map, "tier", false, "lockfile packages:")? {
                Some((tier_s, tier_line)) => Tier::parse(&tier_s, tier_line)?,
                None => Tier::Native,
            };
            let signed_by =
                take_scalar(map, "signed_by", false, "lockfile packages:")?.map(|(v, _)| v);
            if matches!(source, Source::Registry | Source::Git { .. }) && signed_by.is_none() {
                return Err(ManifestError {
                    line: *line,
                    message: format!(
                        "locked package `{name}` needs `signed_by:` \
                         (only path sources skip signatures)"
                    ),
                });
            }
            let experimental = match take_scalar(map, "experimental", false, "lockfile packages:")?
            {
                Some((flag, flag_line)) => match flag.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(ManifestError {
                            line: flag_line,
                            message: format!(
                                "locked package `{name}` key `experimental` \
                                     must be true or false"
                            ),
                        })
                    }
                },
                None => tier == Tier::CxxShim,
            };
            packages.push(LockedPackage {
                name: name.clone(),
                version,
                source,
                content,
                tier,
                signed_by,
                experimental,
            });
        }
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Lockfile {
        lock_version: 1,
        packages,
    })
}

/// `sha256:<64 lowercase hex>`. Unknown prefixes fail here (algorithm
/// agility without silent downgrade, P-003 §5).
fn valid_content_hash(hash: &str) -> bool {
    match hash.strip_prefix("sha256:") {
        Some(hex) => hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

/// Quote a scalar for lockfile output when it would not re-parse as
/// itself. Keys split on the FIRST colon and values are trimmed, so
/// mid-value spaces/colons (even `key:7ad1`) round-trip bare — quoting
/// is only needed for the empty string, leading/trailing whitespace
/// (trimmed on read), and `"` (the quote introducer).
fn quote_if_needed(s: &str) -> String {
    let needs = s.is_empty()
        || s.starts_with([' ', '\t'])
        || s.ends_with([' ', '\t'])
        || s.contains('"')
        || s.contains('\n');
    if !needs {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── Slice 2: canonical manifest output, textual edits, lock checks ──────────

/// Serialize a manifest in canonical form: fixed section/key order,
/// LF endings, exact quoting round-trip. Used for `create` templates
/// and tests — NOT for rewriting user manifests on `add` (that path
/// uses [`upsert_dependency_text`] to preserve the user's bytes).
pub fn serialize_manifest(manifest: &Manifest) -> String {
    let mut out = String::new();
    out.push_str("package:\n");
    out.push_str(&format!("    name: {}\n", manifest.package.name));
    out.push_str(&format!("    version: {}\n", manifest.package.version));
    if let Some(description) = &manifest.package.description {
        out.push_str(&format!(
            "    description: {}\n",
            quote_if_needed(description)
        ));
    }
    if !manifest.package.authors.is_empty() {
        out.push_str("    authors:\n");
        for author in &manifest.package.authors {
            out.push_str(&format!("        - {}\n", quote_if_needed(author)));
        }
    }
    if let Some(license) = &manifest.package.license {
        out.push_str(&format!("    license: {}\n", quote_if_needed(license)));
    }
    if let Some(edition) = &manifest.package.edition {
        out.push_str(&format!("    edition: {}\n", quote_if_needed(edition)));
    }
    write_dep_table(&mut out, "dependencies", &manifest.dependencies);
    write_dep_table(&mut out, "dev_dependencies", &manifest.dev_dependencies);
    out
}

/// Render one dependency table (sorted by name) into canonical text.
/// Entries that are exactly (caret req, native, registry) use the
/// scalar form; anything else uses the expanded block.
fn write_dep_table(out: &mut String, section: &str, deps: &[Dependency]) {
    if deps.is_empty() {
        return;
    }
    out.push_str(section);
    out.push_str(":\n");
    let mut sorted: Vec<&Dependency> = deps.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for dep in sorted {
        out.push_str(&render_dep_entry(dep));
    }
}

/// Render one dependency entry (with trailing newline), 4-space based.
fn render_dep_entry(dep: &Dependency) -> String {
    let simple = dep.tier == Tier::Native && !dep.opt_in && matches!(dep.source, Source::Registry);
    if simple {
        let req = match dep.req {
            VersionReq::Caret(v) => v.to_string(),
            VersionReq::Exact(v) => format!("={v}"),
        };
        return format!("    {}: {}\n", dep.name, req);
    }
    let mut out = format!("    {}:\n", dep.name);
    let req = match dep.req {
        VersionReq::Caret(v) => v.to_string(),
        VersionReq::Exact(v) => format!("={v}"),
    };
    out.push_str(&format!("        version: {req}\n"));
    if dep.tier != Tier::Native {
        out.push_str(&format!("        tier: {}\n", dep.tier.as_str()));
    }
    if dep.opt_in {
        out.push_str("        opt_in: true\n");
    }
    match &dep.source {
        Source::Registry => {}
        Source::Path(path) => {
            out.push_str(&format!("        path: {}\n", quote_if_needed(path)));
        }
        Source::Git { url, rev } => {
            out.push_str(&format!("        git: {}\n", quote_if_needed(url)));
            if let Some(rev) = rev {
                out.push_str(&format!("        rev: {}\n", quote_if_needed(rev)));
            }
        }
    }
    out
}

/// Insert or replace one dependency entry in manifest TEXT, preserving
/// every other byte (ordering, comments — well, v1 HAS no comments —
/// blank lines, quoting style). Rules:
/// - `section` is `dependencies` or `dev_dependencies` (anything else
///   is a caller bug → Err).
/// - If the section exists and holds `dep.name:`, that entry (scalar
///   line or full indented block) is replaced by the rendered entry.
/// - If the section exists without the entry, the entry is appended
///   at the section end (same 4-space indent as siblings, falling
///   back to 4 spaces for an empty section).
/// - If the section is missing, it is appended at end of file with
///   its entry (file keeps its existing trailing-newline state plus
///   exactly one entry block — the result always ends with `\n`).
/// Replacement uses the canonical [`render_dep_entry`] form even when
/// the old entry was scalar: an updated entry is new information and
/// new information is canonical (the rest of the file is untouched).
pub fn upsert_dependency_text(
    text: &str,
    section: &str,
    dep: &Dependency,
) -> Result<String, ManifestError> {
    if section != "dependencies" && section != "dev_dependencies" {
        return Err(ManifestError {
            line: 0,
            message: format!("upsert into unknown section `{section}`"),
        });
    }
    // Validate the round-trip first: garbage in must fail here, not
    // produce a half-edited file.
    let _ = parse_manifest(text)?;
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    // Drop the artifact empty split when text already ends with \n.
    let ends_newline = text.ends_with('\n');
    if ends_newline {
        lines.pop();
    }

    let section_at = lines.iter().position(|l| l.trim() == format!("{section}:"));
    let Some(sec_idx) = section_at else {
        // Missing section: append `section:\n<entry>`, keeping one
        // blank line of air if the file ends with content.
        let mut out = lines.join("\n");
        if !out.trim_end().is_empty() {
            out.push('\n');
        }
        out.push_str(section);
        out.push_str(":\n");
        out.push_str(&render_dep_entry(dep));
        return Ok(out);
    };

    let sec_indent = indent_of(&lines[sec_idx]);
    // Section body: lines after sec_idx while MORE indented (blank
    // lines belong to the body conservatively — they survive).
    let mut body_end = sec_idx + 1;
    while body_end < lines.len() {
        let l = &lines[body_end];
        if l.trim().is_empty() || indent_of(l) > sec_indent {
            body_end += 1;
        } else {
            break;
        }
    }
    // Find an existing entry `name:` at the body's base indent (the
    // minimum indent over non-blank body lines — honors 2-space and
    // 4-space files alike). Deeper lines are continuations.
    let mut base: Option<usize> = None;
    let mut entry_at: Option<(usize, usize)> = None;
    let mut i = sec_idx + 1;
    while i < body_end {
        let l = &lines[i];
        if l.trim().is_empty() {
            i += 1;
            continue;
        }
        let ind = indent_of(l);
        if base.is_none() {
            base = Some(ind);
        }
        if Some(ind) != base {
            i += 1;
            continue;
        }
        if let Some(colon) = l.find(':') {
            let key = l[..colon].trim();
            if key == dep.name {
                // Entry spans this line plus deeper-indented lines
                // (a trailing blank belongs to the section, not it).
                let mut end = i + 1;
                while end < body_end
                    && !lines[end].trim().is_empty()
                    && indent_of(&lines[end]) > ind
                {
                    end += 1;
                }
                entry_at = Some((i, end));
                break;
            }
        }
        i += 1;
    }

    let rendered = render_dep_entry(dep);
    // Re-indent the canonical entry (4-space based) to the section's
    // actual indent: canonical body indent is 4, section indent + 4 is
    // the target for entry lines, +8 for entry-body lines.
    let mut rendered_lines: Vec<String> = rendered
        .trim_end_matches('\n')
        .split('\n')
        .map(str::to_string)
        .collect();
    for (idx, rline) in rendered_lines.iter_mut().enumerate() {
        let canon_indent = if idx == 0 { 4 } else { 8 };
        let body = rline.trim_start_matches(' ');
        *rline = format!("{}{}", " ".repeat(sec_indent + canon_indent), body);
    }

    if let Some((start, end)) = entry_at {
        lines.splice(start..end, rendered_lines);
    } else {
        // Append at section end (before trailing blanks of the body).
        let mut at = body_end;
        while at > sec_idx + 1 && lines[at - 1].trim().is_empty() {
            at -= 1;
        }
        let insert_at = at;
        let room: Vec<String> = rendered_lines;
        for (k, rline) in room.into_iter().enumerate() {
            lines.insert(insert_at + k, rline);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    // The edited file must still parse AND validate (self-check: an
    // upsert that corrupts structure fails here, not in the user's
    // tree).
    let _ = parse_manifest(&out).map_err(|e| ManifestError {
        line: e.line,
        message: format!("upsert produced invalid manifest: {}", e.message),
    })?;
    Ok(out)
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Check a lockfile against its manifest (P-003 §6 consistency input).
/// Returns one human message per problem; empty means current. Rules:
/// - every manifest dependency (both tables) has a lock entry with a
///   SATISFYING version, the same tier, and the same source shape;
/// - every lock entry is still required by the manifest (stale
///   entries are drift, not harmless leftovers).
/// Hash/signature VALUES are not re-verified here (that happens at
/// fetch); their PRESENCE shape was validated at parse.
pub fn check_lock_current(manifest: &Manifest, lock: &Lockfile) -> Vec<String> {
    let mut problems = Vec::new();
    let locked: BTreeMap<&str, &LockedPackage> =
        lock.packages.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut required: HashSet<&str> = HashSet::new();
    for dep in manifest
        .dependencies
        .iter()
        .chain(manifest.dev_dependencies.iter())
    {
        required.insert(dep.name.as_str());
        match locked.get(dep.name.as_str()) {
            None => problems.push(format!(
                "`{}` is required by the manifest but missing from the lockfile",
                dep.name
            )),
            Some(entry) => {
                if !req_satisfied(dep.req, entry.version) {
                    problems.push(format!(
                        "`{}` lock version {} does not satisfy requirement `{}`",
                        dep.name,
                        entry.version,
                        req_string(dep.req)
                    ));
                }
                if entry.tier != dep.tier {
                    problems.push(format!(
                        "`{}` lock tier {} disagrees with manifest tier {}",
                        dep.name,
                        entry.tier.as_str(),
                        dep.tier.as_str()
                    ));
                }
                if !same_source_shape(&entry.source, &dep.source) {
                    problems.push(format!(
                        "`{}` lock source disagrees with the manifest source",
                        dep.name
                    ));
                }
            }
        }
    }
    let mut stale: Vec<&&str> = locked.keys().filter(|n| !required.contains(**n)).collect();
    stale.sort();
    for name in stale {
        problems.push(format!(
            "`{name}` is locked but no longer required (stale lock entry)"
        ));
    }
    problems
}

fn req_string(req: VersionReq) -> String {
    match req {
        VersionReq::Caret(v) => v.to_string(),
        VersionReq::Exact(v) => format!("={v}"),
    }
}

/// Same source KIND and, for path/git, same location (revs may differ
/// — a newer commit still satisfies the same `git:` requirement line;
/// the lock records what was actually fetched).
fn same_source_shape(a: &Source, b: &Source) -> bool {
    match (a, b) {
        (Source::Registry, Source::Registry) => true,
        (Source::Path(x), Source::Path(y)) => x == y,
        (Source::Git { url: x, .. }, Source::Git { url: y, .. }) => x == y,
        _ => false,
    }
}

// ── Tests (P-003 §9: goldens, negatives, round-trips — no network) ──────────

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_MANIFEST: &str = "\
package:
    name: myapp
    version: 0.1.0
    description: Short sentence.
    authors:
        - A U Thor
        - B R Ave
    license: MIT
    edition: 0.1
dependencies:
    http: 1.2.0
    legacy_c: =0.9.1
    fast_png:
        version: 2.0.0
        tier: c-shim
    py_model:
        version: 1.0.0
        tier: foreign-runtime
        opt_in: true
    sibling:
        version: 0.3.0
        path: ../sibling
    pinned_git:
        version: =1.1.0
        git: https://example.com/repo
        rev: abc123
dev_dependencies:
    testkit: 0.4.0
x-notes:
    anything: goes here
";

    #[test]
    fn golden_manifest_parses_to_expected_schema() {
        let m = parse_manifest(GOLDEN_MANIFEST).expect("golden manifest must parse");
        assert_eq!(m.package.name, "myapp");
        assert_eq!(
            m.package.version,
            SemVer {
                major: 0,
                minor: 1,
                patch: 0
            }
        );
        assert_eq!(m.package.description.as_deref(), Some("Short sentence."));
        assert_eq!(m.package.authors, vec!["A U Thor", "B R Ave"]);
        assert_eq!(m.package.license.as_deref(), Some("MIT"));
        assert_eq!(m.package.edition.as_deref(), Some("0.1"));
        assert_eq!(m.dependencies.len(), 6);
        // NOTE: entries iterate in sorted-name order (backed by a
        // BTreeMap), not file order — look everything up by name.
        let dep = |name: &str| {
            m.dependencies
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("missing dep `{name}`"))
        };

        assert_eq!(
            dep("http"),
            &Dependency {
                name: "http".to_string(),
                req: VersionReq::Caret(SemVer {
                    major: 1,
                    minor: 2,
                    patch: 0
                }),
                tier: Tier::Native,
                opt_in: false,
                source: Source::Registry,
            }
        );
        assert_eq!(
            dep("legacy_c").req,
            VersionReq::Exact(SemVer {
                major: 0,
                minor: 9,
                patch: 1
            })
        );
        assert_eq!(dep("fast_png").tier, Tier::CShim);
        let py = dep("py_model");
        assert_eq!(py.tier, Tier::ForeignRuntime);
        assert!(py.opt_in);
        assert_eq!(
            dep("sibling").source,
            Source::Path("../sibling".to_string())
        );
        assert_eq!(
            dep("pinned_git").source,
            Source::Git {
                url: "https://example.com/repo".to_string(),
                rev: Some("abc123".to_string()),
            }
        );
        assert_eq!(m.dev_dependencies.len(), 1);
        assert_eq!(m.dev_dependencies[0].name, "testkit");
    }

    #[test]
    fn quoted_strings_and_escapes() {
        let m = parse_manifest(
            "package:\n    name: esc\n    version: 1.0.0\n    description: \"a \\\"q\\\" b\\\\c\"\n",
        )
        .expect("escapes must parse");
        assert_eq!(m.package.description.as_deref(), Some("a \"q\" b\\c"));
    }

    #[test]
    fn project_name_rule() {
        // The single rule `create` consumes (create spec §4).
        for good in ["hello_world", "a", "x9_y"] {
            assert!(validate_project_name(good).is_ok(), "{good} should pass");
        }
        for bad in ["Hello", "9lives", "has-dash", "has space", ""] {
            assert!(validate_project_name(bad).is_err(), "{bad} should fail");
        }
        for reserved in ["stdlib2", "noctide", "core"] {
            let err = validate_project_name(reserved).expect_err("reserved must fail");
            assert!(err.contains("reserved"), "unexpected message: {err}");
        }
    }

    #[test]
    fn caret_semantics() {
        let v = |a, b, c| SemVer {
            major: a,
            minor: b,
            patch: c,
        };
        let caret = |a, b, c| VersionReq::Caret(v(a, b, c));
        assert!(req_satisfied(caret(1, 2, 3), v(1, 9, 0)));
        assert!(!req_satisfied(caret(1, 2, 3), v(2, 0, 0)));
        assert!(!req_satisfied(caret(1, 2, 3), v(1, 2, 2)));
        // ^0.2.3 locks minor; ^0.0.3 is exact.
        assert!(req_satisfied(caret(0, 2, 3), v(0, 2, 9)));
        assert!(!req_satisfied(caret(0, 2, 3), v(0, 3, 0)));
        assert!(req_satisfied(caret(0, 0, 3), v(0, 0, 3)));
        assert!(!req_satisfied(caret(0, 0, 3), v(0, 0, 4)));
        assert!(req_satisfied(VersionReq::Exact(v(1, 0, 0)), v(1, 0, 0)));
        assert!(!req_satisfied(VersionReq::Exact(v(1, 0, 0)), v(1, 0, 1)));
    }

    #[test]
    fn lockfile_round_trip_is_identity() {
        let lock = Lockfile {
            lock_version: 1,
            packages: vec![
                LockedPackage {
                    name: "http".to_string(),
                    version: SemVer {
                        major: 1,
                        minor: 2,
                        patch: 4,
                    },
                    source: Source::Registry,
                    content: Some(format!("sha256:{}", "ab".repeat(32))),
                    tier: Tier::Native,
                    signed_by: Some("key:7ad1".to_string()),
                    experimental: false,
                },
                LockedPackage {
                    name: "local".to_string(),
                    version: SemVer {
                        major: 0,
                        minor: 1,
                        patch: 0,
                    },
                    source: Source::Path("../local".to_string()),
                    content: None,
                    tier: Tier::Native,
                    signed_by: None,
                    experimental: false,
                },
            ],
        };
        let text = serialize_lockfile(&lock);
        let back = parse_lockfile(&text).expect("serialized lock must re-parse");
        assert_eq!(back, lock);
        // Canonical bytes, exactly (sorted keys, LF, pinned forms).
        let expected = "lock_version: 1\npackages:\n    http:\n        version: 1.2.4\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: native\n        signed_by: key:7ad1\n    local:\n        version: 0.1.0\n        source:\n            path: ../local\n        tier: native\n";
        assert_eq!(text, expected);
    }

    #[test]
    fn serialize_manifest_is_canonical_and_reparseable() {
        let m = parse_manifest(GOLDEN_MANIFEST).expect("golden must parse");
        let text = serialize_manifest(&m);
        let expected = "package:\n    name: myapp\n    version: 0.1.0\n    description: Short sentence.\n    authors:\n        - A U Thor\n        - B R Ave\n    license: MIT\n    edition: 0.1\ndependencies:\n    fast_png:\n        version: 2.0.0\n        tier: c-shim\n    http: 1.2.0\n    legacy_c: =0.9.1\n    pinned_git:\n        version: =1.1.0\n        git: https://example.com/repo\n        rev: abc123\n    py_model:\n        version: 1.0.0\n        tier: foreign-runtime\n        opt_in: true\n    sibling:\n        version: 0.3.0\n        path: ../sibling\ndev_dependencies:\n    testkit: 0.4.0\n";
        assert_eq!(text, expected);
        // Canonical output re-parses to an equal manifest.
        assert_eq!(parse_manifest(&text).expect("canonical must parse"), m);
    }

    fn sample_dep() -> Dependency {
        Dependency {
            name: "newbie".to_string(),
            req: VersionReq::Caret(SemVer {
                major: 1,
                minor: 0,
                patch: 0,
            }),
            tier: Tier::Native,
            opt_in: false,
            source: Source::Registry,
        }
    }

    #[test]
    fn upsert_appends_to_existing_section() {
        let before =
            "package:\n    name: app\n    version: 1.0.0\ndependencies:\n    http: 1.2.0\n";
        let after = upsert_dependency_text(before, "dependencies", &sample_dep())
            .expect("upsert must succeed");
        assert_eq!(
            after,
            "package:\n    name: app\n    version: 1.0.0\ndependencies:\n    http: 1.2.0\n    newbie: 1.0.0\n"
        );
    }

    #[test]
    fn upsert_creates_missing_section() {
        let before = "package:\n    name: app\n    version: 1.0.0\n";
        let after = upsert_dependency_text(before, "dev_dependencies", &sample_dep())
            .expect("upsert must succeed");
        assert_eq!(
            after,
            "package:\n    name: app\n    version: 1.0.0\ndev_dependencies:\n    newbie: 1.0.0\n"
        );
    }

    #[test]
    fn upsert_replaces_scalar_and_expanded_entries() {
        // Scalar entry replaced by an expanded one (tier added).
        let before = "package:\n    name: app\n    version: 1.0.0\ndependencies:\n    http: 1.2.0\n    tail: 2.0.0\n";
        let dep = Dependency {
            name: "http".to_string(),
            req: VersionReq::Caret(SemVer {
                major: 1,
                minor: 5,
                patch: 0,
            }),
            tier: Tier::CShim,
            opt_in: false,
            source: Source::Registry,
        };
        let after =
            upsert_dependency_text(before, "dependencies", &dep).expect("upsert must succeed");
        assert_eq!(
            after,
            "package:\n    name: app\n    version: 1.0.0\ndependencies:\n    http:\n        version: 1.5.0\n        tier: c-shim\n    tail: 2.0.0\n"
        );
        // Expanded entry replaced by a scalar one; unrelated lines
        // (blank lines, other sections) survive byte-identical.
        let before2 = "package:\n    name: app\n    version: 1.0.0\n\ndependencies:\n    http:\n        version: 1.5.0\n        tier: c-shim\n\ndev_dependencies:\n    testkit: 0.4.0\n";
        let dep2 = Dependency {
            name: "http".to_string(),
            req: VersionReq::Exact(SemVer {
                major: 1,
                minor: 5,
                patch: 1,
            }),
            tier: Tier::Native,
            opt_in: false,
            source: Source::Registry,
        };
        let after2 =
            upsert_dependency_text(before2, "dependencies", &dep2).expect("upsert must succeed");
        assert_eq!(
            after2,
            "package:\n    name: app\n    version: 1.0.0\n\ndependencies:\n    http: =1.5.1\n\ndev_dependencies:\n    testkit: 0.4.0\n"
        );
    }

    #[test]
    fn upsert_rejects_unknown_section_and_garbage() {
        let err = upsert_dependency_text("package:\n", "spells", &sample_dep())
            .expect_err("unknown section must fail");
        assert!(err.to_string().contains("unknown section"));
        let err = upsert_dependency_text("not a manifest\n", "dependencies", &sample_dep())
            .expect_err("garbage must fail before editing");
        assert!(err.to_string().contains("expected `key: value`"));
    }

    #[test]
    fn check_lock_current_reports_all_drift() {
        let m = parse_manifest(GOLDEN_MANIFEST).expect("golden must parse");
        // A lock matching the golden manifest exactly: versions chosen
        // to satisfy every requirement.
        let lock_text = "lock_version: 1\npackages:\n    fast_png:\n        version: 2.1.0\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: c-shim\n        signed_by: key:1\n    http:\n        version: 1.9.0\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: native\n        signed_by: key:1\n    legacy_c:\n        version: 0.9.1\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: native\n        signed_by: key:1\n    pinned_git:\n        version: 1.1.0\n        source:\n            git: https://example.com/repo\n            rev: abc123\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: native\n        signed_by: key:1\n    py_model:\n        version: 1.2.0\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: foreign-runtime\n        signed_by: key:2\n    sibling:\n        version: 0.3.5\n        source:\n            path: ../sibling\n        tier: native\n    testkit:\n        version: 0.4.2\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: native\n        signed_by: key:1\n";
        let lock = parse_lockfile(lock_text).expect("lock must parse");
        assert!(check_lock_current(&m, &lock).is_empty());

        // Drifted lock: wrong version, wrong tier, missing entry,
        // stale entry, changed path.
        let mut bad = lock.clone();
        bad.packages
            .iter_mut()
            .find(|p| p.name == "http")
            .expect("http locked")
            .version = SemVer {
            major: 2,
            minor: 0,
            patch: 0,
        };
        bad.packages
            .iter_mut()
            .find(|p| p.name == "fast_png")
            .expect("fast_png locked")
            .tier = Tier::Native;
        bad.packages.retain(|p| p.name != "testkit");
        bad.packages.push(LockedPackage {
            name: "ghost".to_string(),
            version: SemVer {
                major: 1,
                minor: 0,
                patch: 0,
            },
            source: Source::Registry,
            content: Some(format!("sha256:{}", "cd".repeat(32))),
            tier: Tier::Native,
            signed_by: Some("key:1".to_string()),
            experimental: false,
        });
        bad.packages
            .iter_mut()
            .find(|p| p.name == "sibling")
            .expect("sibling locked")
            .source = Source::Path("../elsewhere".to_string());
        let problems = check_lock_current(&m, &bad);
        let joined = problems.join("\n");
        for fragment in [
            "`http` lock version 2.0.0 does not satisfy",
            "`fast_png` lock tier native disagrees",
            "`testkit` is required by the manifest but missing",
            "`ghost` is locked but no longer required",
            "`sibling` lock source disagrees",
        ] {
            assert!(
                joined.contains(fragment),
                "missing `{fragment}` in:\n{joined}"
            );
        }
        assert_eq!(problems.len(), 5);
    }

    /// Each case must fail with a message containing the fragment.
    #[test]
    fn negative_corpus() {
        let manifest_cases: &[(&str, &str)] = &[
            ("", "missing required"),
            ("package:\n", "`package:` has no body"),
            ("package:\n    name: MyApp\n    version: 1.0.0\n", "bad project name"),
            ("package:\n    name: 9lives\n    version: 1.0.0\n", "bad project name"),
            ("package:\n    name: stdlib2\n    version: 1.0.0\n", "reserved"),
            ("package:\n    name: ok\n    version: 1.0\n", "bad version"),
            ("package:\n    name: ok\n    version: 1.0.x\n", "bad version"),
            ("package:\n    name: ok\n    version: 01.0.0\n", "bad version"),
            ("package:\n    name: ok\n    version: 1.0.0\n    frobnicate: 1\n", "unknown key"),
            ("package:\n    name: ok\n    version: 1.0.0\nsorcery:\n    x: 1\n", "unknown key"),
            (
                "package:\n    name: ok\n    version: 1.0.0\n    name: ok\n",
                "duplicate key",
            ),
            ("package:\n\tname: ok\n    version: 1.0.0\n", "tabs are not allowed"),
            (
                "package:\n    name: ok\n      version: 1.0.0\n",
                "unexpected indent",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\n  stray: 1\n",
                "unexpected indent",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    http: \">=1.0\"\n",
                "bad version requirement",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    py:\n        version: 1.0.0\n        tier: foreign-runtime\n",
                "without `opt_in: true`",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    py:\n        version: 1.0.0\n        tier: foreign-runtime\n        opt_in: false\n",
                "without `opt_in: true`",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    http:\n        version: 1.0.0\n        opt_in: true\n",
                "stray opt-in",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    http:\n        version: 1.0.0\n        tier: water\n",
                "unknown tier",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    http:\n        tier: native\n",
                "missing required key `version`",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    a:\n        version: 1.0.0\n        path: x\n        git: y\n",
                "both `path:` and `git:`",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    a:\n        version: 1.0.0\n        rev: abc\n",
                "needs `git:`",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\ndependencies:\n    dup: 1.0.0\ndev_dependencies:\n    dup: 1.0.0\n",
                "both dependencies:",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\n    description: \"abc\n",
                "unterminated quoted string",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\n    description: \"a\" \"b\"\n",
                "unexpected content after quoted string",
            ),
            (
                "package:\n    name: ok\n    version: 1.0.0\n    description: \"a\\qb\"\n",
                "bad escape",
            ),
            ("- lonely: 1\npackage:\n    name: ok\n    version: 1.0.0\n", "sequence item"),
            ("no-colon-here\n", "expected `key: value`"),
        ];
        for (i, (input, fragment)) in manifest_cases.iter().enumerate() {
            match parse_manifest(input) {
                Ok(_) => panic!("negative manifest case {i} parsed cleanly: {input:?}"),
                Err(e) => assert!(
                    e.to_string().contains(fragment),
                    "negative manifest case {i}: error `{e}` lacks `{fragment}`"
                ),
            }
        }

        let lock_cases: &[(&str, &str)] = &[
            ("packages:\n    a:\n        version: 1.0.0\n", "missing required key `lock_version`"),
            ("lock_version: 2\n", "unsupported lock_version"),
            (
                "lock_version: 1\npackages:\n    a:\n        version: 1.0.0\n        source: registry\n        tier: native\n",
                "needs `content:`",
            ),
            (
                "lock_version: 1\npackages:\n    a:\n        version: 1.0.0\n        source: registry\n        content: sha256:abababababababababababababababababababababababababababababababab\n        tier: native\n",
                "needs `signed_by:`",
            ),
            (
                "lock_version: 1\npackages:\n    a:\n        version: 1.0.0\n        source: registry\n        content: md5:abc\n        tier: native\n        signed_by: k\n",
                "bad content hash",
            ),
            (
                "lock_version: 1\npackages:\n    a:\n        version: 1.0.0\n        source: registry\n        content: sha256:zz\n        tier: native\n        signed_by: k\n",
                "bad content hash",
            ),
        ];
        for (i, (input, fragment)) in lock_cases.iter().enumerate() {
            match parse_lockfile(input) {
                Ok(_) => panic!("negative lock case {i} parsed cleanly: {input:?}"),
                Err(e) => assert!(
                    e.to_string().contains(fragment),
                    "negative lock case {i}: error `{e}` lacks `{fragment}`"
                ),
            }
        }
    }
}
