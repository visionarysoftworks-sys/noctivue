//! NIR instruction set — concrete instructions per NIR.md §4.

use crate::hir::types::Ty;
use crate::nir::types::{BlockId, FuncId, NirTy, ValueId};
use std::fmt;

/// Binary arithmetic operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// Binary comparison operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Unary operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// NIR instructions — SSA-based, typed, mode-aware.
#[derive(Debug, Clone)]
pub enum Instr {
    // ── Arithmetic ──────────────────────────────────────────────────────────
    /// `%dst = add %lhs, %rhs`
    Add { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = sub %lhs, %rhs`
    Sub { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = mul %lhs, %rhs`
    Mul { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = div %lhs, %rhs`
    Div { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = rem %lhs, %rhs`
    Rem { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },

    // ── Comparison ──────────────────────────────────────────────────────────
    /// `%dst = icmp op %lhs, %rhs` (integer)
    ICmp { dst: ValueId, op: CmpOp, lhs: ValueId, rhs: ValueId },
    /// `%dst = fcmp op %lhs, %rhs` (float)
    FCmp { dst: ValueId, op: CmpOp, lhs: ValueId, rhs: ValueId },

    // ── Unary ───────────────────────────────────────────────────────────────
    /// `%dst = neg %src`
    Neg { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = not %src`
    Not { dst: ValueId, src: ValueId },

    // ── Constants ───────────────────────────────────────────────────────────
    /// `%dst = const value`
    Const { dst: ValueId, value: ConstValue, ty: NirTy },

    // ── Memory (Native) ─────────────────────────────────────────────────────
    /// `%dst = stack_alloc ty` — allocate on stack (native mode)
    StackAlloc { dst: ValueId, ty: NirTy },
    /// `%dst = load %src` — load from pointer
    Load { dst: ValueId, src: ValueId, ty: NirTy },
    /// `store %val, %ptr` — store to pointer
    Store { val: ValueId, ptr: ValueId },
    /// `%dst = move %src` — move semantics (native mode)
    Move { dst: ValueId, src: ValueId },

    // ── Memory (Managed) ────────────────────────────────────────────────────
    /// `%dst = heap_alloc %src, ty` — allocate on heap (managed mode), moving src value
    HeapAlloc { dst: ValueId, src: ValueId, ty: NirTy },
    /// `arc_retain %src` — increment refcount
    ArcRetain { src: ValueId },
    /// `arc_release %src` — decrement refcount
    ArcRelease { src: ValueId },
    /// `%dst = arc_load %src, ty` — read the value behind a strong
    /// reference. Cannot fail by construction: holding an `Arc` means the
    /// object is alive, so this differs from `unowned_load` (which traps
    /// on a dangling back-reference) and from `weak_load` (which yields
    /// `None` once the last strong reference is gone).
    ArcLoad { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = arc_id %src` — the heap identity of a strong reference, as
    /// an integer. Two `Arc`s to the same heap object report the same id,
    /// so a re-render can recognise a shared subtree without loading or
    /// comparing a single value.
    ArcId { dst: ValueId, src: ValueId },
    /// `%dst = weak_create %src, ty` — a non-owning observer of an `Arc`.
    /// Does not keep the object alive; `WeakLoad` on it yields `None`
    /// once the last strong reference is gone.
    WeakCreate { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = weak_load %src` — upgrade a weak reference to
    /// `Option<Arc>`, or `None` when the object is already freed.
    WeakLoad { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = unowned_create %src, ty` — a checked back-reference that
    /// is deliberately NOT refcounted. Its whole point is to let a
    /// child point at its parent without keeping the parent alive and
    /// without creating a strong cycle, so it must not affect the
    /// count. It trades liveness for safety: `UnownedLoad` traps if the
    /// object is gone, unlike `WeakLoad` which returns `None`.
    UnownedCreate { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = unowned_load %src, ty` — dereference an unowned
    /// back-reference, trapping when the object has been deallocated.
    UnownedLoad { dst: ValueId, src: ValueId, ty: NirTy },

    // ── Aggregates ──────────────────────────────────────────────────────────
    /// `%dst = struct_new [field0, field1, ...]` — construct struct
    StructNew { dst: ValueId, fields: Vec<ValueId>, field_names: Vec<String>, ty: NirTy },
    /// `%dst = field_get %obj, field` — get struct field
    FieldGet { dst: ValueId, obj: ValueId, field: String, ty: NirTy },
    /// `%dst = field_set %obj, field, %val` — set struct field, returns new struct
    FieldSet { dst: ValueId, obj: ValueId, field: String, val: ValueId },
    /// `%dst = list_len %src` — get list length
    ListLen { dst: ValueId, src: ValueId },
    /// `%dst = list_index %src, %index` — index into list
    ListIndex { dst: ValueId, src: ValueId, index: ValueId },
    /// `%dst = enum_tag %src` — get enum variant tag
    EnumTag { dst: ValueId, src: ValueId },
    /// `%dst = enum_payload %src[i]` — get enum payload field `i`
    /// (also extracts `Option`/`Result` payloads, which hold one value).
    EnumPayload { dst: ValueId, src: ValueId, index: u32, ty: NirTy },
    /// `%dst = enum_new %tag, [field0, field1, ...]` — construct enum variant
    EnumNew { dst: ValueId, tag: ValueId, fields: Vec<ValueId>, ty: NirTy },

    // ── Calls ───────────────────────────────────────────────────────────────
    /// `%dst = call @func(args...)` — direct call
    Call { dst: ValueId, func: FuncId, args: Vec<ValueId>, ret_ty: NirTy },
    /// `%dst = call_indirect %func_ptr(args...)` — indirect call
    CallIndirect { dst: ValueId, func_ptr: ValueId, args: Vec<ValueId>, ret_ty: NirTy },

    // ── I/O ────────────────────────────────────────────────────────────────
    /// `print %val` — print a value (returns Unit). `newline` carries
    /// the `print` vs `println` distinction, which lowering used to
    /// discard: it emitted this single instruction for BOTH, so the VM
    /// silently dropped every trailing newline. The differential
    /// harness missed it because every existing case used
    /// `print("{x}")`, never `println`.
    Print { val: ValueId, newline: bool },

    // ── Host I/O (Phase 5/M4 stdlib builtins) ─────────────────────────────
    //
    // One instruction per interpreter builtin: lowering maps the builtin
    // call to its instruction (see `lowering.rs`'s Call dispatch), the VM
    // executes it with interpreter-identical semantics (see `vm.rs`), and
    // the Cranelift backend lowers the scalar-shaped subset to
    // `runtime-native` imports (see `backends/cranelift/{abi,driver,lower}.rs`).
    //
    // Scalar subset (Bool/Int/Unit/String only — representable in
    // native code today): Sleep, FsExists, IoWrite, IoWriteLn, EnvSet,
    // LogEmit, HttpServerRegister, HttpServerShutdown. Everything else
    // returns Result/Option/List, which have no native representation
    // yet (Step 3 boundary): the VM executes those, native fails
    // loudly with `UnsupportedInstr` naming the instruction.
    /// `sleep %ms` — block the calling thread for `%ms` milliseconds.
    /// Returns Unit via a trailing `Const Unit` (same statement shape as
    /// `Print`). Negative input traps (mirrors the interpreter's panic).
    Sleep { ms: ValueId },
    /// `%dst = fs_exists %path` — `Bool`: does `%path` exist.
    FsExists { dst: ValueId, path: ValueId },
    /// `io_write %src` — write a string to stdout, no newline.
    IoWrite { src: ValueId },
    /// `io_writeln %src` — write a string to stdout plus a newline.
    IoWriteLn { src: ValueId },
    /// `env_set %name, %val` — set a process environment variable.
    EnvSet { name: ValueId, value: ValueId },
    /// `log_emit %level, %msg` — one `[LEVEL] msg` line to stderr.
    LogEmit { level: ValueId, message: ValueId },
    /// `http_server_register %server, %method, %path, %handler` —
    /// register a method+path route (handler is a function NAME string,
    /// resolved at serve time).
    HttpServerRegister { server: ValueId, method: ValueId, path: ValueId, handler: ValueId },
    /// `http_server_shutdown %server` — stop the server, release the port.
    HttpServerShutdown { server: ValueId },
    /// `%dst = http_send %method, %url, %headers, %body` — blocking
    /// plaintext-HTTP client over TCP. VM-only (returns
    /// `Result<String, String>`).
    HttpSend { dst: ValueId, method: ValueId, url: ValueId, headers: ValueId, body: ValueId },
    /// `%dst = http_server_listen %port` — bind `0.0.0.0:%port`. VM-only
    /// (returns `Result<Int, String>`).
    HttpServerListen { dst: ValueId, port: ValueId },
    /// `http_server_serve_loop %server` — blocking accept loop that
    /// dispatches to registered Noctivue handlers. VM-only (handler
    /// callbacks into program code have no native equivalent yet).
    HttpServerServeLoop { server: ValueId },
    /// `%dst = db_open %path` — open a SQLite database. VM-only
    /// (returns `Result<Int, String>`; native has no Result
    /// representation and no SQLite linkage yet).
    DbOpen { dst: ValueId, path: ValueId },
    /// `%dst = db_exec %handle, %sql, %params` — run DDL/DML, returning
    /// the changed-row count. VM-only.
    DbExec { dst: ValueId, handle: ValueId, sql: ValueId, params: ValueId },
    /// `%dst = db_query %handle, %sql, %params` — run a query, returning
    /// one JSON-array string per row. VM-only.
    DbQuery { dst: ValueId, handle: ValueId, sql: ValueId, params: ValueId },
    /// `%dst = db_close %handle` — close a database connection. VM-only.
    DbClose { dst: ValueId, handle: ValueId },
    /// `%dst = fs_read %path` — read a UTF-8 text file. VM-only
    /// (returns `Result<String, String>`).
    FsRead { dst: ValueId, path: ValueId },
    /// `%dst = fs_write %path, %contents` — write a UTF-8 text file.
    /// VM-only (returns `Result<Unit, String>`).
    FsWrite { dst: ValueId, path: ValueId, contents: ValueId },
    /// `%dst = fs_list_dir %path` — list immediate directory entries as
    /// basename-sorted `DirEntry` structs. VM-only (returns
    /// `Result<[DirEntry], String>`).
    FsListDir { dst: ValueId, path: ValueId },
    /// `%dst = fs_modified_millis %path` — file mtime as millis since
    /// the Unix epoch. VM-only (returns `Result<Int, String>`).
    FsModifiedMillis { dst: ValueId, path: ValueId },
    /// `%dst = env_get %name` — process environment lookup. VM-only
    /// (returns `Option<String>`).
    EnvGet { dst: ValueId, name: ValueId },
    /// `%dst = config_get %path, %key` — read a `key=value` file. VM-only
    /// (returns `Result<String, String>`).
    ConfigGet { dst: ValueId, path: ValueId, key: ValueId },
    /// `%dst = dotenv_load %path` — load a `KEY=VALUE` file into the
    /// process environment (process wins). VM-only (returns
    /// `Result<Int, String>`).
    DotenvLoad { dst: ValueId, path: ValueId },
    /// `%dst = time_mono_ms` — monotonic millis since an arbitrary
    /// process epoch (Phase 6/Wave 0, ADR-020). Scalar `Int` (u64
    /// range): never goes backward within a process; no wall-clock,
    /// no timezones at this layer.
    TimeMonoMs { dst: ValueId },
    /// `%dst = rng_seed %seed` — allocate an explicit-seed RNG handle
    /// (Phase 6/Wave 0, ADR-021). Scalar `Int` handle into the VM's
    /// RNG registry (db-registry shape); same seed replays the same
    /// sequence.
    RngSeed { dst: ValueId, seed: ValueId },
    /// `%dst = rng_next %handle` — advance the RNG and return its next
    /// value (Phase 6/Wave 0, ADR-021). Scalar `Int`; unknown handles
    /// trap loudly (never a silent default).
    RngNext { dst: ValueId, handle: ValueId },
    /// `task_cancel %handle` — request cooperative cancellation of the
    /// task `%handle` names (Phase 6/M5, ADR-024). Statement shape: no
    /// `dst`, Unit via a trailing `Const Unit` (same as `Sleep`).
    /// Idempotent; the target observes the flag at its next suspend
    /// point and the `await` that joins it yields the pinned
    /// `task {id} cancelled` value. VM-only: the VM spawns no tasks, so
    /// it owns no task registry and every handle is the never-spawned
    /// case — the arm raises a loud `VmError` rather than a silent
    /// success (the interpreter is the task-capable backend, see
    /// CONCURRENCY.md §7).
    TaskCancel { handle: ValueId },

    // ── Control Flow (Terminators) ──────────────────────────────────────────
    /// `branch target` — unconditional jump
    Branch { target: BlockId },
    /// `cond_branch %cond, then_block, else_block` — conditional branch
    CondBranch { cond: ValueId, then_block: BlockId, else_block: BlockId },
    /// `switch %val, cases[], default` — switch on enum tag
    Switch { val: ValueId, cases: Vec<(u32, BlockId)>, default: BlockId },
    /// `return [val]` — return from function
    Return { val: Option<ValueId> },
    /// `unreachable` — unreachable code
    Unreachable,
    /// `early_return val` — early return from current function (for ? propagation)
    EarlyReturn { val: ValueId },

    // ── Error Handling ──────────────────────────────────────────────────────
    /// `%dst = result_ok %val` — construct Ok
    ResultOk { dst: ValueId, val: ValueId, ty: NirTy },
    /// `%dst = result_err %val` — construct Err
    ResultErr { dst: ValueId, val: ValueId, ty: NirTy },
    /// `%dst = try_unwrap %src` — unwrap Result/Option (`?` lowering)
    TryUnwrap { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = option_some %val` — construct Some(value)
    OptionSome { dst: ValueId, val: ValueId, ty: NirTy },
    /// `%dst = option_none` — construct None
    OptionNone { dst: ValueId, ty: NirTy },

    // ── Conversion ─────────────────────────────────────────────────────────
    /// `%dst = to_string %val` — convert value to String.
    /// `from_ty` records the source type: string conversion is
    /// type-directed (int/float/bool each convert differently), and the
    /// Cranelift backend selects its runtime helper from this field.
    ToString { dst: ValueId, src: ValueId, from_ty: Ty },

    // ── Phi / Block Arguments ───────────────────────────────────────────────
    /// `%dst = phi [val0, block0], [val1, block1], ...` — phi node (SSA)
    Phi { dst: ValueId, incoming: Vec<(ValueId, BlockId)>, ty: NirTy },

    // ── Closures ────────────────────────────────────────────────────────────
    /// `%dst = closure_new @func(captured...)` — create closure
    ClosureNew { dst: ValueId, func: FuncId, captured: Vec<ValueId>, ty: NirTy },
    /// `%dst = closure_call %closure(args...)` — call closure
    ClosureCall { dst: ValueId, closure: ValueId, args: Vec<ValueId>, ret_ty: NirTy },
}

/// Constant values in NIR.
#[derive(Debug, Clone)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
    Unit,
}

impl fmt::Display for Instr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instr::Add { dst, lhs, rhs, ty } => write!(f, "{} = add {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Sub { dst, lhs, rhs, ty } => write!(f, "{} = sub {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Mul { dst, lhs, rhs, ty } => write!(f, "{} = mul {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Div { dst, lhs, rhs, ty } => write!(f, "{} = div {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Rem { dst, lhs, rhs, ty } => write!(f, "{} = rem {}, {} : {}", dst, lhs, rhs, ty),
            Instr::ICmp { dst, op, lhs, rhs } => write!(f, "{} = icmp {:?} {}, {}", dst, op, lhs, rhs),
            Instr::FCmp { dst, op, lhs, rhs } => write!(f, "{} = fcmp {:?} {}, {}", dst, op, lhs, rhs),
            Instr::Neg { dst, src, ty } => write!(f, "{} = neg {} : {}", dst, src, ty),
            Instr::Not { dst, src } => write!(f, "{} = not {}", dst, src),
            Instr::Const { dst, value, ty } => write!(f, "{} = const {} : {}", dst, value, ty),
            Instr::StackAlloc { dst, ty } => write!(f, "{} = stack_alloc : {}", dst, ty),
            Instr::Load { dst, src, ty } => write!(f, "{} = load {} : {}", dst, src, ty),
            Instr::Store { val, ptr } => write!(f, "store {}, {}", val, ptr),
            Instr::Move { dst, src } => write!(f, "{} = move {}", dst, src),
            Instr::HeapAlloc { dst, src, ty } => write!(f, "{} = heap_alloc {}, : {}", dst, src, ty),
            Instr::ArcRetain { src } => write!(f, "arc_retain {}", src),
            Instr::ArcRelease { src } => write!(f, "arc_release {}", src),
            Instr::ArcLoad { dst, src, ty } => write!(f, "{} = arc_load {} : {}", dst, src, ty),
            Instr::ArcId { dst, src } => write!(f, "{} = arc_id {}", dst, src),
            Instr::WeakCreate { dst, src, ty } => write!(f, "{} = weak_create {} : {}", dst, src, ty),
            Instr::WeakLoad { dst, src, ty } => write!(f, "{} = weak_load {} : {}", dst, src, ty),
            Instr::UnownedCreate { dst, src, ty } => {
                write!(f, "{} = unowned_create {} : {}", dst, src, ty)
            }
            Instr::UnownedLoad { dst, src, ty } => {
                write!(f, "{} = unowned_load {} : {}", dst, src, ty)
            }
            Instr::StructNew { dst, fields, field_names, ty } => write!(f, "{} = struct_new [{}] : {} ({})", dst, fields.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ty, field_names.join(", ")),
            Instr::FieldGet { dst, obj, field, ty } => write!(f, "{} = field_get {}, {} : {}", dst, obj, field, ty),
            Instr::FieldSet { dst, obj, field, val } => write!(f, "{} = field_set {}, {}, {}", dst, obj, field, val),
            Instr::ListLen { dst, src } => write!(f, "{} = list_len {}", dst, src),
            Instr::ListIndex { dst, src, index } => write!(f, "{} = list_index {}, {}", dst, src, index),
            Instr::EnumTag { dst, src } => write!(f, "{} = enum_tag {}", dst, src),
            Instr::EnumPayload { dst, src, index, ty } => write!(f, "{} = enum_payload {}[{}] : {}", dst, src, index, ty),
            Instr::EnumNew { dst, tag, fields, ty } => write!(f, "{} = enum_new {} [{}] : {}", dst, tag, fields.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ty),
            Instr::Call { dst, func, args, ret_ty } => write!(f, "{} = call {}({}) : {}", dst, func, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ret_ty),
            Instr::CallIndirect { dst, func_ptr, args, ret_ty } => write!(f, "{} = call_indirect {}({}) : {}", dst, func_ptr, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ret_ty),
            Instr::Print { val, newline } => {
                write!(f, "{} {}", if *newline { "println" } else { "print" }, val)
            }
            Instr::Sleep { ms } => write!(f, "sleep {}", ms),
            Instr::FsExists { dst, path } => write!(f, "{} = fs_exists {}", dst, path),
            Instr::IoWrite { src } => write!(f, "io_write {}", src),
            Instr::IoWriteLn { src } => write!(f, "io_writeln {}", src),
            Instr::EnvSet { name, value } => write!(f, "env_set {}, {}", name, value),
            Instr::LogEmit { level, message } => write!(f, "log_emit {}, {}", level, message),
            Instr::HttpServerRegister { server, method, path, handler } => write!(f, "http_server_register {}, {}, {}, {}", server, method, path, handler),
            Instr::HttpServerShutdown { server } => write!(f, "http_server_shutdown {}", server),
            Instr::HttpSend { dst, method, url, headers, body } => write!(f, "{} = http_send {}, {}, {}, {}", dst, method, url, headers, body),
            Instr::HttpServerListen { dst, port } => write!(f, "{} = http_server_listen {}", dst, port),
            Instr::HttpServerServeLoop { server } => write!(f, "http_server_serve_loop {}", server),
            Instr::DbOpen { dst, path } => write!(f, "{} = db_open {}", dst, path),
            Instr::DbExec { dst, handle, sql, params } => write!(f, "{} = db_exec {}, {}, {}", dst, handle, sql, params),
            Instr::DbQuery { dst, handle, sql, params } => write!(f, "{} = db_query {}, {}, {}", dst, handle, sql, params),
            Instr::DbClose { dst, handle } => write!(f, "{} = db_close {}", dst, handle),
            Instr::FsRead { dst, path } => write!(f, "{} = fs_read {}", dst, path),
            Instr::FsWrite { dst, path, contents } => write!(f, "{} = fs_write {}, {}", dst, path, contents),
            Instr::FsListDir { dst, path } => write!(f, "{} = fs_list_dir {}", dst, path),
            Instr::FsModifiedMillis { dst, path } => write!(f, "{} = fs_modified_millis {}", dst, path),
            Instr::EnvGet { dst, name } => write!(f, "{} = env_get {}", dst, name),
            Instr::ConfigGet { dst, path, key } => write!(f, "{} = config_get {}, {}", dst, path, key),
            Instr::DotenvLoad { dst, path } => write!(f, "{} = dotenv_load {}", dst, path),
            Instr::TimeMonoMs { dst } => write!(f, "{} = time_mono_ms", dst),
            Instr::RngSeed { dst, seed } => write!(f, "{} = rng_seed {}", dst, seed),
            Instr::RngNext { dst, handle } => write!(f, "{} = rng_next {}", dst, handle),
            Instr::TaskCancel { handle } => write!(f, "task_cancel {}", handle),
            Instr::Branch { target } => write!(f, "branch {}", target),
            Instr::CondBranch { cond, then_block, else_block } => write!(f, "cond_branch {}, {}, {}", cond, then_block, else_block),
            Instr::Switch { val, cases, default } => write!(f, "switch {} [{}] default {}", val, cases.iter().map(|(tag, blk)| format!("{} -> {}", tag, blk)).collect::<Vec<_>>().join(", "), default),
            Instr::Return { val } => write!(f, "return {}", val.map(|v| v.to_string()).unwrap_or_default()),
            Instr::EarlyReturn { val } => write!(f, "early_return {}", val),
            Instr::Unreachable => write!(f, "unreachable"),
            Instr::ResultOk { dst, val, ty } => write!(f, "{} = result_ok {} : {}", dst, val, ty),
            Instr::ResultErr { dst, val, ty } => write!(f, "{} = result_err {} : {}", dst, val, ty),
            Instr::TryUnwrap { dst, src, ty } => write!(f, "{} = try_unwrap {} : {}", dst, src, ty),
            Instr::OptionSome { dst, val, ty } => write!(f, "{} = option_some {} : {}", dst, val, ty),
            Instr::OptionNone { dst, ty } => write!(f, "{} = option_none : {}", dst, ty),
            Instr::ToString { dst, src, .. } => write!(f, "{} = to_string {}", dst, src),
            Instr::Phi { dst, incoming, ty } => write!(f, "{} = phi [{}] : {}", dst, incoming.iter().map(|(v, b)| format!("{} from {}", v, b)).collect::<Vec<_>>().join(", "), ty),
            Instr::ClosureNew { dst, func, captured, ty } => write!(f, "{} = closure_new {}({}) : {}", dst, func, captured.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ty),
            Instr::ClosureCall { dst, closure, args, ret_ty } => write!(f, "{} = closure_call {}({}) : {}", dst, closure, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ret_ty),
        }
    }
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstValue::Int(v) => write!(f, "{}", v),
            ConstValue::Float(v) => write!(f, "{}", v),
            ConstValue::Bool(v) => write!(f, "{}", v),
            ConstValue::Char(c) => write!(f, "'{}'", c),
            ConstValue::String(s) => write!(f, "\"{}\"", s.escape_default()),
            ConstValue::Unit => write!(f, "()"),
        }
    }
}