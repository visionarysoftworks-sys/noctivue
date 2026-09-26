//! nestpkg language servicing (`nestpkg.nvpm` manifests + `nestpkg.lock`
//! lockfiles) for the shared single-binary LSP server.
//!
//! Same binary and same VSIX as the `.nv` language: the server routes by
//! URI filename, the VS Code extension contributes a second language ID
//! (`nestpkg`) in the same package. No separate extension, no second
//! server process (TOOLCHAIN.md "One official X").
//!
//! Feature scope (Phase 1):
//! - diagnostics from the shared `nestpkg` parser (one parser for CLI+LSP)
//! - completion for sections, keys, and fixed value sets (tiers, bools)
//! - hover cards explaining keys, tiers, and version-requirement forms
//! - document symbols (sections + dependency entries)
//! - formatting: canonical `serialize_*` when the file parses, else a
//!   safe trailing-whitespace normalization (never corrupts invalid files)
//!
//! Deliberately OUT of scope here: manifest↔lock drift lenses (needs
//! workspace file I/O + `check_lock_current` wiring — CLI `build` owns
//! that today), rename-across-files, and registry-aware dependency-name
//! completion.

use lsp_types::*;

use nestpkg::{parse_lockfile, parse_manifest};

// ── URI routing ─────────────────────────────────────────────────────────────

/// Any `*.nvpm` file is a manifest. In practice this is `nestpkg.nvpm`,
/// plus registry-index `manifest.nvpm` files (same schema).
pub fn is_manifest_uri(uri: &Uri) -> bool {
    let s = uri.as_str();
    let path = s.split('?').next().unwrap_or(s);
    path.ends_with(".nvpm")
}

/// The lockfile is a fixed basename, not a generic extension: only
/// `nestpkg.lock` is claimed so `Cargo.lock` etc. are never hijacked.
pub fn is_lock_uri(uri: &Uri) -> bool {
    let s = uri.as_str();
    let path = s.split('?').next().unwrap_or(s);
    path.ends_with("nestpkg.lock")
}

pub fn is_nestpkg_uri(uri: &Uri) -> bool {
    is_manifest_uri(uri) || is_lock_uri(uri)
}

// ── Diagnostics ─────────────────────────────────────────────────────────────

fn range_for_error_line(text: &str, line_1based: usize) -> Range {
    if line_1based == 0 {
        return Range::new(Position::new(0, 0), Position::new(0, 0));
    }
    let idx = line_1based - 1;
    let len = text
        .lines()
        .nth(idx)
        .map(|l| l.chars().count() as u32)
        .unwrap_or(0);
    Range::new(
        Position::new(idx as u32, 0),
        Position::new(idx as u32, len),
    )
}

/// Parse diagnostics for whichever nestpkg file this URI names.
/// Returns an empty vec when the URI is not a nestpkg file.
pub fn diagnostics_for_uri(uri: &Uri, text: &str) -> Vec<Diagnostic> {
    if is_manifest_uri(uri) {
        manifest_diagnostics(text)
    } else if is_lock_uri(uri) {
        lock_diagnostics(text)
    } else {
        Vec::new()
    }
}

pub fn manifest_diagnostics(text: &str) -> Vec<Diagnostic> {
    match parse_manifest(text) {
        Ok(_) => Vec::new(),
        Err(e) => vec![Diagnostic {
            range: range_for_error_line(text, e.line),
            severity: Some(DiagnosticSeverity::ERROR),
            code: None,
            code_description: None,
            source: Some("nestpkg".to_string()),
            message: format!("invalid manifest: {e}"),
            related_information: None,
            tags: None,
            data: None,
        }],
    }
}

pub fn lock_diagnostics(text: &str) -> Vec<Diagnostic> {
    match parse_lockfile(text) {
        Ok(_) => Vec::new(),
        Err(e) => vec![Diagnostic {
            range: range_for_error_line(text, e.line),
            severity: Some(DiagnosticSeverity::ERROR),
            code: None,
            code_description: None,
            source: Some("nestpkg".to_string()),
            message: format!("invalid lockfile: {e}"),
            related_information: None,
            tags: None,
            data: None,
        }],
    }
}

// ── Completion ──────────────────────────────────────────────────────────────

