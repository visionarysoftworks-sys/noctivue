//! `noct outdated` — compare locked versions against a file-backed index.
//!
//! [Phase 4] Purely informational: prints one table row per locked
//! package — `(name, current, latest-satisfying, latest-available)` —
//! and exits 0 even when updates exist. Exit 1 is reserved for real
//! failures (bad flags, missing/invalid lockfile, unreadable index).
//!
//! - `current`: the version pinned in `nestpkg.lock`.
//! - `latest-satisfying`: the highest indexed version satisfying the
//!   manifest requirement for direct dependencies (`none` when the
//!   index holds no satisfying version); for transitive lock entries
//!   with no direct requirement this equals `latest-available`.
//! - `latest-available`: the highest indexed version, full stop.
//! - `path` sources print `local` (your tree — no index to compare
//!   against); `git` sources print `git`; registry packages unknown
//!   to the index print `unknown` (still exit 0).
//!
//! Usage:
//!   noct outdated --index <dir>

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(args: &[String]) -> i32 {
    let mut index_dir: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--index" {
            i += 1;
            if i >= args.len() {
                eprintln!("noct outdated: --index requires a directory (usage: noct outdated --index <dir>)");
                return 1;
            }
            index_dir = Some(args[i].clone());
        } else if let Some(p) = a.strip_prefix("--index=") {
            if p.is_empty() {
                eprintln!("noct outdated: --index requires a directory (usage: noct outdated --index <dir>)");
                return 1;
            }
            index_dir = Some(p.to_string());
        } else if a == "--help" || a == "-h" {
            print_usage();
            return 0;
        } else {
            eprintln!("noct outdated: unknown flag `{a}` (usage: noct outdated --index <dir>)");
            return 1;
        }
        i += 1;
    }
    let Some(index_dir) = index_dir else {
        eprintln!("noct outdated: --index <dir> is required (no default registry exists)");
        return 1;
    };
    if !Path::new(&index_dir).is_dir() {
        eprintln!("noct outdated: cannot read index `{index_dir}` (not a directory)");
        return 1;
    };

    let lock_text = match fs::read_to_string(Path::new("nestpkg.lock")) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("noct outdated: no nestpkg.lock in current directory (run `noct add` first)");
            return 1;
        }
    };
    let lock = match crate::manifest::parse_lockfile(&lock_text) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("noct outdated: invalid lockfile: {e}");
            return 1;
        }
    };

    // Direct requirements come from the manifest when one exists; a
    // missing manifest just means every entry is treated as transitive
    // (latest-satisfying = latest-available).
    let mut reqs: BTreeMap<String, crate::manifest::VersionReq> = BTreeMap::new();
    match fs::read_to_string(Path::new("nestpkg.nvpm")) {
        Ok(text) => match crate::manifest::parse_manifest(&text) {
            Ok(manifest) => {
                for dep in manifest.dependencies.iter().chain(manifest.dev_dependencies.iter()) {
                    reqs.insert(dep.name.clone(), dep.req);
                }
            }
            Err(e) => {
                eprintln!("noct outdated: invalid manifest: {e}");
                return 1;
            }
        },
        Err(_) => {}
    }

    let index = crate::registry::FileIndex::new(PathBuf::from(&index_dir));

    println!(
        "{:<20} {:<12} {:<18} {}",
        "name", "current", "latest-satisfying", "latest-available"
    );
    for pkg in &lock.packages {
        let current = pkg.version.to_string();
        let (satisfying, latest) = match &pkg.source {
            crate::manifest::Source::Path(_) => ("local".to_string(), "local".to_string()),
            crate::manifest::Source::Git { .. } => ("git".to_string(), "git".to_string()),
            crate::manifest::Source::Registry => {
                match crate::registry::Registry::versions(&index, &pkg.name) {
                    Err(_) => ("unknown".to_string(), "unknown".to_string()),
                    Ok(versions) => {
                        // `versions()` returns ascending order; the last
                        // element is the highest indexed version.
                        let latest = versions.last().copied().unwrap_or(pkg.version);
                        let latest_text = latest.to_string();
                        let satisfying = match reqs.get(&pkg.name) {
                            Some(req) => versions
                                .iter()
                                .rev()
                                .find(|v| crate::manifest::req_satisfied(*req, **v))
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "none".to_string()),
                            None => latest_text.clone(),
                        };
                        (satisfying, latest_text)
                    }
                }
            }
        };
        println!("{:<20} {:<12} {:<18} {}", pkg.name, current, satisfying, latest);
    }
    0
}

fn print_usage() {
    println!("usage: noct outdated --index <dir>");
    println!();
    println!("Compare locked versions against a file-backed index and print");
    println!("a table of (name, current, latest-satisfying, latest-available).");
    println!("Informational only: exits 0 even when updates exist.");
}
