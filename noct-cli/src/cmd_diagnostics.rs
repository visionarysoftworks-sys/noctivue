//! `noct diagnostics` — dump compiler diagnostics for a `.nv` file as JSON.
//!
//! [Phase 1] AI-native compiler interface (AI_TOOLING.md §2, ADR-011).
//! Runs the full compiler frontend (lex → parse → resolve → typecheck) and
//! emits all diagnostics as structured JSON to stdout, rather than human-
//! readable text.
//!
//! This command is built alongside each pipeline stage (not retrofitted
//! afterward) per IMPLEMENTATION_PLAN.md §3 and COMPILER_ARCHITECTURE.md §6.
//!
//! Usage:
//!   noct diagnostics [file.nv]
//!   noct diagnostics --json [file.nv]   # JSON (primary use case)
//!
//! JSON schema (illustrative — will be stabilised in Phase 1):
//! ```json
//! {
//!   "diagnostics": [
//!     {
//!       "severity": "error",
//!       "code": "E0001",
//!       "message": "declaration classification cycle: `Foo`",
//!       "labels": [
//!         { "span": { "start": 0, "end": 10 }, "message": "cycle detected here" }
//!       ],
//!       "notes": []
//!     }
//!   ]
//! }
//! ```

use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // TODO (Phase 1): implement `noct diagnostics --json`.
    let mut sink = DiagnosticSink::new();
    let _ = (args, &mut sink);
    eprintln!("noct diagnostics: not yet implemented (Phase 1)");
    1
}
