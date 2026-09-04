//! Extern-"C" runtime boundary for the Cranelift backend.
//!
//! The backend never emits host I/O itself: `Instr::Print` lowers to an
//! ordinary call of this symbol, which `runtime-native` provides (Phase 3,
//! Step 1 pulls this forward minimally; the full runtime surface is Step 4).

/// Symbol name of the host print routine the backend imports.
pub const NOCTIVUE_RT_PRINT: &str = "noctivue_rt_print";
