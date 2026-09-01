//! `noct add` — add a dependency to the current package.
//!
//! [Phase 4 / M3] Resolves, downloads, and adds a dependency to the package
//! manifest and lockfile. Every dependency entry declares a trust tier
//! (ADR-015, TOOLCHAIN.md §3). Package integrity hashes MUST be in place
//! before any public registry launch.
//!
//! Usage:
//!   noct add <package>[@version]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("noct add: not yet implemented (Phase 4 — package manager / M3)");
    1
}