fn line_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn enclosing_section(text: &str, line_idx: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = line_idx.min(lines.len().saturating_sub(1));
    loop {
        if let Some(line) = lines.get(i) {
            let trimmed = line.trim();
            if !trimmed.is_empty()
                && line_indent(line) == 0
                && trimmed.ends_with(':')
                && !trimmed.starts_with("- ")
            {
                return Some(trimmed.trim_end_matches(':').to_string());
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    None
}

fn completion_item(label: &str, detail: &str, insert: Option<&str>) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::PROPERTY),
        detail: Some(detail.to_string()),
        insert_text: Some(insert.unwrap_or(label).to_string()),
        ..Default::default()
    }
}

fn value_item(label: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::VALUE),
        detail: Some(detail.to_string()),
        ..Default::default()
    }
}

const TIERS: &[(&str, &str)] = &[
    ("native", "Pure Noctivue; default tier."),
    ("c-shim", "C interop shim."),
    ("cxx-shim", "C++ interop shim (experimental flag implied)."),
    ("wasm-component", "WASM component dependency."),
    ("foreign-runtime", "Foreign runtime — requires `opt_in: true`."),
];

/// Context-sensitive completions for manifests and lockfiles.
pub fn get_completions(uri: &Uri, text: &str, position: Position) -> Vec<CompletionItem> {
    let is_lock = is_lock_uri(uri);
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = position.line as usize;
    let current = lines.get(line_idx).copied().unwrap_or("");
    let col = position.character as usize;
    let prefix: String = current.chars().take(col).collect();
    let indent = line_indent(current);
    let section = enclosing_section(text, line_idx);

    // Value completions win when the cursor is past a `key:` colon.
    if let Some(colon) = prefix.find(':') {
        let key = prefix[..colon].trim();
        match key {
            "tier" => {
                return TIERS
                    .iter()
                    .map(|(t, d)| value_item(t, d))
                    .collect();
            }
            "opt_in" | "experimental" => {
                return vec![
                    value_item("true", "Enable."),
                    value_item("false", "Disable."),
                ];
            }
            "source" if !is_lock => {
                return vec![value_item("registry", "Default source.")];
            }
            "source" => {
                return vec![value_item("registry", "Default source.")];
            }
            _ => {}
        }
    }

    if is_lock {
        if indent == 0 {
            return vec![
                completion_item("lock_version:", "Lockfile schema version (1).", None),
                completion_item("packages:", "Locked package table.", None),
            ];
        }
        // Inside a locked-package block (indent >= 8) suggest field keys.
        if section.as_deref() == Some("packages") && indent >= 8 {
            return vec![
                completion_item("version:", "Pinned MAJOR.MINOR.PATCH.", None),
                completion_item("source:", "registry, or a path:/git: block.", None),
                completion_item("path:", "Path source location.", None),
                completion_item("git:", "Git source URL.", None),
                completion_item("rev:", "Pinned git revision.", None),
                completion_item("content:", "sha256:<64 hex> integrity hash.", None),
                completion_item("tier:", "Trust tier (native, c-shim, …).", None),
                completion_item("signed_by:", "Publisher key-id.", None),
                completion_item("experimental:", "true/false (cxx-shim default).", None),
            ];
        }
        return Vec::new();
    }

    // Manifest.
    if indent == 0 {
        return vec![
            completion_item("package:", "Required section: name + version.", None),
            completion_item(
                "dependencies:",
                "Runtime dependency table.",
                None,
            ),
            completion_item(
                "dev_dependencies:",
                "Dev-only dependency table.",
                None,
            ),
        ];
    }
    match section.as_deref() {
        Some("package") => vec![
            completion_item("name:", "Lowercase snake_case package name.", None),
            completion_item("version:", "Strict MAJOR.MINOR.PATCH.", None),
            completion_item("description:", "Short sentence.", None),
            completion_item("authors:", "A `- ` sequence of names.", None),
            completion_item("license:", "SPDX expression, e.g. MIT.", None),
            completion_item("edition:", "Opaque toolchain marker.", None),
        ],
        Some("dependencies") | Some("dev_dependencies") => {
            if indent >= 8 {
                vec![
                    completion_item(
                        "version:",
                        "Caret `1.2.3` or exact `=1.2.3`.",
                        None,
                    ),
                    completion_item("tier:", "Trust tier (default native).", None),
                    completion_item(
                        "opt_in:",
                        "Required true with foreign-runtime.",
                        None,
                    ),
                    completion_item("path:", "Sibling-tree source.", None),
                    completion_item("git:", "VCS source URL.", None),
                    completion_item("rev:", "Pinned VCS revision.", None),
                    completion_item("source:", "registry, or path:/git: block.", None),
                ]
            } else {
                // Dependency entry lines name packages; the fixed keys
                // cannot be completed there (names are user-defined).
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

// ── Hover ───────────────────────────────────────────────────────────────────

fn hover_card(signature: &str, body: &str) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```nestpkg\n{signature}\n```\n\n{body}"),
        }),
        range: None,
    }
}

