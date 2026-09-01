//! `noct publish` — publish a package to the Noctivue package registry.
//!
//! [Phase 4 / M3] Package signing is a **hard precondition** before the public
//! registry goes live (TOOLCHAIN.md §3), not a nice-to-have.
//!
//! Usage:
//!   noct publish [--dry-run]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("noct publish: not yet implemented (Phase 4 — registry / M3)");
    1
}
