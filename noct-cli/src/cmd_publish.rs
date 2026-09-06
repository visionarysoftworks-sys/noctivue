//! `noct publish` — publish a package to the Noctivue package registry.
//!
//! [Phase 4 / M3] There is no registry to publish to, and package
//! signing/signature verification — the HARD precondition before any
//! public registry launch (TOOLCHAIN.md §3, P-003 §§6–8: key-id and
//! hash shapes are parsed and carried, values not verified) — is not
//! implemented. A real `publish` is therefore refused with a typed
//! message, never a silent no-op or a half-upload.
//!
//! `noct publish --dry-run` validates everything publish would check
//! locally without touching the network or any file:
//! - `nestpkg.nvpm` exists and parses (schema, names, SemVer, reqs,
//!   tier + opt-in rules, sources);
//! - `nestpkg.lock`, when dependency entries require one, exists,
//!   parses (presence rules: registry/git entries need `content:` +
//!   `signed_by:`, path entries skip both), and is current against
//!   the manifest (`check_lock_current`: satisfying versions, same
//!   tiers, same source shapes, no stale entries).
//!
//! Usage:
//!   noct publish --dry-run
//!   noct publish                (refused: no registry + no signing)

use std::fs;
use std::path::Path;

pub fn run(args: &[String]) -> i32 {
    let mut dry_run = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            other => {
                eprintln!("noct publish: unknown flag `{other}` (usage: noct publish --dry-run)");
                return 1;
            }
        }
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

    if let Some(text) = lock_text.as_deref() {
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
    }

    println!("ready to publish (dry run — nothing was uploaded)");
    0
}
