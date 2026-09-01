//! `noct ast` — dump the AST of a `.nv` file as JSON.
//!
//! [Phase 1] AI-native compiler interface (AI_TOOLING.md §2, ADR-011).
//! This command is part of the machine-readable surface that tools, IDE
//! extensions, and AI agents can consume without parsing human-readable text.
//!
//! Usage:
//!   noct ast [file.nv]
//!   noct ast --json [file.nv]   # JSON output (primary use case)
//!
//! The JSON schema is stabilised enough (by Phase 1 exit criteria) that a
//! second tool can consume it without fragile text parsing. See
//! IMPLEMENTATION_PLAN.md §3 and AI_TOOLING.md for the contract.

use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // TODO (Phase 1): implement `noct ast --json`.
    //
    // Steps:
    // 1. Lex + parse the target file.
    // 2. Serialise the AST to JSON.
    //    - Structured output is required from day one; retrofitting it later is expensive.
    //    - Human-readable (pretty-printed) JSON by default; compact with --compact.
    // 3. Print to stdout so callers can pipe it.
    let mut sink = DiagnosticSink::new();
    let _ = (args, &mut sink);
    eprintln!("noct ast: not yet implemented (Phase 1)");
    1
}
