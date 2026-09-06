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
//!   noct audit

use std::fs;
use std::path::Path;

pub fn run(args: &[String]) -> i32 {
    for arg in args {
        eprintln!("noct audit: unknown flag `{arg}` (usage: noct audit)");
        return 1;
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
