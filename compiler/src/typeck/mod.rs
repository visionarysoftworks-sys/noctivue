//! Type checker — produces a typed AST / HIR.
//!
//! Responsibilities (COMPILER_ARCHITECTURE.md §3):
//! - **Local type inference** within function bodies (TYPE_SYSTEM.md §13).
//!   Every function signature MUST be fully annotated; inference does not cross
//!   function boundaries (whole-program inference is explicitly out of scope).
//! - Trait bound checking.
//! - `Option<T>` / `Result<T, E>` flow analysis (including `?` lowering).
//! - Mode consistency checks — detecting native/managed boundary crossings
//!   (MEMORY_MODEL.md §7, DECISIONS.md Issue 3).
//!
//! [Phase 1] This module is created in Phase 1 alongside the resolver and
//! interpreter. The type checker operates on the resolved AST produced by
//! [`crate::resolver`] and outputs a typed representation consumed by
//! [`crate::hir`] and then [`crate::interp`].

use crate::ast::Program;
use crate::diagnostics::DiagnosticSink;
use crate::hir;

/// Run the type checker over a resolved [`Program`] and produce a [`hir::Module`].
///
/// Errors are emitted into `sink`. The returned `Module` may be partial if
/// type errors were found (best-effort recovery to surface as many errors as
/// possible in a single compilation).
pub fn typecheck(program: Program, sink: &mut DiagnosticSink) -> hir::Module {
    // TODO (Phase 1): implement type checker.
    let _ = (program, sink);
    hir::Module::empty()
}
