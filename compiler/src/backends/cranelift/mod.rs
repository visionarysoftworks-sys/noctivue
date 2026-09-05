//! Cranelift native backend (Phase 3, M2).
//!
//! Entry point and per-module compile loop live here once written;
//! per-category instruction lowering lives in `lower`, and the
//! extern-"C" runtime boundary in `abi`. Only the arithmetic category
//! (`lower.rs`) exists so far — control flow, aggregates, and managed
//! mode are Phase 3 follow-ups (see IMPLEMENTATION_PLAN.md §5).

pub mod abi;
pub mod driver;
pub mod lower;
