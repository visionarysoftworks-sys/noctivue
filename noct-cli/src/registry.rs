//! Package registry: file-backed index, resolution, fetch, verification.
//!
//! Phase 4 closes P-003's deferred half (registry I/O, resolution,
//! signature verification) with explicit scope cuts, all recorded
//! here rather than silently assumed:
//!
//! - **No network anywhere in this module.** The `Registry` trait is
//!   the seam a future HTTP backend implements; the only backend is
//!   `FileIndex`, a directory tree (the fixture index P-003 §9
//!   demands, grown into the real local-registry story).
//! - **Index layout** (`<root>/<name>/<version>/`):
//!   `manifest.nvpm` (the package's own manifest, parsed by the real
//!   manifest parser — a version's dependencies are data, not a new
//!   format), `pkg.bin` (opaque tarball bytes), `pkg.hash`
//!   (`sha256:<hex>` over those bytes), `pkg.sig` (hex Ed25519
//!   signature), `pkg.key` (key-id string). `<root>/_keys/<key-id>.pub`
//!   holds the hex publisher public key (P-003 §7: the index serves
//!   tarball + signature + key directory).
//! - **Tarball framing v1** (fixture interchange, not a wire format):
//!   repeated `[u32LE path-len][path][u64LE content-len][content]`.
//!   Unpacks under `.noct/packages/<name>-<version>/`; raw bytes also
//!   cached at `.noct/cache/<name>-<version>.pkg` for hash re-checks.
//! - **Resolution**: highest satisfying version per requirement set
//!   (fixpoint over the closure — constraints only accumulate, so no
//!   backtracking false-conflicts), tier conflicts are errors,
//!   dependency cycles terminate by construction, and P-003 §4's
//!   foreign-runtime transitivity rule is enforced with the chain
//!   named (`myapp → warez → py_model`). Only `Registry` sources
//!   resolve here — `Path` deps are the caller's tree (see `cmd_add`),
//!   `Git` sources need network fetch and fail loud.
//! - **Signing** (P-003 §7): Ed25519, SHA-256, payload
//!   `noctivue-publish-v1:<name>@<version>:<content-hash>` (domain
//!   separation kills cross-protocol reuse). TOFU with a paper trail:
//!   unknown key-ids print and are stored under the key dir (`NOCT_KEYS`
//!   env override, else the per-OS config home); later fetches fail
//!   closed on key OR signature mismatch; `--rotate-key` re-prints and
//!   re-records explicitly. No `--insecure` flag exists on purpose.
//! - **Verification is mandatory on fetch** for registry sources;
//!   `path` sources are exempt (your tree, your responsibility).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::manifest::{self, LockedPackage, Lockfile, Manifest, SemVer, Source, Tier, VersionReq};

// ── Hashing + signing primitives ────────────────────────────────────────────

/// `sha256:<lowercase-hex>` over `bytes` (P-003 §5 content format).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Domain-separated signing payload (P-003 §7).
pub fn sign_payload(name: &str, version: SemVer, content_hash: &str) -> String {
    format!("noctivue-publish-v1:{name}@{version}:{content_hash}")
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return Err("odd-length hex".to_string());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&text[i..i + 2], 16)
                .map_err(|_| format!("bad hex at byte {i}"))
        })
        .collect()
}

/// Hex-encode (fixture/test tooling; the CLI path formats inline).
#[allow(dead_code)]
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Verify an Ed25519 signature. Fails closed on every malformed
/// input (bad hex, wrong lengths) — callers map the message into
/// the four mismatch modes (bad sig, wrong key, rotated key,
/// unknown algorithm; unknown hash prefixes die earlier in the
/// manifest parser, never here).
pub fn verify_signature(pubkey_hex: &str, payload: &str, sig_hex: &str) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let pub_bytes = decode_hex(pubkey_hex).map_err(|e| format!("bad publisher key hex: {e}"))?;
    if pub_bytes.len() != 32 {
        return Err(format!("bad publisher key length: {}", pub_bytes.len()));
    }
    let sig_bytes = decode_hex(sig_hex).map_err(|e| format!("bad signature hex: {e}"))?;
    if sig_bytes.len() != 64 {
        return Err(format!("bad signature length: {}", sig_bytes.len()));
    }
    let mut pub_arr = [0u8; 32];
    pub_arr.copy_from_slice(&pub_bytes);
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let key = VerifyingKey::from_bytes(&pub_arr).map_err(|e| format!("bad publisher key: {e}"))?;
    let sig = Signature::from_bytes(&sig_arr);
    key.verify(payload.as_bytes(), &sig)
        .map_err(|_| "signature mismatch".to_string())
}

