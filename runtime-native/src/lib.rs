//! Native-mode runtime, Phase 3 Step 1 minimal surface.
//!
//! Every function here is `extern "C"` and `#[no_mangle]`: these are the
//! only symbols Cranelift-compiled Noctivue code is allowed to call into
//! directly (see `compiler/src/backends/cranelift/abi.rs`). Anything not
//! listed here does not exist yet — Step 4 (FFI.md) grows this file, it
//! does not replace it.
//!
//! ## String model
//!
//! A Noctivue `String` crosses this boundary, and lives in compiled code,
//! as a single pointer to a heap header:
//!
//! ```text
//! struct NvStr { data: *const u8, len: usize }   // 16 bytes
//! ```
//!
//! Single-pointer (not `(ptr, len)` pairs) is deliberate: NIR values map
//! 1:1 onto Cranelift values, so a two-word string representation would
//! need multi-value plumbing through every Phi, merge, call, and return
//! in both the Cranelift backend and any future backend. One pointer
//! keeps all of that untouched. This matches FFI.md §7's statement that
//! Noctivue `String` is UTF-8 and "not necessarily null-terminated" — a
//! real C-string boundary (for `strlen` et al.) is Step 4's concern and
//! goes through explicit conversion functions, not through these helpers.
//!
//! Allocation here uses the Rust global allocator via `alloc`/`Layout`
//! (this crate links std). Headers and their byte buffers are currently
//! never freed — a leak by design for Step 1, recorded honestly: real
//! lifetime management for native-mode strings arrives with the borrow
//! checker (Step 3) and the freestanding profile (M6), not here.

use std::alloc::{alloc, Layout};
use std::io::Write;
use std::slice;
use std::str;

/// Layout of the 16-byte string header: `(data: *const u8, len: usize)`.
fn header_layout() -> Layout {
    Layout::from_size_align(16, 8).expect("NvStr header layout")
}

/// Allocate a header pointing at `bytes` (which are copied into a fresh
/// heap buffer owned by the header). Returns the header pointer.
unsafe fn alloc_str(bytes: &[u8]) -> *mut u8 {
    let header = alloc(header_layout()) as *mut (*const u8, usize);
    let buf = if bytes.is_empty() {
        std::ptr::null()
    } else {
        let layout =
            Layout::from_size_align(bytes.len(), 1).expect("string buffer layout");
        let dst = alloc(layout);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        dst as *const u8
    };
    (*header).0 = buf;
    (*header).1 = bytes.len();
    header as *mut u8
}

/// Read `(data, len)` back out of a header pointer. Null header yields
/// empty; malformed UTF-8 yields a placeholder (a bad header means a
/// compiler bug upstream, not a user error — defensive fallback only).
unsafe fn read_str(header: *const u8) -> &'static str {
    if header.is_null() {
        return "";
    }
    let data = *(header as *const *const u8);
    let len = *(header as *const usize).add(1);
    if data.is_null() || len == 0 {
        return "";
    }
    let bytes = slice::from_raw_parts(data, len);
    str::from_utf8(bytes).unwrap_or("<invalid utf8>")
}

/// `noctivue_rt_print(str)` — write a string to stdout, no trailing
/// newline (matches the VM's `print!("{}", v)`; `println` is a separate,
/// not-yet-lowered builtin per NIR.md §6).
///
/// # Safety
/// `str` must be a header pointer produced by this module's constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_print(str_header: *const u8) {
    let s = read_str(str_header);
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(s.as_bytes());
    let _ = lock.flush();
}

/// `noctivue_rt_str_from_parts(ptr, len)` — wrap a UTF-8 byte range
/// (typically `.rodata` string-literal bytes emitted by codegen) in a
/// fresh header. Used for every string literal.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_str_from_parts(ptr: *const u8, len: usize) -> *mut u8 {
    if ptr.is_null() || len == 0 {
        return alloc_str(b"");
    }
    alloc_str(slice::from_raw_parts(ptr, len))
}

/// `noctivue_rt_str_from_int(v)` — decimal rendering, matching Rust's
/// `{}` (and therefore the VM's `to_string`) for integers.
#[no_mangle]
pub extern "C" fn noctivue_rt_str_from_int(v: i64) -> *mut u8 {
    unsafe { alloc_str(v.to_string().as_bytes()) }
}

/// `noctivue_rt_str_from_float(v)` — decimal rendering via `{}`.
#[no_mangle]
pub extern "C" fn noctivue_rt_str_from_float(v: f64) -> *mut u8 {
    unsafe { alloc_str(v.to_string().as_bytes()) }
}

/// `noctivue_rt_str_from_bool(v)` — `v != 0` renders `"true"`, else `"false"`.
#[no_mangle]
pub extern "C" fn noctivue_rt_str_from_bool(v: u8) -> *mut u8 {
    unsafe { alloc_str(if v != 0 { b"true" } else { b"false" }) }
}

/// `noctivue_rt_str_concat(a, b)` — fresh header holding `a` followed by `b`.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_str_concat(a: *const u8, b: *const u8) -> *mut u8 {
    let sa = read_str(a);
    let sb = read_str(b);
    let mut buf = Vec::with_capacity(sa.len() + sb.len());
    buf.extend_from_slice(sa.as_bytes());
    buf.extend_from_slice(sb.as_bytes());
    alloc_str(&buf)
}