fn lookup_hover(word: &str, is_lock: bool) -> Option<Hover> {
    let card = match word {
        "package:" | "package" => hover_card(
            "package:",
            "Required section. Holds `name:` + `version:` (plus optional `description:`, `authors:`, `license:`, `edition:`).",
        ),
        "dependencies:" | "dependencies" => hover_card(
            "dependencies:",
            "Runtime dependency table. Scalar form `name: 1.2.3` is caret + native + registry; expanded blocks add `tier:`, `opt_in:`, `path:`/`git:`.",
        ),
        "dev_dependencies:" | "dev_dependencies" => hover_card(
            "dev_dependencies:",
            "Dev-only dependency table (same entry shapes as `dependencies:`). A name in both tables is an error.",
        ),
        "name" => hover_card(
            "name: my_package",
            "Lowercase snake_case, max 64 chars. `std*`, `noct*`, `core` are reserved.",
        ),
        "version" => {
            if is_lock {
                hover_card("version: 1.2.4", "Exact pinned version (strict MAJOR.MINOR.PATCH).")
            } else {
                hover_card(
                    "version: 1.2.3  |  =1.2.3",
                    "Bare `1.2.3` is a caret requirement (`>=1.2.3, <2.0.0`; `0.x` rules per P-003 §3). `=1.2.3` pins exactly. Compound ranges are v2 (loud error).",
                )
            }
        }
        "description" => hover_card("description: Short sentence.", "Optional package description."),
        "authors" => hover_card("authors:\n    - A U Thor", "Optional `- ` sequence of author names."),
        "license" => hover_card("license: MIT", "Optional license expression."),
        "edition" => hover_card("edition: 0.1", "Opaque to v1 tools; recorded and echoed in errors."),
        "tier" => hover_card(
            "tier: native",
            "One of `native` (default), `c-shim`, `cxx-shim`, `wasm-component`, `foreign-runtime`. Required, never inferred — `noct audit` surfaces it.",
        ),
        "native" => hover_card("native", "Pure Noctivue dependency. Default tier for scalar entries."),
        "c-shim" => hover_card("c-shim", "C interop shim dependency."),
        "cxx-shim" => hover_card("cxx-shim", "C++ interop shim (`experimental` defaults true in lockfiles)."),
        "wasm-component" => hover_card("wasm-component", "WASM component dependency."),
        "foreign-runtime" => hover_card(
            "foreign-runtime",
            "Foreign-runtime dependency: requires `opt_in: true`, excluded from default signing/audit trust, never pulled transitively without a top-level opt-in.",
        ),
        "opt_in" => hover_card(
            "opt_in: true",
            "Per-entry foreign-runtime opt-in. REQUIRED with `tier: foreign-runtime`, ERROR with any other tier.",
        ),
        "source" => hover_card(
            "source: registry",
            "Default source. Block form holds exactly one of `path:` or `git:` (+ optional `rev:` for git).",
        ),
        "path" => hover_card("path: ../sibling", "Sibling-tree source. Version still checked; hash/signature skipped."),
        "git" => hover_card("git: https://example.com/repo", "VCS source. The lock pins the fetched commit in `rev:`."),
        "rev" => hover_card("rev: abc123", "Pinned VCS revision (needs `git:`)."),
        "lock_version" => hover_card("lock_version: 1", "Lockfile schema version. This tool reads 1."),
        "packages:" | "packages" => hover_card("packages:", "Locked package table, sorted by name in canonical form."),
        "content" => hover_card(
            "content: sha256:<64 hex>",
            "Integrity hash over canonical package bytes. Absent only for `path:` sources.",
        ),
        "signed_by" => hover_card(
            "signed_by: key:7ad1",
            "Publisher key-id that signed this version. Absent only for `path:` sources.",
        ),
        "experimental" => hover_card(
            "experimental: true",
            "Set for `cxx-shim` entries so `audit` never re-derives it.",
        ),
        _ => return None,
    };
    Some(card)
}

