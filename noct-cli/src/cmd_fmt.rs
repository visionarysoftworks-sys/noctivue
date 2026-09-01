//! `noct fmt` — run the official Noctivue formatter.
//!
//! [Phase 4 / M3] The formatter is normative: it MUST be able to losslessly
//! transform between the density levels and block forms defined in
//! SYNTAX.md §§3, 6–7 (ADR-008). Semicolons are a formatting decision the
//! formatter owns (DECISIONS.md Issue 5).
//!
//! Usage:
//!   noct fmt [file.nv | --check]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("noct fmt: not yet implemented (Phase 4 — formatter / M3)");
    1
}
