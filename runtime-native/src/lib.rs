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

use std::alloc::{alloc, alloc_zeroed, Layout};
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

/// `noctivue_rt_str_from_str(s)` — identity: return the header pointer
/// unchanged. Declared in `abi::RUNTIME_IMPORTS` for API symmetry and
/// therefore emitted as an import in every native object, so the
/// definition must exist even though no lowering calls it yet. Safe
/// (not `unsafe`): the pointer is passed through untouched, never
/// dereferenced — strings are immutable values, so no copy is needed.
#[no_mangle]
pub extern "C" fn noctivue_rt_str_from_str(s: *mut u8) -> *mut u8 {
    s
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

/// `noctivue_rt_alloc(bytes)` — zeroed record allocation backing
/// aggregate construction in native code. Headers and records are
/// never freed (leak by design, see module docs).
#[no_mangle]
pub extern "C" fn noctivue_rt_alloc(bytes: usize) -> *mut u8 {
    let layout = Layout::from_size_align(bytes.max(8), 8).expect("alloc layout");
    unsafe { alloc_zeroed(layout) }
}

/// `noctivue_rt_list_get(list, idx)` — bounds-checked element read
/// from a `[len, e0, e1, ...]` i64 record. Traps loudly (stderr +
/// `NOCTIVUE_TRAP_EXIT_CODE`) on out-of-bounds — same discipline as
/// element stores, never a silent garbage read.
///
/// # Safety
/// `list` must point to a readable record whose first slot holds a
/// valid element count.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_list_get(list: *const u8, idx: i64) -> i64 {
    let len = *(list as *const i64);
    if idx < 0 || idx >= len {
        eprintln!("noctivue: runtime error: list index out of bounds");
        std::process::exit(NOCTIVUE_TRAP_EXIT_CODE);
    }
    *(list as *const i64).add(1 + idx as usize)
}

/// `noctivue_rt_strlen(ptr)` — NUL scan for the FFI round trip
/// (Phase 3 exit criterion). No platform `libc` dependency: "scan
/// for NUL" is the whole implementation. Null header/data pointer
/// yields 0; interior NULs stop the scan (C semantics). Noctivue
/// strings are UTF-8 and NOT necessarily null-terminated (FFI.md
/// §7) — the FFI boundary pays the NUL cost, never these helpers.
///
/// # Safety
/// Beyond the null cases above, `ptr` must be a header whose `data`
/// points to at least one readable byte.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_strlen(ptr: *const u8) -> i64 {
    if ptr.is_null() {
        return 0;
    }
    let data = *(ptr as *const *const u8);
    if data.is_null() {
        return 0;
    }
    let mut len: i64 = 0;
    while *data.add(len as usize) != 0 {
        len += 1;
    }
    len
}

// ── Phase 5/M4 host-IO scalar builtins ─────────────────────────────────────
// One `extern "C"` entry per scalar-shaped interpreter builtin (see
// `compiler/src/nir/instr.rs`'s Host I/O section): blocking sleep,
// file existence, stdout writes, process-environment mutation,
// stderr log lines, and the Unit-shaped http-server route table ops.
// Each mirrors `interp/src/lib.rs`'s `eval_builtin` arm exactly
// (messages, byte counts, blocking behavior); the interpreter is the
// oracle. Result/Option/List-returning builtins (db_*, fs_read/write,
// env_get, http_send, http_server_listen, …) have no native value
// representation yet (Step 3 boundary) and stay loud `UnsupportedInstr`
// failures in the Cranelift backend — never stubs here.

/// `noctivue_rt_sleep_ms(ms: I64)` — block the calling thread for `ms`
/// milliseconds. Traps loudly on negative input with the interpreter's
/// exact message (the zero-check + message live here, like the checked
/// division helpers own theirs).
#[no_mangle]
pub extern "C" fn noctivue_rt_sleep_ms(ms: i64) {
    if ms < 0 {
        unsafe {
            let msg = alloc_str(
                "sleep_builtin requires a non-negative Int (milliseconds)".as_bytes(),
            );
            noctivue_rt_panic(msg, NOCTIVUE_TRAP_EXIT_CODE);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
}

/// `noctivue_rt_fs_exists(path: I64) -> I8` (`0` = false) — mirrors
/// the interpreter's `fs_exists` (nonexistent paths are `false`, never
/// an error).
///
/// # Safety
/// `path` must be a header pointer produced by this module's constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_fs_exists(path: *const u8) -> u8 {
    let p = read_str(path);
    u8::from(std::path::Path::new(p).exists())
}

/// `noctivue_rt_io_write(str: I64)` — stdout, no trailing newline
/// (matches the interpreter's `print!`-without-newline).
///
/// # Safety
/// `str` must be a header pointer produced by this module's constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_io_write(str_header: *const u8) {
    let s = read_str(str_header);
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(s.as_bytes());
    let _ = lock.flush();
}

