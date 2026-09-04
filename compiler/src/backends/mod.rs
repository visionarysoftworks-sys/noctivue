//! Compiler backends — NIR consumers.
//!
//! `cranelift` is the Phase 3 (M2) native backend (ADR-003, first).
//! Additional backends (LLVM, per ADR-003) slot in here later behind the
//! same shape: one submodule per backend, shared types at this level if
//! and only if two backends genuinely need them.

pub mod cranelift;
