//! `noct test` — run the test suite for a Noctivue package.
//!
//! [Phase 1] Discovers and runs test functions (functions annotated with a
//! test marker — exact syntax TBD) via the interpreter. Outputs pass/fail
//! counts and diagnostics for each failing test.
//!
//! Usage:
//!   noct test [filter]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // TODO (Phase 1): implement `noct test`.
    let _ = args;
    eprintln!("noct test: not yet implemented (Phase 1)");
    1
}
