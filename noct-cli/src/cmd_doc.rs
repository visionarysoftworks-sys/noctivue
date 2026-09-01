//! `noct doc` — generate documentation from `///` doc comments.
//!
//! [Phase 4 / M3] Doc comments (`///` and `//!`) are parsed by the lexer from
//! Phase 1 onward (LANGUAGE_SPEC.md §3) and are available in the AST.
//! The `noct doc` command renders them into browsable documentation.
//!
//! Usage:
//!   noct doc [--open]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("noct doc: not yet implemented (Phase 4 — documentation / M3)");
    1
}
