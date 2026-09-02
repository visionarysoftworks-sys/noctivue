//! NIR (Noctivue Intermediate Representation) — the typed, SSA-based IR.
//!
//! This module implements the concrete instruction set specified in NIR.md §4.
//! It is the single IR used for both native (Cranelift) and VM backends,
//! with mode tags (`native` / `managed`) attached per function.

pub mod instr;
pub mod module;
pub mod lowering;
pub mod types;
pub mod vm;

pub use instr::*;
pub use module::*;
pub use lowering::*;
pub use types::*;