/// Sign (publisher side; used by fixture tooling and tests — real
/// `publish` upload is refused until the registry exists).
#[allow(dead_code)]
pub fn sign_bytes(signing_key: &[u8; 32], payload: &str) -> [u8; 64] {
    use ed25519_dalek::{Signer, SigningKey};
    SigningKey::from_bytes(signing_key).sign(payload.as_bytes()).to_bytes()
}

#[allow(dead_code)]
pub fn pubkey_hex(signing_key: &[u8; 32]) -> String {
    use ed25519_dalek::SigningKey;
    encode_hex(&SigningKey::from_bytes(signing_key).verifying_key().to_bytes())
}

// ── Local key store (TOFU paper trail) ──────────────────────────────────────

/// Key directory: `NOCT_KEYS` override, else the per-OS config home
/// (P-003 §7). One file per key-id holding hex public keys.
pub fn key_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOCT_KEYS") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("noct").join("keys");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("noct")
                .join("keys");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("noct").join("keys")
}

fn load_key_in(dir: &Path, key_id: &str) -> Option<String> {
    fs::read_to_string(dir.join(key_id)).ok().map(|s| s.trim().to_string())
}

fn store_key_in(dir: &Path, key_id: &str, pubkey_hex: &str) -> Result<(), String> {
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create key dir: {e}"))?;
    fs::write(dir.join(key_id), format!("{pubkey_hex}\n"))
        .map_err(|e| format!("cannot store key: {e}"))
}

// ── Tarball framing v1 ──────────────────────────────────────────────────────

/// Pack `(path, bytes)` entries. Paths must be relative, `/`-separated,
/// and free of `..` (rejected loud — tarballs are untrusted input).
/// (Test/fixture tooling + future backends; the CLI path exercises it
/// through `fetch_locked`.)
#[allow(dead_code)]
pub fn pack(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (path, bytes) in entries {
        if path.is_empty() || path.starts_with('/') || path.contains("..") {
            return Err(format!("unsafe tarball path: `{path}`"));
        }
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

/// Unpack, enforcing the same path rules (a malicious tarball never
/// writes outside its destination — the destination join is checked
/// by the caller via these clean relative paths).
pub fn unpack(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if i + 4 > bytes.len() {
            return Err("truncated tarball (path length)".to_string());
        }
        let plen = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if i + plen > bytes.len() {
            return Err("truncated tarball (path)".to_string());
        }
        let path = std::str::from_utf8(&bytes[i..i + plen])
            .map_err(|_| "tarball path is not UTF-8".to_string())?
            .to_string();
        i += plen;
        if path.is_empty() || path.starts_with('/') || path.contains("..") {
            return Err(format!("unsafe tarball path: `{path}`"));
        }
        if i + 8 > bytes.len() {
            return Err("truncated tarball (content length)".to_string());
        }
        let clen = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap()) as usize;
        i += 8;
        if i + clen > bytes.len() {
            return Err("truncated tarball (content)".to_string());
        }
        out.push((path, bytes[i..i + clen].to_vec()));
        i += clen;
    }
    if out.is_empty() {
        return Err("empty tarball".to_string());
    }
    Ok(out)
}

// ── Registry abstraction + file index ───────────────────────────────────────

/// One published version: its manifest (dependencies included),
/// tarball bytes, expected hash, signature, and key-id.
pub struct IndexEntry {
    pub manifest: Manifest,
    pub tarball: Vec<u8>,
    pub content_hash: String,
    pub signature_hex: String,
    pub key_id: String,
    pub pubkey_hex: String,
}

/// Package source. File-backed index today; the trait is the seam a
/// future HTTP backend implements (P-003 keeps the HTTP API out of
/// scope — this trait must not grow HTTP-shaped methods silently).
pub trait Registry {
    fn versions(&self, name: &str) -> Result<Vec<SemVer>, String>;
    fn entry(&self, name: &str, version: SemVer) -> Result<IndexEntry, String>;
}

