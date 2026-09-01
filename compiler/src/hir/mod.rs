//! High-level Intermediate Representation (HIR) / typed AST.
//!
//! The HIR is the output of the type checker and the primary input to the
//! tree-walking interpreter ([`interp`]) and (eventually) NIR lowering.
//!
//! It is structurally similar to the unresolved AST ([`crate::ast`]) but with:
//! - All identifiers resolved to their definitions.
//! - All `BareDecl` nodes replaced by their classified counterparts.
//! - Type annotations on every expression and binding.
//! - `?` operators desugared to explicit `Result`/`Option` handling.
//!
//! [Phase 1] This module is skeletal; it will be fleshed out alongside the type
//! checker. NIR lowering (Phase 2 / M1) will consume this representation.

/// A type-checked, resolved module (output of [`crate::typeck`]).
#[derive(Debug, Clone)]
pub struct Module {
    // TODO (Phase 1): populate with typed item definitions.
    pub items: Vec<()>,
}

impl Module {
    /// Returns an empty module (stub until the type checker is implemented).
    pub fn empty() -> Self {
        Module { items: Vec::new() }
    }
}
