//! `noct audit` — show dependency trust rows from the lockfile.
//!
//! [Phase 4 / M3] Reads trust columns that exist from day one
//! (P-003 §7): `name version tier signed_by audit-status`, computed
//! from lock data alone — no network, no re-derivation (the
//! `cxx-shim` experimental flag rides in the lock for exactly this
//! reason). The vulnerability-database column arrives in M5; until
//! then every row reports `unknown (no vuln database)`.
//!
//! Row statuses:
//! - `path` sources: `local` (your tree — nothing to verify, by design).
//! - `registry`/`git` sources: `signed` (presence of `content:` +
//!   `signed_by:`, enforced at parse; VALUES are verified at fetch,
//!   not here).
//!
//! Usage:
//!   noct audit [--advisories <file>]

use std::fs;
use std::path::Path;

pub fn run(args: &[String]) -> i32 {
    let mut advisories_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--advisories" {
            match it.next() {
                Some(value) => advisories_path = Some(value.clone()),
                None => {
                    eprintln!("noct audit: --advisories requires a file path");
                    return 1;
                }
            }
        } else if let Some(value) = arg.strip_prefix("--advisories=") {
            if value.is_empty() {
                eprintln!("noct audit: --advisories requires a file path");
                return 1;
            }
            advisories_path = Some(value.to_string());
        } else {
            eprintln!("noct audit: unknown flag `{arg}` (usage: noct audit [--advisories <file>])");
            return 1;
        }
    }

    let lock_path = Path::new("nestpkg.lock");
    let lock_text = match fs::read_to_string(lock_path) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("noct audit: no nestpkg.lock in current directory (run `noct add` first)");
            return 1;
        }
    };
    let lock = match crate::manifest::parse_lockfile(&lock_text) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("noct audit: invalid lockfile: {e}");
            return 1;
        }
    };

    let db = match advisories_path {
        None => None,
        Some(path) => match crate::advisory::load_database(Path::new(&path)) {
            Ok(db) => Some(db),
            Err(e) => {
                eprintln!("noct audit: invalid advisory database: {e}");
                return 1;
            }
        },
    };

    match db {
        None => {
            println!("{:<20} {:<10} {:<16} {:<12} {}", "name", "version", "tier", "signed_by", "audit");
            for pkg in &lock.packages {
                let tier = pkg.tier.as_str();
                let tier = if pkg.experimental {
                    format!("{tier} (experimental)")
                } else {
                    tier.to_string()
                };
                let (signed_by, status) = match &pkg.source {
                    crate::manifest::Source::Path(_) => ("-".to_string(), "local"),
                    _ => (
                        pkg.signed_by.clone().unwrap_or_else(|| "-".to_string()),
                        "signed",
                    ),
                };
                println!(
                    "{:<20} {:<10} {:<16} {:<12} {} (no vuln database)",
                    pkg.name, pkg.version, tier, signed_by, status
                );
            }
            0
        }
        Some(db) => {
            println!(
                "{:<20} {:<10} {:<16} {:<12} {} {}",
                "name", "version", "tier", "signed_by", "audit", "vulns"
            );
            for pkg in &lock.packages {
                let tier = pkg.tier.as_str();
                let tier = if pkg.experimental {
                    format!("{tier} (experimental)")
                } else {
                    tier.to_string()
                };
                let (signed_by, status) = match &pkg.source {
                    crate::manifest::Source::Path(_) => ("-".to_string(), "local"),
                    _ => (
                        pkg.signed_by.clone().unwrap_or_else(|| "-".to_string()),
                        "signed",
                    ),
                };
                let hits = db.matching(&pkg.name, pkg.version);
                let vulns = if hits.is_empty() {
                    "clean".to_string()
                } else {
                    hits.iter()
                        .map(|a| format!("{}({})", a.id, a.severity.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!(
                    "{:<20} {:<10} {:<16} {:<12} {} {}",
                    pkg.name, pkg.version, tier, signed_by, status, vulns
                );
            }
            0
        }
    }
}