/// Directory-tree index: `<root>/<name>/<version>/` holding
/// `manifest.nvpm`, `pkg.bin`, `pkg.hash`, `pkg.sig`, `pkg.key`,
/// plus `<root>/_keys/<key-id>.pub`.
pub struct FileIndex {
    root: PathBuf,
}

impl FileIndex {
    pub fn new(root: PathBuf) -> Self {
        FileIndex { root }
    }

    fn version_dir(&self, name: &str, version: SemVer) -> PathBuf {
        self.root.join(name).join(version.to_string())
    }

    fn read_file(&self, path: &Path) -> Result<String, String> {
        fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .map_err(|_| format!("index: cannot read {}", path.display()))
    }
}

impl Registry for FileIndex {
    fn versions(&self, name: &str) -> Result<Vec<SemVer>, String> {
        let dir = self.root.join(name);
        let entries = fs::read_dir(&dir)
            .map_err(|_| format!("index: unknown package `{name}`"))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let v = entry.file_name().to_string_lossy().to_string();
            match SemVer::parse(&v, 0) {
                Ok(ver) => out.push(ver),
                Err(_) => {
                    return Err(format!("index: bad version dir `{name}/{v}`"));
                }
            }
        }
        if out.is_empty() {
            return Err(format!("index: package `{name}` has no versions"));
        }
        out.sort();
        Ok(out)
    }

    fn entry(&self, name: &str, version: SemVer) -> Result<IndexEntry, String> {
        let dir = self.version_dir(name, version);
        if !dir.is_dir() {
            return Err(format!("index: unknown version `{name}@{version}`"));
        }
        let manifest_text = self.read_file(&dir.join("manifest.nvpm")).map_err(|e| {
            format!("index: {name}@{version}: {e}")
        })?;
        let manifest = manifest::parse_manifest(&manifest_text)
            .map_err(|e| format!("index: {name}@{version}: bad manifest: {e}"))?;
        if manifest.package.name != name {
            return Err(format!(
                "index: {name}@{version}: manifest names `{}`",
                manifest.package.name
            ));
        }
        if manifest.package.version != version {
            return Err(format!(
                "index: {name}@{version}: manifest versions `{}`",
                manifest.package.version
            ));
        }
        let tarball = fs::read(dir.join("pkg.bin"))
            .map_err(|_| format!("index: {name}@{version}: missing pkg.bin"))?;
        let content_hash = self.read_file(&dir.join("pkg.hash"))?;
        if !content_hash.starts_with("sha256:") {
            return Err(format!(
                "index: {name}@{version}: unknown hash prefix in pkg.hash"
            ));
        }
        let signature_hex = self.read_file(&dir.join("pkg.sig"))?;
        let key_id = self.read_file(&dir.join("pkg.key"))?;
        let pubkey_hex = self
            .read_file(&self.root.join("_keys").join(format!("{key_id}.pub")))
            .map_err(|_| format!("index: {name}@{version}: no key `{key_id}` in key directory"))?;
        Ok(IndexEntry {
            manifest,
            tarball,
            content_hash,
            signature_hex,
            key_id,
            pubkey_hex,
        })
    }
}

// ── Resolution ──────────────────────────────────────────────────────────────

