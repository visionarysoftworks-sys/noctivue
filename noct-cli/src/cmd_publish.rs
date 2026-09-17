//! `noct publish` — publish a package to the Noctivue package registry.
//!
//! [Phase 4 / M3] There is no registry to publish to, and package
//! signing/signature verification — the HARD precondition before any
//! public registry launch (TOOLCHAIN.md §3, P-003 §§6–8: key-id and
//! hash shapes are parsed and carried, values not verified) — is not
//! implemented. A real `publish` is therefore refused with a typed
//! message, never a silent no-op or a half-upload.
//!
//! `noct publish --dry-run` runs the full publish pipeline locally
//! WITHOUT writing to any index or cache:
//! - `nestpkg.nvpm` exists and parses (schema, names, SemVer, reqs,
//!   tier + opt-in rules, sources);
//! - `nestpkg.lock`, when dependency entries require one, exists,
//!   parses (presence rules: registry/git entries need `content:` +
//!   `signed_by:`, path entries skip both), and is current against
//!   the manifest (`check_lock_current`: satisfying versions, same
//!   tiers, same source shapes, no stale entries);
//! - tarball assembly + hash: `nestpkg.nvpm` bytes plus every `*.nv`
//!   source under the project (sorted, `/`-separated), excluding
//!   `.noct/`, `vendor/`, `target/`, `.git/`, `nestpkg.lock`, any
//!   `path:`-dependency subtree (a separate package, never bundled),
//!   and the `--sbom` output file itself when it lives in-project.
//!   Framed with the registry tarball v1 codec (`registry::pack`),
//!   hashed with `sha256:<hex>`;
//! - signature check: payload
//!   `noctivue-publish-v1:<name>@<version>:<content-hash>` signed with
//!   `NOCT_SIGNING_KEY` (hex 32-byte seed) and self-verified locally.
//!   The root tier comes from `--tier` (the manifest has no root-tier
//!   field; default `native`). `native` may dry-run UNSIGNED with a
//!   warning; every non-native tier (`c-shim`, `cxx-shim`,
//!   `wasm-component`, `foreign-runtime`) REQUIRES a signing key and
//!   fails cleanly without one. `signed_by` is `NOCT_KEY_ID` when set,
//!   else `key:local`.
//!
//! Prints what WOULD be published (`name version tier content-hash
//! signed_by`, or an UNSIGNED warning) and exits 0 on success,
//! non-zero on any validation failure. Never creates or mutates
//! `<index>/`, `.noct/cache/`, or `.noct/packages/` — the only file
//! ever written is the explicit `--sbom` output.
//!
//! SBOM: `--sbom <file>` writes a minimal JSON closure
//! (`sbom_version`, `root`, `packages[]` with name/version/tier/
//! source/content_hash/signed_by per entry; `content_hash` and
//! `signed_by` are `null` for `path` sources — your tree, nothing to
//! verify). `--sbom` implies `--dry-run` (there is no upload to
//! attach it to); `noct publish --dry-run --sbom <file>` is the
//! explicit form and behaves identically.
//!
//! Usage:
//!   noct publish --dry-run [--tier <tier>] [--sbom <file>]
//!   noct publish --sbom <file> [--tier <tier>]   (implies --dry-run)
//!   noct publish                (refused: no registry + no signing)

use std::fs;
use std::path::{Path, PathBuf};

