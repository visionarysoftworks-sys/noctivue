//! Noctivue tree-walking interpreter — the Phase 1 / M0 execution engine.
//!
//! ## Role
//!
//! This crate is the M0 deliverable: it takes a type-checked HIR
//! ([`compiler::hir::Module`]) and executes it by walking the tree directly,
//! without lowering to NIR or native code.
//!
//! Keeping the interpreter as a **separate crate** from `compiler/` is
//! intentional (SCAFFOLD.md §3): it ensures M0 remains a runnable, demoable
//! artifact (lex → parse → resolve → typecheck → interpret) without depending
//! on NIR or backends that do not exist yet.
//!
//! When M1 introduces NIR (`compiler/src/nir/`), this crate either:
//! - Gets replaced by a NIR-based VM crate, or
//! - Is kept as a reference implementation for differential testing.
//! That decision is **Open** (SCAFFOLD.md §3, IMPLEMENTATION_PLAN.md §4).
//!
//! ## Pipeline position
//!
//! ```text
//! compiler::lexer  →  compiler::parser  →  compiler::resolver
//!   →  compiler::typeck  →  compiler::hir  →  interp::Interpreter
//! ```
//!
//! ## Phase 1 scope
//!
//! Covers the non-UI subset of `examples/dashboard.nv`:
//! structs, functions, `enum`, `match`, `Result`/`Option`, `?`.
//!
//! Explicitly excluded: UI, async/await, ARC/managed mode, native codegen,
//! macros (ROADMAP.md §1, IMPLEMENTATION_PLAN.md §3).

use compiler::diagnostics::DiagnosticSink;
use compiler::hir::Module;

/// The tree-walking interpreter.
///
/// Holds any interpreter state needed during a single program execution
/// (global value environment, call stack, etc.).
pub struct Interpreter {
    // TODO (Phase 1): add execution state (value environment, call stack, …)
}

impl Interpreter {
    /// Create a new interpreter instance.
    pub fn new() -> Self {
        Interpreter {}
    }

    /// Execute a type-checked [`Module`], returning the program's exit code.
    ///
    /// Errors are emitted into `sink`. The interpreter continues after
    /// recoverable runtime errors where possible to surface as many issues as
    /// it can in a single run.
    pub fn run(&mut self, module: &Module, sink: &mut DiagnosticSink) -> i32 {
        // TODO (Phase 1): implement tree-walking evaluation.
        let _ = (module, sink);
        0
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}