/// Resolve a manifest's full `Registry`-source closure against an
/// index. Returns canonical (name-sorted) lock entries. `Path` deps
/// are skipped (the caller's tree — resolved by `cmd_add`'s sibling
/// logic); `Git` deps fail loud (network fetch deferred).
pub fn resolve(manifest: &Manifest, index: &dyn Registry) -> Result<Vec<LockedPackage>, String> {
    // Accumulated requirements per package: (req, tier, chain).
    let mut reqs: BTreeMap<String, Vec<(VersionReq, Tier, Vec<String>)>> = BTreeMap::new();
    for dep in manifest.dependencies.iter().chain(manifest.dev_dependencies.iter()) {
        match &dep.source {
            Source::Registry => {
                reqs.entry(dep.name.clone())
                    .or_default()
                    .push((dep.req, dep.tier, vec![manifest.package.name.clone()]));
            }
            Source::Path(_) => {}
            Source::Git { url, .. } => {
                return Err(format!(
                    "cannot resolve `{}` from git source {url} without network fetch (deferred)",
                    dep.name
                ));
            }
        }
    }

    // Fixpoint: picks only move down as constraints accumulate, over a
    // finite version set — terminates (bounded defensively anyway).
    let mut locked: BTreeMap<String, SemVer> = BTreeMap::new();
    for _ in 0..1000 {
        let mut changed = false;
        // Snapshot names so expansion can extend `reqs` mid-loop.
        let names: Vec<String> = reqs.keys().cloned().collect();
        for name in &names {
            // Own this package's requirement list: expansion below
            // pushes new requirements while we still read these.
            let list: Vec<(VersionReq, Tier, Vec<String>)> = reqs[name].clone();
            let candidates = index.versions(name).map_err(|_| {
                let chains: Vec<String> =
                    list.iter().map(|(_, _, c)| c.join(" → ")).collect();
                format!(
                    "cannot resolve `{name}`: unknown package (required by {})",
                    chains.join(", ")
                )
            })?;
            let best = candidates
                .iter()
                .rev()
                .find(|v| list.iter().all(|(r, _, _)| manifest::req_satisfied(*r, **v)))
                .copied();
            let best = match best {
                Some(v) => v,
                None => {
                    let wants: Vec<String> = list
                        .iter()
                        .map(|(r, _, c)| format!("`{}` (via {})", req_text(*r), c.join(" → ")))
                        .collect();
                    return Err(format!(
                        "cannot resolve `{name}`: no indexed version satisfies {}",
                        wants.join(", ")
                    ));
                }
            };
            // Tier agreement across requirers (tiers are declared facts,
            // never inferred — disagreement is a conflict, not a vote).
            let tier = list[0].1;
            if list.iter().any(|(_, t, _)| *t != tier) {
                return Err(format!("cannot resolve `{name}`: requirers disagree on tier"));
            }
            if locked.get(name) != Some(&best) {
                locked.insert(name.clone(), best);
                changed = true;
            }
            // Expand transitive deps (registry sources only; path/git
            // inside published manifests fail loud at fetch, not here —
            // no wait: fail HERE, at resolve, naming the chain. A
            // published manifest naming a path dep is nonsense.
            let entry = index.entry(name, best)?;
            let chain_base = list[0].2.clone();
            for dep in entry.manifest.dependencies.iter().chain(entry.manifest.dev_dependencies.iter()) {
                let mut chain = chain_base.clone();
                chain.push(name.clone());
                match &dep.source {
                    Source::Registry => {
                        // P-003 §4 transitivity: foreign-runtime reachable
                        // only transitively is an error naming the chain,
                        // unless the top manifest declares it with opt-in.
                        if dep.tier == Tier::ForeignRuntime && !top_opt_in(manifest, &dep.name) {
                            let mut full = chain.clone();
                            full.push(dep.name.clone());
                            return Err(format!(
                                "cannot resolve `{}`: foreign-runtime package reachable only transitively ({}) — declare it directly with `opt_in: true`",
                                dep.name,
                                full.join(" → ")
                            ));
                        }
                        reqs.entry(dep.name.clone())
                            .or_default()
                            .push((dep.req, dep.tier, chain));
                    }
                    Source::Path(p) => {
                        return Err(format!(
                            "cannot resolve `{}`: published manifest of `{name}` names a path source `{p}` (unpublishable)",
                            dep.name
                        ));
                    }
                    Source::Git { url, .. } => {
                        return Err(format!(
                            "cannot resolve `{}`: published manifest of `{name}` names git source {url} (network fetch deferred)",
                            dep.name
                        ));
                    }
                }
            }
        }
        if !changed {
            // Locked set stable — but new reqs may have arrived during
            // the last expansion; loop once more to confirm. Track via
            // a second quiet pass: simplest is one extra iteration.
            // (Changed=false with pending unseen reqs is impossible
            // here: unseen reqs only arrive via expansion, and any
            // expansion sets changed=true through its own insert…
            // except when the pick is unchanged. A re-pick with more
            // constraints can only move down, which sets changed.
            // So: stable means closed.)
            break;
        }
    }

    // Materialize lock entries (tiers/sources from the requirers).
    let mut out = Vec::new();
    for (name, version) in &locked {
        let list = &reqs[name];
        let entry = index.entry(name, *version)?;
        out.push(LockedPackage {
            name: name.clone(),
            version: *version,
            source: Source::Registry,
            content: Some(entry.content_hash),
            tier: list[0].1,
            signed_by: Some(entry.key_id),
            experimental: list[0].1 == Tier::CxxShim,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Top-level opt-in for a foreign-runtime package (P-003 §4: opt-in
/// is per top-level project, never inherited).
fn top_opt_in(manifest: &Manifest, name: &str) -> bool {
    manifest
        .dependencies
        .iter()
        .chain(manifest.dev_dependencies.iter())
        .any(|d| d.name == *name && d.tier == Tier::ForeignRuntime && d.opt_in)
}

fn req_text(req: VersionReq) -> String {
    match req {
        VersionReq::Caret(v) => v.to_string(),
        VersionReq::Exact(v) => format!("={v}"),
    }
}

// ── Fetch + verify + cache ──────────────────────────────────────────────────

/// Cache locations under a project root.
pub fn cache_file(project: &Path, name: &str, version: SemVer) -> PathBuf {
    project.join(".noct").join("cache").join(format!("{name}-{version}.pkg"))
}

pub fn unpack_dir(project: &Path, name: &str, version: SemVer) -> PathBuf {
    project.join(".noct").join("packages").join(format!("{name}-{version}"))
}

/// Fetch one locked registry package: bytes → hash check → TOFU key
/// check → signature check → cache + unpack. `rotate_key` re-prints
/// and re-records the index key explicitly (never automatic).
/// `keys` is the TOFU directory (production: `key_dir()`).
pub fn fetch_locked(
    locked: &LockedPackage,
    index: &dyn Registry,
    project: &Path,
    keys: &Path,
    rotate_key: bool,
) -> Result<(), String> {
    let entry = index.entry(&locked.name, locked.version)?;
    let expected = locked.content.as_deref().ok_or_else(|| {
        format!("lock entry `{}` has no content hash", locked.name)
    })?;
    if entry.content_hash != expected {
        return Err(format!(
            "index moved under us: `{}` lock hash {expected} != index hash {}",
            locked.name, entry.content_hash
        ));
    }
    let actual = sha256_hex(&entry.tarball);
    if actual != expected {
        return Err(format!(
            "hash mismatch for `{}@{}`: expected {expected}, fetched {actual} (never warn-and-continue)",
            locked.name, locked.version
        ));
    }
    let key_id = locked.signed_by.as_deref().ok_or_else(|| {
        format!("lock entry `{}` has no signed_by", locked.name)
    })?;
    if entry.key_id != key_id {
        return Err(format!(
            "key-id moved under us: `{}` lock says {key_id}, index says {}",
            locked.name, entry.key_id
        ));
    }
    // TOFU with a paper trail.
    match load_key_in(keys, key_id) {
        Some(stored) if stored == entry.pubkey_hex => {}
        Some(_) if rotate_key => {
            println!("rotated publisher key {key_id} for `{}` (re-recorded)", locked.name);
            store_key_in(keys, key_id, &entry.pubkey_hex)?;
        }
        Some(_) => {
            return Err(format!(
                "publisher key mismatch for `{}` (key {key_id} changed) — re-add with --rotate-key after verifying out of band",
                locked.name
            ));
        }
        None => {
            println!(
                "new publisher key {key_id} for `{}` (trust-on-first-use, recorded)",
                locked.name
            );
            store_key_in(keys, key_id, &entry.pubkey_hex)?;
        }
    }
    let payload = sign_payload(&locked.name, locked.version, expected);
    verify_signature(&entry.pubkey_hex, &payload, &entry.signature_hex).map_err(|e| {
        format!(
            "signature mismatch for `{}@{}`: {e}",
            locked.name, locked.version
        )
    })?;

    let cache = cache_file(project, &locked.name, locked.version);
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create cache: {e}"))?;
    }
    fs::write(&cache, &entry.tarball).map_err(|e| format!("cannot write cache: {e}"))?;
    let dest = unpack_dir(project, &locked.name, locked.version);
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
    }
    fs::create_dir_all(&dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    for (rel, bytes) in unpack(&entry.tarball)? {
        let target = dest.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create dir: {e}"))?;
        }
        fs::write(&target, &bytes).map_err(|e| format!("cannot unpack {rel}: {e}"))?;
    }
    Ok(())
}

/// Re-verify cached tarballs against the lock (P-003 §6: verified on
/// every build that touches the cache). Returns problems; empty = ok.
pub fn check_cache_current(lock: &Lockfile, project: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for pkg in &lock.packages {
        if !matches!(pkg.source, Source::Registry) {
            continue;
        }
        let cache = cache_file(project, &pkg.name, pkg.version);
        let bytes = match fs::read(&cache) {
            Ok(b) => b,
            Err(_) => {
                problems.push(format!(
                    "`{}` not in cache (run `noct add` to fetch)",
                    pkg.name
                ));
                continue;
            }
        };
        match &pkg.content {
            Some(expected) => {
                let actual = sha256_hex(&bytes);
                if &actual != expected {
                    problems.push(format!(
                        "`{}` cache hash mismatch: expected {expected}, found {actual}",
                        pkg.name
                    ));
                }
            }
            None => problems.push(format!("`{}` lock entry has no content hash", pkg.name)),
        }
    }
    problems
}

/// Manifest-vs-lock-vs-cache gate for build/run/test (P-003 §6: the
/// lock is the build input — a stale lock is an error prompting
/// `add`, never a silent re-resolve). Skips silently when no
/// manifest is present (single-file use has no package context).
/// `project` is the directory holding `nestpkg.nvpm` (usually cwd).
pub fn require_package_current(project: &Path) -> Result<(), String> {
    let manifest_path = project.join("nestpkg.nvpm");
    let manifest_text = match fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let manifest = manifest::parse_manifest(&manifest_text)
        .map_err(|e| format!("invalid manifest: {e}"))?;
    let needs_lock =
        !manifest.dependencies.is_empty() || !manifest.dev_dependencies.is_empty();
    let lock_path = project.join("nestpkg.lock");
    let lock_text = match fs::read_to_string(&lock_path) {
        Ok(t) => t,
        Err(_) => {
            if needs_lock {
                return Err("lockfile missing (run `noct add` to establish nestpkg.lock)".to_string());
            }
            return Ok(());
        }
    };
    let lock = manifest::parse_lockfile(&lock_text)
        .map_err(|e| format!("invalid lockfile: {e}"))?;
    let mut problems = manifest::check_lock_current(&manifest, &lock);
    problems.extend(check_cache_current(&lock, project));
    if problems.is_empty() {
        return Ok(());
    }
    let mut msg = String::from("package is not current:");
    for p in problems {
        msg.push_str(&format!("\n  - {p}"));
    }
    msg.push_str("\n(run `noct add` to refresh)");
    Err(msg)
}

/// Names every package an index entry's manifest requires (both
/// tables) — used by tests and future `audit --why` work.
#[allow(dead_code)]
pub fn transitive_names(entry_manifest: &Manifest) -> BTreeSet<String> {
    entry_manifest
        .dependencies
        .iter()
        .chain(entry_manifest.dev_dependencies.iter())
        .map(|d| d.name.clone())
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Fixed publisher key (deterministic fixtures — no RNG needed).
    const SEED: [u8; 32] = [0x42; 32];
    const KEY_ID: &str = "key:4242";

    fn scratch() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("noctivue-regtest-{}-{id}", id));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One fixture package version. deps: (name, req-text, tier-or-"native").
    struct Fixture {
        name: &'static str,
        version: &'static str,
        deps: &'static [(&'static str, &'static str, &'static str)],
        files: &'static [(&'static str, &'static str)],
    }

    /// Build a file index + return its root. Every version signed with SEED.
    fn build_index(dir: &Path, pkgs: &[Fixture]) -> PathBuf {
        let root = dir.join("index");
        let pubkey = pubkey_hex(&SEED);
        fs::create_dir_all(root.join("_keys")).unwrap();
        fs::write(root.join("_keys").join(format!("{KEY_ID}.pub")), format!("{pubkey}\n")).unwrap();
        for pkg in pkgs {
            let vdir = root.join(pkg.name).join(pkg.version);
            fs::create_dir_all(&vdir).unwrap();
            let mut manifest = format!("package:\n    name: {}\n    version: {}\n", pkg.name, pkg.version);
            if !pkg.deps.is_empty() {
                manifest.push_str("dependencies:\n");
                for (name, req, tier) in pkg.deps {
                    if *tier == "native" {
                        manifest.push_str(&format!("    {name}: {req}\n"));
                    } else {
                        // The publisher's own opt-in (schema requires it
                        // alongside foreign-runtime); whether the TOP
                        // project inherits it is the transitivity rule's
                        // business, tested below — never inherited.
                        let opt = if *tier == "foreign-runtime" {
                            "\n        opt_in: true"
                        } else {
                            ""
                        };
                        manifest.push_str(&format!(
                            "    {name}:\n        version: {req}\n        tier: {tier}{opt}\n"
                        ));
                    }
                }
            }
            fs::write(vdir.join("manifest.nvpm"), &manifest).unwrap();
            let files: Vec<(&str, &[u8])> = pkg.files.iter().map(|(p, c)| (*p, c.as_bytes())).collect();
            let tarball = pack(&files).unwrap();
            let hash = sha256_hex(&tarball);
            let ver = SemVer::parse(pkg.version, 0).unwrap();
            let payload = sign_payload(pkg.name, ver, &hash);
            let sig = encode_hex(&sign_bytes(&SEED, &payload));
            fs::write(vdir.join("pkg.bin"), &tarball).unwrap();
            fs::write(vdir.join("pkg.hash"), format!("{hash}\n")).unwrap();
            fs::write(vdir.join("pkg.sig"), format!("{sig}\n")).unwrap();
            fs::write(vdir.join("pkg.key"), format!("{KEY_ID}\n")).unwrap();
        }
        root
    }

    fn top_manifest(deps: &str) -> Manifest {
        manifest::parse_manifest(&format!("package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n{deps}")).unwrap()
    }

    #[test]
    fn tarball_round_trip_and_path_jail() {
        let packed = pack(&[("lib/main.nv", b"main():\n"), ("a/b.nv", b"x")]).unwrap();
        let back = unpack(&packed).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].0, "lib/main.nv");
        assert_eq!(back[0].1, b"main():\n");
        assert!(pack(&[("../evil", b"x")]).is_err());
        assert!(unpack(b"\x02\x00").is_err());
        assert!(unpack(b"").is_err());
    }

    #[test]
    fn sign_verify_round_trip_and_mismatch_modes() {
        let payload = sign_payload("http", SemVer::parse("1.2.0", 0).unwrap(), "sha256:abc");
        let good = encode_hex(&sign_bytes(&SEED, &payload));
        let pubkey = pubkey_hex(&SEED);
        assert!(verify_signature(&pubkey, &payload, &good).is_ok());
        // Bad signature bytes.
        let mut bad = sign_bytes(&SEED, &payload);
        bad[0] ^= 0xff;
        assert!(verify_signature(&pubkey, &payload, &encode_hex(&bad)).is_err());
        // Wrong key.
        let other = pubkey_hex(&[0x13; 32]);
        assert!(verify_signature(&other, &payload, &good).is_err());
        // Malformed inputs fail closed (never panic).
        assert!(verify_signature("zz", &payload, &good).is_err());
        assert!(verify_signature(&pubkey, &payload, "zz").is_err());
        assert!(verify_signature(&pubkey, &payload, &encode_hex(&[0u8; 32])).is_err());
    }

    #[test]
    fn resolve_picks_highest_and_walks_transitives() {
        let dir = scratch();
        let root = build_index(
            &dir,
            &[
                Fixture { name: "leaf", version: "1.0.0", deps: &[], files: &[("lib.nv", "a")] },
                Fixture { name: "leaf", version: "1.4.0", deps: &[], files: &[("lib.nv", "b")] },
                Fixture { name: "leaf", version: "2.0.0", deps: &[], files: &[("lib.nv", "c")] },
                Fixture {
                    name: "mid",
                    version: "0.5.0",
                    deps: &[("leaf", "1.0.0", "native")],
                    files: &[("lib.nv", "m")],
                },
            ],
        );
        let index = FileIndex::new(root);
        let manifest = top_manifest("    mid: 0.5.0\n");
        let locked = resolve(&manifest, &index).unwrap();
        let names: Vec<(&str, String)> =
            locked.iter().map(|p| (p.name.as_str(), p.version.to_string())).collect();
        // ^1.0.0 over {1.0.0, 1.4.0, 2.0.0} → 1.4.0 (highest satisfying).
        assert!(names.contains(&("mid", "0.5.0".to_string())), "{names:?}");
        assert!(names.contains(&("leaf", "1.4.0".to_string())), "{names:?}");
        let leaf = locked.iter().find(|p| p.name == "leaf").unwrap();
        assert_eq!(leaf.tier, Tier::Native);
        assert!(leaf.signed_by.as_deref() == Some(KEY_ID));
        assert!(leaf.content.as_deref().unwrap().starts_with("sha256:"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_conflicts_and_unknowns_are_loud() {
        let dir = scratch();
        let root = build_index(
            &dir,
            &[Fixture { name: "leaf", version: "1.4.0", deps: &[], files: &[("lib.nv", "b")] }],
        );
        let index = FileIndex::new(root);
        // Unsatisfiable requirement.
        let err = resolve(&top_manifest("    leaf: =9.9.9\n"), &index).unwrap_err();
        assert!(err.contains("no indexed version satisfies"), "{err}");
        // Unknown package.
        let err = resolve(&top_manifest("    ghost: 1.0.0\n"), &index).unwrap_err();
        assert!(err.contains("unknown package"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_rejects_transitive_foreign_runtime() {
        let dir = scratch();
        let root = build_index(
            &dir,
            &[
                Fixture {
                    name: "py_model",
                    version: "1.0.0",
                    deps: &[],
                    files: &[("lib.nv", "p")],
                },
                Fixture {
                    name: "warez",
                    version: "1.0.0",
                    deps: &[("py_model", "1.0.0", "foreign-runtime")],
                    files: &[("lib.nv", "w")],
                },
            ],
        );
        let index = FileIndex::new(root);
        let err = resolve(&top_manifest("    warez: 1.0.0\n"), &index).unwrap_err();
        assert!(err.contains("foreign-runtime"), "{err}");
        assert!(err.contains("warez → py_model"), "{err}");
        // Direct declaration with opt_in heals it.
        let manifest = manifest::parse_manifest(
            "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    warez: 1.0.0\n    py_model:\n        version: 1.0.0\n        tier: foreign-runtime\n        opt_in: true\n",
        )
        .unwrap();
        let locked = resolve(&manifest, &index).unwrap();
        assert!(locked.iter().any(|p| p.name == "py_model"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fetch_verifies_hash_sig_and_tofu() {
        let dir = scratch();
        let root = build_index(
            &dir,
            &[Fixture { name: "leaf", version: "1.4.0", deps: &[], files: &[("lib/main.nv", "hi")] }],
        );
        let index = FileIndex::new(root);
        let project = dir.join("proj");
        let keys = dir.join("keys");
        fs::create_dir_all(&project).unwrap();
        let locked = &resolve(&top_manifest("    leaf: 1.0.0\n"), &index).unwrap()[0];

        // First fetch: TOFU prints + records, cache + unpack land.
        fetch_locked(locked, &index, &project, &keys, false).unwrap();
        assert!(keys.join(KEY_ID).exists());
        assert!(cache_file(&project, "leaf", locked.version).exists());
        let main = unpack_dir(&project, "leaf", locked.version).join("lib/main.nv");
        assert_eq!(fs::read_to_string(main).unwrap(), "hi");
        assert!(check_cache_current(
            &Lockfile { lock_version: 1, packages: vec![locked.clone()] },
            &project
        )
        .is_empty());

        // Tampered cache: re-verify fails closed.
        fs::write(cache_file(&project, "leaf", locked.version), b"evil").unwrap();
        let problems = check_cache_current(
            &Lockfile { lock_version: 1, packages: vec![locked.clone()] },
            &project,
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("hash mismatch"), "{problems:?}");

        // Rotated publisher key without --rotate-key: refused.
        let other_pub = pubkey_hex(&[0x77; 32]);
        fs::write(keys.join(KEY_ID), format!("{other_pub}\n")).unwrap();
        // Restore good cache first (fetch re-verifies hash before keys).
        let entry = index.entry("leaf", locked.version).unwrap();
        fs::write(cache_file(&project, "leaf", locked.version), &entry.tarball).unwrap();
        let err = fetch_locked(locked, &index, &project, &keys, false).unwrap_err();
        assert!(err.contains("--rotate-key"), "{err}");
        // With --rotate-key: re-records and succeeds.
        fetch_locked(locked, &index, &project, &keys, true).unwrap();
        assert_eq!(
            fs::read_to_string(keys.join(KEY_ID)).unwrap().trim(),
            pubkey_hex(&SEED)
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
