//! `noct run` — build and execute a `.nv` program via the tree-walking interpreter.
//!
//! [Phase 1] Pipeline: lex → parse → resolve → typecheck → interpret.
//!
//! This command is the primary Phase 1 user-facing entry point. It runs the
//! full compiler frontend and then hands the resulting HIR to the interpreter
//! (`interp` crate). No native code generation happens here.
//!
//! Usage:
//!   noct run [file.nv]
//!   noct run          # runs main.nv in the current package

use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // TODO (Phase 1): implement `noct run`.
    //
    // Steps:
    // 1. Resolve the target .nv file from args or current-directory convention.
    // 2. Read the source file.
    // 3. Lex → parse → resolve → typecheck.
    // 4. If any errors in sink, print diagnostics and return 1.
    // 5. Run interpreter; return its exit code.
    let mut sink = DiagnosticSink::new();
    let _ = (args, &mut sink);
    eprintln!("noct run: not yet implemented (Phase 1)");
    1
}
