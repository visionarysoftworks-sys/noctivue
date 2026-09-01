//! `noct lint` — run the official Noctivue linter.
//!
//! [Phase 4 / M3] Default-on rules enforce things the grammar can't (unused
//! imports, unreachable match arms beyond exhaustiveness, etc.).
//! Style-preference rules (e.g., "prefer concise over explicit declaration
//! style") ship **default-off** (DECISIONS.md Issue 7, TOOLCHAIN.md §5).
//!
//! Usage:
//!   noct lint [file.nv | --fix]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("noct lint: not yet implemented (Phase 4 — linter / M3)");
    1
}
