//! NIR type system — typed SSA values with mode tags.

use crate::hir::types::Ty;
use std::fmt;

/// Execution mode for NIR functions and values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Native mode: ownership + borrowing, stack allocation, move semantics
    Native,
    /// Managed mode: ARC reference counting, heap allocation, cycle mitigation
    Managed,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mode::Native => write!(f, "native"),
            Mode::Managed => write!(f, "managed"),
        }
    }
}

/// A NIR type — wraps a HIR type with a mode tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NirTy {
    pub inner: Ty,
    pub mode: Mode,
}

impl NirTy {
    pub fn new(inner: Ty, mode: Mode) -> Self {
        NirTy { inner, mode }
    }

    pub fn native(ty: Ty) -> Self {
        NirTy::new(ty, Mode::Native)
    }

    pub fn managed(ty: Ty) -> Self {
        NirTy::new(ty, Mode::Managed)
    }
}

impl fmt::Display for NirTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.inner, self.mode)
    }
}

/// NIR value identifier (SSA register).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

/// NIR block identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block{}", self.0)
    }
}

/// NIR function identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

impl fmt::Display for FuncId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@func{}", self.0)
    }
}

/// Function signature in NIR.
#[derive(Debug, Clone)]
pub struct FuncSig {
    pub params: Vec<NirTy>,
    pub ret: NirTy,
    pub mode: Mode,
}

impl FuncSig {
    pub fn new(params: Vec<NirTy>, ret: NirTy, mode: Mode) -> Self {
        FuncSig { params, ret, mode }
    }
}