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
//! ## Module layout
//!
//! - [`types`] — the [`Ty`] enum (resolved type representation).
//! - [`items`] — typed item, statement, expression definitions.

pub mod items;
pub mod types;

pub use items::{Enum, Function, Module, Struct};
pub use types::Ty;
