//! `noct build` — compile a Noctivue package to a native binary.
//!
//! [Phase 3 / M2] Requires the Cranelift backend (`compiler/src/backends/cranelift/`),
//! native runtime (`runtime-native/`), and ownership/borrow enforcement.
//! Not available until Phase 3 exit criteria are met (IMPLEMENTATION_PLAN.md §5).
//!
//! Usage:
//!   noct build [--release]

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("noct build: not yet implemented (Phase 3 — native compilation via Cranelift)");
    1
}