/// `noctivue_rt_io_writeln(str: I64)` — stdout plus a newline (matches
/// the interpreter's `println!`).
///
/// # Safety
/// `str` must be a header pointer produced by this module's constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_io_writeln(str_header: *const u8) {
    let s = read_str(str_header);
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(s.as_bytes());
    let _ = lock.write_all(b"\n");
    let _ = lock.flush();
}

/// `noctivue_rt_env_set(name: I64, value: I64)` — set a process
/// environment variable (mirrors the interpreter's `env_set_builtin`).
///
/// # Safety
/// Both pointers must be header pointers produced by this module's
/// constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_env_set(name: *const u8, value: *const u8) {
    let n = read_str(name).to_owned();
    let v = read_str(value).to_owned();
    unsafe { std::env::set_var(n, v) };
}

/// `noctivue_rt_log_emit(level: I64, message: I64)` — one
/// `[LEVEL] message` line to stderr, no timestamp (deterministic
/// output — mirrors the interpreter's `log_emit_builtin` exactly; the
/// level gate lives in `log/log.nv`, this pipe stays dumb).
///
/// # Safety
/// Both pointers must be header pointers produced by this module's
/// constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_log_emit(level: *const u8, message: *const u8) {
    let l = read_str(level).to_owned();
    let m = read_str(message).to_owned();
    eprintln!("[{}] {}", l.to_ascii_uppercase(), m);
}

// Native http-server route table. `http_server_listen` is VM-only
// (its `Result` return has no native representation yet), so no
// listener can exist in this process — but `register`/`shutdown` are
// unconditionally `Unit` on every path (unknown handles are ignored,
// never errors), and this table keeps that behavior structural
// instead of stubbed: entries would serve nothing today (serving
// needs callbacks into compiled program code, which is why
// `serve_loop` stays a loud `UnsupportedInstr`), yet the observable
// contract — register-then-shutdown is silent `Unit` — holds exactly.
// Stored routes are never read today (serving needs program-code
// callbacks, which is why `serve_loop` stays a loud `UnsupportedInstr`)
// — the shape is kept so `register` stays structural, not stubbed.
#[allow(dead_code)]
struct NativeRoute {
    method: String,
    path_pattern: String,
    handler: String,
}

struct NativeServerState {
    routes: Vec<NativeRoute>,
    shutdown: bool,
}

static NATIVE_SERVERS: std::sync::Mutex<Option<std::collections::HashMap<i64, NativeServerState>>> =
    std::sync::Mutex::new(None);