/// The one exit code every native-mode runtime trap uses. Decided
/// (IMPLEMENTATION_PLAN.md Phase 3, Step 1/2 boundary): every runtime
/// trap on every backend exits with the SAME code regardless of trap
/// kind — this matches the ALREADY-SHIPPED behavior of both other
/// backends (every `VmError` variant collapses to exit 1 in
/// `cmd_run_vm.rs`; every `RuntimeError` variant collapses to exit 1 in
/// `interp/src/lib.rs`'s `run`). There is no precedent anywhere in this
/// codebase for a differentiated exit code per trap kind, so native
/// does not invent one. Every future trap call site (Step 2's
/// `try_unwrap`-on-None/Err, `unreachable`, etc.) MUST pass this
/// constant, never a literal. Revisiting this is a new three-way
/// contract requiring a DECISIONS.md entry and changes to all three
/// backends together, not a native-only choice.
pub const NOCTIVUE_TRAP_EXIT_CODE: i32 = 1;

/// `noctivue_rt_panic(str, exit_code)` — print a message to stderr and
/// terminate with `exit_code`. The native backend's equivalent of the
/// VM's `Err(VmError::...)` path (NIR.md §4.1 item 3's "no silent
/// recovery" discipline). `exit_code` should be
/// `NOCTIVUE_TRAP_EXIT_CODE` for every current and future call site —
/// see that constant's doc comment.
///
/// # Safety
/// `str` must be a header pointer produced by this module's constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_panic(str_header: *const u8, exit_code: i32) -> ! {
    let s = read_str(str_header);
    if s.is_empty() {
        eprintln!("noctivue: runtime error");
    } else {
        eprintln!("noctivue: runtime error: {s}");
    }
    std::process::exit(exit_code);
}

/// `noctivue_rt_div_by_zero() -> !` — zero-argument convenience trap for
/// the Cranelift `Div`/`Rem` lowering's pre-check (Step 1d). Kept
/// separate from `noctivue_rt_panic` so the Cranelift lowering site
/// doesn't need to materialize a message string just to trap — the
/// message is owned here, once, in Rust source (easier to keep in sync
/// with the VM's `VmError::DivisionByZero` `Display` text than
/// duplicating the string at every Cranelift call site).
///
/// Exit code is `NOCTIVUE_TRAP_EXIT_CODE` (`1`), matching `noct run-vm`'s
/// trap path (`cmd_run_vm.rs` returns 1 on `VmError`). Stderr prefixes
/// intentionally differ by backend (`VM error:` vs `noctivue: runtime
/// error:`) — the differential harness normalizes the prefix and asserts
/// on trap class + exit code.
#[no_mangle]
pub extern "C" fn noctivue_rt_div_by_zero() -> ! {
    eprintln!("noctivue: runtime error: division by zero");
    std::process::exit(NOCTIVUE_TRAP_EXIT_CODE);
}

/// Checked integer division for native code: the Cranelift backend has
/// no equivalent of the VM's `div_values` zero-check, and a raw `sdiv`
/// by zero traps the process with no Noctivue-level diagnostic. Routing
/// through here keeps the zero-check + message + exit code in one
/// place, owned by Rust source.
///
/// Overflow (`i64::MIN / -1`): Rust's `/` wraps in release, panics in
/// debug — same profile-dependent behavior as the VM's own `a / b`, so
/// native and VM agree under any given profile. Not silent either way.
#[no_mangle]
pub extern "C" fn noctivue_rt_checked_sdiv(a: i64, b: i64) -> i64 {
    if b == 0 {
        noctivue_rt_div_by_zero();
    }
    a.wrapping_div(b)
}

/// Checked unsigned division. Same trap discipline as `checked_sdiv`.
#[no_mangle]
pub extern "C" fn noctivue_rt_checked_udiv(a: u64, b: u64) -> u64 {
    if b == 0 {
        noctivue_rt_div_by_zero();
    }
    a / b
}

/// Checked signed remainder.
///
/// Honest asymmetry note: the VM's `rem_values` does NOT check for zero
/// — `a % 0` panics inside the VM via Rust's own `%` operator (loud, but
/// a Rust panic message, not `VmError::DivisionByZero`). Native routes
/// through the same division-by-zero trap as the other checked ops, so
/// both backends fail loudly on `% 0` with exit code 1, but their stderr
/// text differs. The differential harness treats this as "same trap
/// class", not byte-identical stderr.
#[no_mangle]
pub extern "C" fn noctivue_rt_checked_srem(a: i64, b: i64) -> i64 {
    if b == 0 {
        noctivue_rt_div_by_zero();
    }
    a.wrapping_rem(b)
}

/// Checked unsigned remainder. Same trap discipline as `checked_srem`.
#[no_mangle]
pub extern "C" fn noctivue_rt_checked_urem(a: u64, b: u64) -> u64 {
    if b == 0 {
        noctivue_rt_div_by_zero();
    }
    a % b
}

/// Float remainder: Cranelift IR has no `frem` instruction, so `%` on
/// floats routes through here. Never traps (matches the VM's
/// `a % b` on floats, including NaN/inf propagation via Rust semantics).
#[no_mangle]
pub extern "C" fn noctivue_rt_frem(a: f64, b: f64) -> f64 {
    a % b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip() {
        unsafe {
            let h = noctivue_rt_str_from_parts(b"hello".as_ptr(), 5);
            assert_eq!(read_str(h), "hello");
            let n = noctivue_rt_str_from_int(-42);
            assert_eq!(read_str(n), "-42");
            let t = noctivue_rt_str_from_bool(1);
            assert_eq!(read_str(t), "true");
            let c = noctivue_rt_str_concat(h, n);
            assert_eq!(read_str(c), "hello-42");
        }
    }

    #[test]
    fn print_handles_null_safely() {
        unsafe {
            noctivue_rt_print(std::ptr::null());
        }
    }
}