pub fn run(args: &[String]) -> i32 {
    let mut dry_run = false;
    let mut sbom: Option<String> = None;
    let mut tier_arg: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--sbom" {
            i += 1;
            if i >= args.len() {
                eprintln!("noct publish: --sbom requires a file path (usage: noct publish --dry-run [--tier <tier>] [--sbom <file>])");
                return 1;
            }
            sbom = Some(args[i].clone());
        } else if let Some(p) = arg.strip_prefix("--sbom=") {
            if p.is_empty() {
                eprintln!("noct publish: --sbom requires a file path (usage: noct publish --dry-run [--tier <tier>] [--sbom <file>])");
                return 1;
            }
            sbom = Some(p.to_string());
        } else if arg == "--tier" {
            i += 1;
            if i >= args.len() {
                eprintln!("noct publish: --tier requires a value (expected native, c-shim, cxx-shim, wasm-component, or foreign-runtime)");
                return 1;
            }
            tier_arg = Some(args[i].clone());
        } else if let Some(p) = arg.strip_prefix("--tier=") {
            if p.is_empty() {
                eprintln!("noct publish: --tier requires a value (expected native, c-shim, cxx-shim, wasm-component, or foreign-runtime)");
                return 1;
            }
            tier_arg = Some(p.to_string());
        } else if arg == "--help" || arg == "-h" {
            println!("usage: noct publish --dry-run [--tier <tier>] [--sbom <file>]");
            println!();
            println!("Run the full publish pipeline locally without uploading:");
            println!("manifest parse, lock-current check, tarball assembly, content");
            println!("hash, and signature check. Prints what WOULD be published.");
            println!();
            println!("  --tier <tier>   root package tier (default native; non-native");
            println!("                  tiers require NOCT_SIGNING_KEY to be set)");
            println!("  --sbom <file>   write a minimal JSON SBOM for the closure");
            println!("                  (implies --dry-run; only file ever written)");
            println!("Signing: NOCT_SIGNING_KEY=hex-32-byte-seed, NOCT_KEY_ID=<id>");
            return 0;
        } else {
            eprintln!("noct publish: unknown flag `{arg}` (usage: noct publish --dry-run [--tier <tier>] [--sbom <file>])");
            return 1;
        }
        i += 1;
    }
    // `--sbom` without `--dry-run` is still a dry run: there is no
    // upload path, so the only honest reading is validate + emit SBOM.
    if sbom.is_some() {
        dry_run = true;
    }
    if !dry_run {
        eprintln!(
            "noct publish: refused — no package registry is configured, and package \
             signing/signature verification (the hard precondition in TOOLCHAIN.md §3) \
             is not implemented; nothing was published"
        );
        eprintln!("hint: `noct publish --dry-run` validates manifest + lockfile locally");
        return 1;
    }

    let manifest_path = Path::new("nestpkg.nvpm");
    let manifest_text = match fs::read_to_string(manifest_path) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("noct publish: no nestpkg.nvpm in current directory");
            return 1;
        }
    };
    let manifest = match crate::manifest::parse_manifest(&manifest_text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("noct publish: invalid manifest: {e}");
            return 1;
        }
    };
    println!(
        "package {} {}: manifest ok",
        manifest.package.name, manifest.package.version
    );

    let needs_lock = !manifest.dependencies.is_empty() || !manifest.dev_dependencies.is_empty();
    let lock_path = Path::new("nestpkg.lock");
    let lock_text = match fs::read_to_string(lock_path) {
        Ok(t) => Some(t),
        Err(_) => None,
    };
    match (needs_lock, lock_text.as_ref()) {
        (false, None) => {
            println!("no lockfile: nothing to lock (no dependencies)");
        }
        (false, Some(_)) => {
            // A lock with no manifest deps is stale by definition —
            // still run the checker so the message is uniform.
        }
        (true, None) => {
            eprintln!(
                "noct publish: lockfile missing (run `noct add` to establish nestpkg.lock); not ready"
            );
            return 1;
        }
        _ => {}
    }

    // Root tier: the manifest has no root-tier field, so `--tier`
    // declares what tier this package would publish AS (default
    // native). Non-native tiers vouch for foreign code and require a
    // signature even for a dry run.
    let root_tier = match tier_arg.as_deref() {
        None => crate::manifest::Tier::Native,
        Some(t) => match crate::manifest::Tier::parse(t, 0) {
            Ok(tier) => tier,
            Err(e) => {
                eprintln!("noct publish: invalid tier '{t}': {e}");
                return 1;
            }
        },
    };

    // Keep the parsed lock (when present) for the SBOM closure. The
    // drift check above already gated staleness; this re-parse cannot
    // fail (same bytes), but fail loudly rather than assuming.
    let lock_for_sbom: Option<crate::manifest::Lockfile> = match lock_text.as_deref() {
        Some(text) => {
            let lock = match crate::manifest::parse_lockfile(text) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("noct publish: invalid lockfile: {e}; not ready");
                    return 1;
                }
            };
            let drift = crate::manifest::check_lock_current(&manifest, &lock);
            if !drift.is_empty() {
                eprintln!("noct publish: lockfile drift:");
                for problem in &drift {
                    eprintln!("  - {problem}");
                }
                eprintln!("not ready (run `noct add` to refresh the lock)");
                return 1;
            }
            println!("lockfile current ({} packages)", lock.packages.len());
            Some(lock)
        }
        None => None,
    };

    // ── Tarball assembly + hash (read-only; never touches index/cache)
    let sbom_path = sbom.as_deref().map(PathBuf::from);
    let entries = match collect_publish_entries(Path::new("."), &manifest, &manifest_text, sbom_path.as_deref()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("noct publish: cannot assemble tarball: {e}");
            return 1;
        }
    };
    let entry_refs: Vec<(&str, &[u8])> =
        entries.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
    let tarball = match crate::registry::pack(&entry_refs) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("noct publish: cannot assemble tarball: {e}");
            return 1;
        }
    };
    let content_hash = crate::registry::sha256_hex(&tarball);

    // ── Signature check (local self-verify; no network, no writes)
    let signed_by: Option<String> = match signing_key_from_env() {
        Ok(None) => None,
        Ok(Some((key_id, seed))) => {
            use ed25519_dalek::{Signer, SigningKey};
            let sk = SigningKey::from_bytes(&seed);
            let pubkey_hex = {
                let bytes = sk.verifying_key().to_bytes();
                let mut out = String::with_capacity(64);
                for b in bytes {
                    out.push_str(&format!("{b:02x}"));
                }
                out
            };
            let payload = crate::registry::sign_payload(
                &manifest.package.name,
                manifest.package.version,
                &content_hash,
            );
            let sig_bytes = sk.sign(payload.as_bytes()).to_bytes();
            let mut sig_hex = String::with_capacity(128);
            for b in sig_bytes {
                sig_hex.push_str(&format!("{b:02x}"));
            }
            if let Err(e) =
                crate::registry::verify_signature(&pubkey_hex, &payload, &sig_hex)
            {
                eprintln!("noct publish: local signature self-check failed: {e}");
                return 1;
            }
            Some(key_id)
        }
        Err(e) => {
            eprintln!("noct publish: {e}");
            return 1;
        }
    };

    match &signed_by {
        Some(key) => {
            println!(
                "would publish {} {} tier {} hash {content_hash} signed_by {key}",
                manifest.package.name,
                manifest.package.version,
                root_tier.as_str(),
            );
        }
        None => {
            if root_tier != crate::manifest::Tier::Native {
                eprintln!(
                    "noct publish: refusing to publish unsigned {} package `{}`@{} (tier {} requires signing; set NOCT_SIGNING_KEY=<hex-32-byte-seed> and optionally NOCT_KEY_ID=<id>)",
                    root_tier.as_str(),
                    manifest.package.name,
                    manifest.package.version,
                    root_tier.as_str(),
                );
                return 1;
            }
            println!(
                "would publish {} {} tier {} hash {content_hash} UNSIGNED (warning: no NOCT_SIGNING_KEY in environment — native tier allows an unsigned dry run)",
                manifest.package.name,
                manifest.package.version,
                root_tier.as_str(),
            );
        }
    }

    if let Some(path) = sbom_path.as_deref() {
        let sbom_json = render_sbom(
            &manifest,
            root_tier,
            &content_hash,
            signed_by.as_deref(),
            lock_for_sbom.as_ref(),
        );
        if let Err(e) = fs::write(path, sbom_json.as_bytes()) {
            eprintln!("noct publish: cannot write SBOM {}: {e}", path.display());
            return 1;
        }
        let count = lock_for_sbom.as_ref().map(|l| l.packages.len()).unwrap_or(0);
        println!("wrote SBOM to {} ({} packages + root)", path.display(), count);
    }

    println!("ready to publish (dry run — nothing was uploaded)");
    0
}