fn native_servers(
) -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<i64, NativeServerState>>> {
    NATIVE_SERVERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `noctivue_rt_http_server_register(server: I64, method: I64,
/// path: I64, handler: I64)` — record a route on a known server;
/// unknown servers are ignored (mirrors the interpreter exactly).
///
/// # Safety
/// The three string pointers must be header pointers produced by this
/// module's constructors.
#[no_mangle]
pub unsafe extern "C" fn noctivue_rt_http_server_register(
    server: i64,
    method: *const u8,
    path: *const u8,
    handler: *const u8,
) {
    let m = read_str(method).to_owned();
    let p = read_str(path).to_owned();
    let h = read_str(handler).to_owned();
    if let Some(table) = native_servers().as_mut() {
        if let Some(state) = table.get_mut(&server) {
            state.routes.push(NativeRoute {
                method: m,
                path_pattern: p,
                handler: h,
            });
        }
    }
}

/// `noctivue_rt_http_server_shutdown(server: I64)` — flag the server
/// stopped and drop it from the table (unknown servers are a silent
/// no-op, mirroring the interpreter exactly).
#[no_mangle]
pub extern "C" fn noctivue_rt_http_server_shutdown(server: i64) {
    if let Some(table) = native_servers().as_mut() {
        if let Some(state) = table.get_mut(&server) {
            state.shutdown = true;
        }
        table.remove(&server);
    }
}

/// `noctivue_dashboard_echo(str: I64) -> I64` — echo a string back
/// through the FFI boundary. Used by the dashboard UI for round-trip
/// string exposure.
#[no_mangle]
pub extern "C" fn noctivue_dashboard_echo(str: i64) -> i64 {
    str
}

/// `noctivue_dashboard_arith(a: I64, b: I64) -> I64` — add two i64 values
/// through the FFI boundary. Used by the dashboard UI for arithmetic
/// exposure.
#[no_mangle]
pub extern "C" fn noctivue_dashboard_arith(a: i64, b: i64) -> i64 {
    a.wrapping_add(b)
}

/// Native runtime helpers for [+String]/[+Float]/[+Char] twins.
///
/// Each returns `1` when the relation holds, `0` otherwise; the Cranelift
/// backend lowers the corresponding NIR instruction to a call of the
/// matching symbol.
#[no_mangle]
pub extern "C" fn noctivue_rt_string_eq(a: i64, b: i64) -> i64 {
    if a == b { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn noctivue_rt_float_le(a: f64, b: f64) -> i64 {
    if a <= b { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn noctivue_rt_char_eq(a: i64, b: i64) -> i64 {
    if a == b { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn noctivue_rt_char_lt(a: i64, b: i64) -> i64 {
    if a < b { 1 } else { 0 }
}

// ── Phase 5: net.http FFI stubs ──────────────────────────────────────────────
//
// These are identity/passthrough stubs that allow the .nv stdlib to call
// them without linker errors. Real network I/O is implemented in later
// Phase 5 iterations; the symbols exist so the API surface is wired.
// Each takes a String header pointer (I64 per abi.rs contract) and
// returns an I64 (String header) representing the result or empty error string.

/// `noctivue_rt_http_get(url: I64) -> I64` — placeholder; returns empty string.
/// Real implementation will perform a DNS lookup + TCP connect + HTTP/1.1 request.
#[no_mangle]
pub extern "C" fn noctivue_rt_http_get(_url: i64) -> i64 {
    0
}

/// `noctivue_rt_http_post(url: I64, body: I64) -> I64` — placeholder.
#[no_mangle]
pub extern "C" fn noctivue_rt_http_post(_url: i64, _body: i64) -> i64 {
    0
}

/// `noctivue_rt_http_server_start(port: I64) -> I64` — returns opaque server handle or 0 on failure.
#[no_mangle]
pub extern "C" fn noctivue_rt_http_server_start(_port: i64) -> i64 {
    0
}

/// `noctivue_rt_http_server_serve(server: I64)` — blocks running the server event loop.
#[no_mangle]
pub extern "C" fn noctivue_rt_http_server_serve(_server: i64) {
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

    #[test]
    fn str_from_str_is_identity() {
        unsafe {
            let h = noctivue_rt_str_from_parts(b"hello".as_ptr(), 5);
            assert_eq!(noctivue_rt_str_from_str(h), h);
            assert_eq!(read_str(noctivue_rt_str_from_str(h)), "hello");
        }
    }

    #[test]
    fn alloc_and_list_get_round_trip() {
        unsafe {
            let rec = noctivue_rt_alloc(24);
            assert!(!rec.is_null());
            // Zeroed: fresh slots read back 0 without any store.
            assert_eq!(*(rec as *const i64), 0);
            // Shape a 2-list manually: [len=2][10][20].
            *(rec as *mut i64) = 2;
            *(rec as *mut i64).add(1) = 10;
            *(rec as *mut i64).add(2) = 20;
            assert_eq!(noctivue_rt_list_get(rec, 0), 10);
            assert_eq!(noctivue_rt_list_get(rec, 1), 20);
        }
    }

    #[test]
    fn strlen_round_trip() {
        // Phase 3 FFI round-trip proof: a header pointer wrapping the
        // byte address of a C string returns the correct strlen.
        // Lengths INCLUDE the terminator: `from_parts` copies exactly
        // `len` bytes and never appends a NUL, so the caller's bytes
        // must carry it (Noctivue strings are UTF-8, not necessarily
        // null-terminated — FFI.md §7). Anything else reads past the
        // buffer (flaky by construction, never do it).
        unsafe {
            let c_hello = [b'h', b'e', b'l', b'l', b'o', 0u8];
            let header = noctivue_rt_str_from_parts(c_hello.as_ptr(), 6);
            assert_eq!(noctivue_rt_strlen(header), 5);
            let c_empty = [0u8];
            let empty = noctivue_rt_str_from_parts(c_empty.as_ptr(), 0);
            assert_eq!(noctivue_rt_strlen(empty), 0);
            // Interior NULs stop the scan (C semantics).
            let c_with_nul = [b'a', b'b', b'c', 0u8, b'd', b'e', b'f', 0u8];
            let with_nul = noctivue_rt_str_from_parts(c_with_nul.as_ptr(), 8);
            assert_eq!(noctivue_rt_strlen(with_nul), 3);
        }
    }
}