/// Hover over the key/value under the cursor for manifests + lockfiles.
pub fn get_hover(text: &str, position: Position, is_lock: bool) -> Option<Hover> {
    let line = text.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    let chars: Vec<char> = line.chars().collect();
    if col > chars.len() {
        return None;
    }
    // Tokenize on whitespace; find the token under the cursor so
    // `tier: foreign-runtime` hovers `foreign-runtime`, not `tier:`.
    let mut start = 0usize;
    let mut token: Option<String> = None;
    for (i, chunk) in line.split_inclusive(char::is_whitespace).enumerate() {
        let _ = i;
        let end = start + chunk.chars().count();
        if col >= start && col < end.max(start + 1) {
            let t = chunk.trim();
            if !t.is_empty() {
                token = Some(t.trim_end_matches(':').to_string());
            }
            break;
        }
        start = end;
    }
    let word = token?;
    // Strip quotes for quoted scalars.
    let word = word.trim_matches('"');
    // Keys keep their colon form for section lookup; try both.
    if let Some(h) = lookup_hover(word, is_lock) {
        return Some(h);
    }
    lookup_hover(&format!("{word}:"), is_lock)
}

// ── Document symbols ────────────────────────────────────────────────────────

/// Sections + dependency entries. Line-scanned (like the `.nv` symbols)
/// so broken files still outline.
pub fn document_symbols(text: &str, is_lock: bool) -> Vec<DocumentSymbol> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut current_section: Option<(String, Range)> = None;
    let mut children: Vec<DocumentSymbol> = Vec::new();

    let flush = |section: &mut Option<(String, Range)>,
                 children: &mut Vec<DocumentSymbol>,
                 out: &mut Vec<DocumentSymbol>| {
        if let Some((name, range)) = section.take() {
            out.push(DocumentSymbol {
                name,
                detail: None,
                kind: SymbolKind::MODULE,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: if children.is_empty() {
                    None
                } else {
                    Some(std::mem::take(children))
                },
            });
        }
    };

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line_indent(line);
        if indent == 0 && trimmed.ends_with(':') && !trimmed.starts_with("- ") {
            flush(&mut current_section, &mut children, &mut out);
            let range = Range::new(
                Position::new(idx as u32, 0),
                Position::new(idx as u32, line.chars().count() as u32),
            );
            current_section =
                Some((trimmed.trim_end_matches(':').to_string(), range));
        } else if indent == 4 && !trimmed.starts_with("- ") && trimmed.contains(':') {
            let name = trimmed.split(':').next().unwrap_or("").trim();
            if name.is_empty() || name.contains(' ') {
                continue;
            }
            // `package:` scalar keys live at indent 4 too — only treat
            // dependency-table entries as children.
            let in_deps = matches!(
                current_section.as_ref().map(|(n, _)| n.as_str()),
                Some("dependencies") | Some("dev_dependencies") | Some("packages")
            );
            if !in_deps {
                continue;
            }
            let _ = is_lock;
            let range = Range::new(
                Position::new(idx as u32, indent as u32),
                Position::new(idx as u32, line.chars().count() as u32),
            );
            children.push(DocumentSymbol {
                name: name.to_string(),
                detail: None,
                kind: SymbolKind::PACKAGE,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: None,
            });
        }
    }
    flush(&mut current_section, &mut children, &mut out);
    out
}

// ── Formatting ──────────────────────────────────────────────────────────────

fn full_range(text: &str) -> Range {
    let mut line = 0u32;
    let mut character = 0u32;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else if ch != '\r' {
            character += 1;
        }
    }
    Range::new(Position::new(0, 0), Position::new(line, character))
}

