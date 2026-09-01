//! Name resolution — binds identifiers to their definitions and classifies
//! bare declarations by body shape.
//!
//! Responsibilities (COMPILER_ARCHITECTURE.md §3):
//! - Scope construction and identifier binding.
//! - Import resolution.
//! - **Declaration-kind disambiguation** via [`classify`]:
//!   [`crate::ast::BareDecl`] nodes are classified as struct, function, or
//!   component based on body shape alone — never based on naming convention
//!   (COMPILER_ARCHITECTURE.md §4, DECISIONS.md Issue 1/2).
//!
//! ## Classification cycle detection
//!
//! The classification of some `BareDecl`s depends on what other names in the
//! same scope resolve to (the "transitive resolution order" problem,
//! COMPILER_ARCHITECTURE.md §4, EXECUTION_GUIDE.md Phase 0). If a genuine
//! cycle is detected — `A`'s classification depends on `B`'s, which depends
//! on `A`'s — the resolver MUST emit a `"declaration classification cycle"`
//! diagnostic and MUST NOT loop infinitely or silently guess.

pub mod classify;

use crate::ast::Program;
use crate::diagnostics::DiagnosticSink;

/// Run name resolution and declaration classification over an unresolved [`Program`].
///
/// Returns a new `Program` where all resolvable `BareDecl` items have been
/// replaced by their classified counterparts. Errors are emitted into `sink`.
pub fn resolve(program: Program, sink: &mut DiagnosticSink) -> Program {
    // TODO (Phase 1): implement name resolution.
    let _ = sink;
    program
}