/// `NOCT_SIGNING_KEY` (hex 32-byte seed) + `NOCT_KEY_ID` (default
/// `key:local`). `Ok(None)` = unsigned dry run. Every malformed input
/// is a clean error string, never a panic.
fn signing_key_from_env() -> Result<Option<(String, [u8; 32])>, String> {
    let raw = match std::env::var("NOCT_SIGNING_KEY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(None),
    };
    let hex = raw.trim();
    if hex.len() != 64 {
        return Err(format!(
            "bad NOCT_SIGNING_KEY (expected 64 hex chars for a 32-byte seed, found {} chars)",
            hex.len()
        ));
    }
    let mut seed = [0u8; 32];
    for i in 0..32 {
        match u8::from_str_radix(&hex[2 * i..2 * i + 2], 16) {
            Ok(b) => seed[i] = b,
            Err(_) => {
                return Err(format!(
                    "bad NOCT_SIGNING_KEY (not hex at byte {i}; expected 64 hex chars)"
                ));
            }
        }
    }
    let key_id = match std::env::var("NOCT_KEY_ID") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => "key:local".to_string(),
    };
    Ok(Some((key_id, seed)))
}

/// Collect `(relative-path, bytes)` for the would-be tarball. Read-only:
/// walks `.` for `*.nv` files (sorted for determinism) plus the exact
/// `nestpkg.nvpm` bytes already read. Skips `.noct/`, `vendor/`,
/// `target/`, `.git/`, `nestpkg.lock`, every `path:`-dependency subtree,
/// and the SBOM output itself when it lives in-project.
fn collect_publish_entries(
    project: &Path,
    manifest: &crate::manifest::Manifest,
    manifest_text: &str,
    sbom_path: Option<&Path>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    use crate::manifest::Source;

    let mut excluded_dirs: Vec<String> = Vec::new();
    for dep in manifest.dependencies.iter().chain(manifest.dev_dependencies.iter()) {
        if let Source::Path(p) = &dep.source {
            // Normalize `sibling`, `./sibling`, `sibling/` to `sibling`.
            let mut norm = p.replace('\\', "/");
            while norm.starts_with("./") {
                norm = norm[2..].to_string();
            }
            while norm.ends_with('/') && norm.len() > 1 {
                norm.pop();
            }
            if !norm.is_empty() && norm != "." {
                excluded_dirs.push(norm);
            }
        }
    }
    // SBOM output living in-project must not poison its own hash.
    let mut sbom_rel: Option<String> = None;
    if let Some(p) = sbom_path {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.strip_prefix("./").unwrap_or(&s).to_string();
        if !s.contains("..") && !Path::new(&s).is_absolute() {
            sbom_rel = Some(s);
        } else if let Ok(cwd) = std::env::current_dir() {
            if let Ok(rel) = p.strip_prefix(&cwd) {
                sbom_rel = Some(rel.to_string_lossy().replace('\\', "/"));
            } else {
                // Best effort for absolute test paths: match by file name.
                sbom_rel = p.file_name().map(|n| n.to_string_lossy().into_owned());
            }
        }
    }

    let mut out: Vec<(String, Vec<u8>)> = vec![("nestpkg.nvpm".to_string(), manifest_text.as_bytes().to_vec())];

    let mut stack = vec![project.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = fs::read_dir(&dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
        let mut names: Vec<PathBuf> = Vec::new();
        for entry in rd {
            let entry = entry.map_err(|e| format!("cannot read dir entry: {e}"))?;
            names.push(entry.path());
        }
        names.sort();
        for path in names {
            let rel = path
                .strip_prefix(project)
                .map_err(|_| "cannot relativize project file".to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if rel.is_empty() {
                continue;
            }
            let top = rel.split('/').next().unwrap_or("");
            if matches!(top, ".noct" | "vendor" | "target" | ".git") {
                continue;
            }
            if rel == "nestpkg.lock" || rel == "nestpkg.nvpm" {
                continue;
            }
            if let Some(s) = sbom_rel.as_deref() {
                if rel == s {
                    continue;
                }
            }
            if excluded_dirs.iter().any(|d| rel == *d || rel.starts_with(&format!("{d}/"))) {
                continue;
            }
            let meta = fs::symlink_metadata(&path).map_err(|e| format!("cannot stat {rel}: {e}"))?;
            if meta.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("nv") {
                let bytes =
                    fs::read(&path).map_err(|e| format!("cannot read {rel}: {e}"))?;
                out.push((rel, bytes));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Minimal JSON SBOM: `{"sbom_version":1,"root":{...},"packages":[...]}`.
/// `content_hash`/`signed_by` are strings or `null` (null exactly for
/// `path` sources, mirroring the lock). Hand-rolled escaping (no new
/// dependencies): `"`/`\\`/controls escaped, everything else literal.
fn render_sbom(
    manifest: &crate::manifest::Manifest,
    root_tier: crate::manifest::Tier,
    root_hash: &str,
    root_signed_by: Option<&str>,
    lock: Option<&crate::manifest::Lockfile>,
) -> String {
    fn esc(s: &str) -> String {
        let mut o = String::with_capacity(s.len() + 2);
        for c in s.chars() {
            match c {
                '"' => o.push_str("\\\""),
                '\\' => o.push_str("\\\\"),
                '\n' => o.push_str("\\n"),
                '\r' => o.push_str("\\r"),
                '\t' => o.push_str("\\t"),
                c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
                c => o.push(c),
            }
        }
        o
    }
    fn opt_str(v: Option<&str>) -> String {
        match v {
            Some(s) => format!("\"{}\"", esc(s)),
            None => "null".to_string(),
        }
    }

    let mut out = String::from("{\n");
    out.push_str("  \"sbom_version\": 1,\n");
    out.push_str("  \"root\": {\n");
    out.push_str(&format!("    \"name\": \"{}\",\n", esc(&manifest.package.name)));
    out.push_str(&format!("    \"version\": \"{}\",\n", manifest.package.version));
    out.push_str(&format!("    \"tier\": \"{}\",\n", root_tier.as_str()));
    out.push_str(&format!("    \"content_hash\": \"{}\",\n", esc(root_hash)));
    out.push_str(&format!("    \"signed_by\": {}\n", opt_str(root_signed_by)));
    out.push_str("  },\n");
    out.push_str("  \"packages\": [\n");
    if let Some(lock) = lock {
        let mut pkgs = lock.packages.clone();
        pkgs.sort_by(|a, b| a.name.cmp(&b.name));
        for (idx, p) in pkgs.iter().enumerate() {
            let source = match &p.source {
                crate::manifest::Source::Registry => "registry",
                crate::manifest::Source::Path(_) => "path",
                crate::manifest::Source::Git { .. } => "git",
            };
            out.push_str("    {\n");
            out.push_str(&format!("      \"name\": \"{}\",\n", esc(&p.name)));
            out.push_str(&format!("      \"version\": \"{}\",\n", p.version));
            out.push_str(&format!("      \"tier\": \"{}\",\n", p.tier.as_str()));
            out.push_str(&format!("      \"source\": \"{source}\",\n"));
            out.push_str(&format!("      \"content_hash\": {},\n", opt_str(p.content.as_deref())));
            out.push_str(&format!("      \"signed_by\": {}\n", opt_str(p.signed_by.as_deref())));
            if idx + 1 == pkgs.len() {
                out.push_str("    }\n");
            } else {
                out.push_str("    },\n");
            }
        }
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}
