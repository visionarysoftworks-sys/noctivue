//! Extern-"C" runtime boundary for the Cranelift backend.
//!
//! The backend never emits host I/O, allocation, or trapping itself: every
//! such operation lowers to an ordinary call of one of the symbols below,
//! all defined in `runtime-native` (Phase 3, Step 1 pulls this forward
//! minimally; the full runtime surface is Step 4).
//!
//! ## String model
//!
//! A Noctivue `String` is a single `i64` holding a pointer to a 16-byte
//! heap header `(data: *const u8, len: usize)` — see
//! `runtime-native/src/lib.rs`. Single-pointer (not `(ptr, len)` pairs)
//! is what keeps NIR's `ValueId -> exactly one Cranelift Value` mapping
//! intact through Phis, merges, calls, and returns. Every signature below
//! that mentions "str" means one `I64` header pointer.

/// `noctivue_rt_print(str: I64)`.
pub const NOCTIVUE_RT_PRINT: &str = "noctivue_rt_print";

/// `noctivue_rt_panic(str: I64, exit_code: I32) -> !`.
pub const NOCTIVUE_RT_PANIC: &str = "noctivue_rt_panic";

/// `noctivue_rt_str_from_parts(ptr: I64, len: I64) -> I64` — wrap
/// `.rodata` literal bytes in a fresh header.
pub const NOCTIVUE_RT_STR_FROM_PARTS: &str = "noctivue_rt_str_from_parts";

/// `noctivue_rt_str_from_int(v: I64) -> I64`.
pub const NOCTIVUE_RT_STR_FROM_INT: &str = "noctivue_rt_str_from_int";

/// `noctivue_rt_str_from_float(v: F64) -> I64`.
pub const NOCTIVUE_RT_STR_FROM_FLOAT: &str = "noctivue_rt_str_from_float";

/// `noctivue_rt_str_from_bool(v: I8) -> I64` (`0` = false).
pub const NOCTIVUE_RT_STR_FROM_BOOL: &str = "noctivue_rt_str_from_bool";

/// `noctivue_rt_str_concat(a: I64, b: I64) -> I64`.
pub const NOCTIVUE_RT_STR_CONCAT: &str = "noctivue_rt_str_concat";

/// `noctivue_rt_checked_sdiv(a: I64, b: I64) -> I64` — traps loudly on
/// zero instead of emitting a bare `sdiv` (see `lower.rs`'s Div arm).
pub const NOCTIVUE_RT_CHECKED_SDIV: &str = "noctivue_rt_checked_sdiv";

/// `noctivue_rt_checked_udiv(a: I64, b: I64) -> I64`.
pub const NOCTIVUE_RT_CHECKED_UDIV: &str = "noctivue_rt_checked_udiv";

/// `noctivue_rt_checked_srem(a: I64, b: I64) -> I64`.
pub const NOCTIVUE_RT_CHECKED_SREM: &str = "noctivue_rt_checked_srem";

/// `noctivue_rt_checked_urem(a: I64, b: I64) -> I64`.
pub const NOCTIVUE_RT_CHECKED_UREM: &str = "noctivue_rt_checked_urem";

/// `noctivue_rt_frem(a: F64, b: F64) -> F64` — Cranelift IR has no float
/// remainder instruction, so `%` on floats calls out (never traps,
/// matching the VM).
pub const NOCTIVUE_RT_FREM: &str = "noctivue_rt_frem";

/// Phase 5: net.http FFI stubs.
pub const NOCTIVUE_RT_HTTP_GET: &str = "noctivue_rt_http_get";
pub const NOCTIVUE_RT_HTTP_POST: &str = "noctivue_rt_http_post";
pub const NOCTIVUE_RT_HTTP_SERVER_START: &str = "noctivue_rt_http_server_start";
pub const NOCTIVUE_RT_HTTP_SERVER_SERVE: &str = "noctivue_rt_http_server_serve";

use cranelift_codegen::ir::types as clif_types;
use cranelift_codegen::ir::Type as ClifType;

/// Single source of truth for every runtime import: `(symbol, params,
/// returns)`. The driver declares exactly this table, in this order, so
/// adding a runtime function is one row here plus one field on each of
/// the driver's `RtIds` / lowerer's `RtRefs` structs — the compiler
/// rejects a mismatch (no silent drift between declaration and use).
pub const RUNTIME_IMPORTS: &[(&str, &[ClifType], &[ClifType])] = &[
    (NOCTIVUE_RT_PRINT, &[clif_types::I64], &[]),
    (
        NOCTIVUE_RT_PANIC,
        &[clif_types::I64, clif_types::I32],
        &[],
    ),
    (
        NOCTIVUE_RT_STR_FROM_PARTS,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_STR_FROM_INT,
        &[clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_STR_FROM_FLOAT,
        &[clif_types::F64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_STR_FROM_BOOL,
        &[clif_types::I8],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_STR_CONCAT,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_CHECKED_SDIV,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_CHECKED_UDIV,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_CHECKED_SREM,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_CHECKED_UREM,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_FREM,
        &[clif_types::F64, clif_types::F64],
        &[clif_types::F64],
    ),
    // Phase 5: net.http FFI stubs (string-header pointers, I64 contract).
    (
        NOCTIVUE_RT_HTTP_GET,
        &[clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_HTTP_POST,
        &[clif_types::I64, clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_HTTP_SERVER_START,
        &[clif_types::I64],
        &[clif_types::I64],
    ),
    (
        NOCTIVUE_RT_HTTP_SERVER_SERVE,
        &[clif_types::I64],
        &[],
    ),
];