fn safe_normalize(text: &str) -> String {
    let mut out: String = text
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// Canonical serialize when the file parses; safe whitespace
/// normalization otherwise (never corrupts an invalid file).
pub fn formatting(uri: &Uri, text: &str) -> Vec<TextEdit> {
    let canonical: Option<String> = if is_manifest_uri(uri) {
        nestpkg::parse_manifest(text)
            .ok()
            .map(|m| nestpkg::serialize_manifest(&m))
    } else if is_lock_uri(uri) {
        nestpkg::parse_lockfile(text)
            .ok()
            .map(|l| nestpkg::serialize_lockfile(&l))
    } else {
        None
    };
    let new_text = canonical.unwrap_or_else(|| safe_normalize(text));
    if new_text == text {
        return Vec::new();
    }
    vec![TextEdit {
        range: full_range(text),
        new_text,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Uri {
        path.parse().unwrap()
    }

    #[test]
    fn routes_by_filename() {
        assert!(is_manifest_uri(&uri("file:///proj/nestpkg.nvpm")));
        assert!(is_manifest_uri(&uri("file:///idx/manifest.nvpm")));
        assert!(!is_manifest_uri(&uri("file:///proj/nestpkg.lock")));
        assert!(is_lock_uri(&uri("file:///proj/nestpkg.lock")));
        assert!(!is_lock_uri(&uri("file:///proj/Cargo.lock")));
        assert!(!is_manifest_uri(&uri("file:///proj/main.nv")));
    }

    #[test]
    fn manifest_diagnostic_points_at_bad_line() {
        let diags = manifest_diagnostics("package:\n    name: MyApp\n    version: 1.0.0\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].range.start.line, 1);
        assert!(diags[0].message.contains("invalid manifest"));
    }

    #[test]
    fn lock_diagnostic_and_ok_case() {
        let bad = lock_diagnostics("packages:\n    a:\n        version: 1.0.0\n");
        assert_eq!(bad.len(), 1);
        assert!(bad[0].message.contains("invalid lockfile"));
        let good = lock_diagnostics("lock_version: 1\n");
        assert!(good.is_empty());
    }

    #[test]
    fn completions_cover_sections_package_keys_and_tiers() {
        let top = get_completions(
            &uri("file:///p/nestpkg.nvpm"),
            "pack",
            Position::new(0, 4),
        );
        assert!(top.iter().any(|c| c.label == "package:"));

        let pkg = get_completions(
            &uri("file:///p/nestpkg.nvpm"),
            "package:\n    ",
            Position::new(1, 4),
        );
        assert!(pkg.iter().any(|c| c.label == "version:"));

        let tier = get_completions(
            &uri("file:///p/nestpkg.nvpm"),
            "dependencies:\n    a:\n        version: 1.0.0\n        tier: ",
            Position::new(3, 14),
        );
        assert!(tier.iter().any(|c| c.label == "foreign-runtime"));
    }

    #[test]
    fn hover_explains_tier_and_version() {
        let text = "dependencies:\n    a:\n        version: 1.0.0\n        tier: foreign-runtime\n";
        let h = get_hover(text, Position::new(3, 16), false).expect("tier hover");
        let val = match h.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(val.contains("foreign-runtime"));
        let v = get_hover("package:\n    version: 1.0.0\n", Position::new(1, 8), false)
            .expect("version hover");
        let val = match v.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(val.contains("caret"));
    }

    #[test]
    fn symbols_outline_deps() {
        let text = "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    http: 1.2.0\n";
        let syms = document_symbols(text, false);
        assert_eq!(syms.len(), 2);
        let deps = syms.iter().find(|s| s.name == "dependencies").expect("deps");
        let kids = deps.children.as_ref().expect("children");
        assert!(kids.iter().any(|k| k.name == "http"));
    }

    #[test]
    fn formatting_is_canonical_when_valid_and_safe_when_not() {
        let unordered = "package:\n    version: 0.1.0\n    name: myapp\n";
        let edits = formatting(&uri("file:///p/nestpkg.nvpm"), unordered);
        assert_eq!(edits.len(), 1);
        assert!(edits[0].new_text.starts_with("package:\n    name: myapp"));

        let broken = "package:\n    name: MyApp\n    version: 1.0.0   \n";
        let edits = formatting(&uri("file:///p/nestpkg.nvpm"), broken);
        assert_eq!(edits.len(), 1);
        assert!(!edits[0].new_text.ends_with("   \n"));
    }
}
