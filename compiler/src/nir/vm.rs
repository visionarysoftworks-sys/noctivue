//! NIR bytecode VM — executes NIR directly for faster iteration than native compilation.
//!
//! This is the M1 deliverable: a bytecode VM that consumes NIR and produces
//! identical observable behavior to the M0 tree-walking interpreter.

use crate::http_wire::{self, WireError};
use crate::nir::instr::{CmpOp, ConstValue, Instr};
use crate::nir::module::NirModule;
use crate::nir::types::{BlockId, FuncId, ValueId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Arc as StdArc, RwLock, Weak as StdWeak, LazyLock, Mutex};

#[derive(Debug, Clone)]
pub enum VmValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
    Unit,
    Struct { name: String, fields: HashMap<String, VmValue> },
    Enum { variant: String, tag: u32, fields: Vec<VmValue> },
    List(Vec<VmValue>),
    Option(Option<Box<VmValue>>),
    Result(Result<Box<VmValue>, Box<VmValue>>),
    Function(FuncId),
    Closure { func: FuncId, captured: Vec<VmValue> },
    Tuple(Vec<VmValue>),
    Range { start: Box<VmValue>, end: Box<VmValue>, inclusive: bool },
    Pointer(usize),
    // UI-S0: Managed mode (ARC) values
    Arc(VmArcValue),
    Weak(VmWeakValue),
    Unowned(VmUnownedValue),
}

impl VmValue {
    /// The runtime shape's name, for error messages. A managed-heap
    /// operation that gets the wrong shape must SAY which shape it got
    /// — "expected Arc value" alone leaves the reader guessing, and
    /// these are the messages that tell a `Weak` from a dangling
    /// `Unowned` at a glance.
    pub fn type_name(&self) -> &'static str {
        match self {
            VmValue::Int(_) => "Int",
            VmValue::Float(_) => "Float",
            VmValue::Bool(_) => "Bool",
            VmValue::Char(_) => "Char",
            VmValue::String(_) => "String",
            VmValue::Unit => "Unit",
            VmValue::Struct { .. } => "Struct",
            VmValue::Enum { .. } => "Enum",
            VmValue::List(_) => "List",
            VmValue::Option(_) => "Option",
            VmValue::Result(_) => "Result",
            VmValue::Function(_) => "Function",
            VmValue::Closure { .. } => "Closure",
            VmValue::Tuple(_) => "Tuple",
            VmValue::Arc(_) => "Arc",
            VmValue::Weak(_) => "Weak",
            VmValue::Unowned(_) => "Unowned",
            VmValue::Range { .. } => "Range",
            VmValue::Pointer(_) => "Pointer",
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            VmValue::Bool(b) => *b,
            VmValue::Int(i) => *i != 0,
            VmValue::Float(f) => *f != 0.0,
            VmValue::String(s) => !s.is_empty(),
            VmValue::List(l) => !l.is_empty(),
            VmValue::Option(Some(_)) => true,
            VmValue::Option(None) => false,
            VmValue::Result(Ok(_)) => true,
            VmValue::Result(Err(_)) => false,
            VmValue::Unit => false,
            _ => true,
        }
    }
}

impl std::fmt::Display for VmValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmValue::Int(i) => write!(f, "{}", i),
            VmValue::Float(fl) => write!(f, "{}", fl),
            VmValue::Bool(b) => write!(f, "{}", b),
            VmValue::Char(c) => write!(f, "{}", c),
            VmValue::String(s) => write!(f, "{}", s),
            VmValue::Unit => write!(f, "()"),
            VmValue::Struct { name, fields } => {
                let field_strs: Vec<String> = fields.iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect();
                write!(f, "{} {{ {} }}", name, field_strs.join(", "))
            }
            VmValue::Enum { variant, tag: _, fields } => {
                if fields.is_empty() {
                    write!(f, "{}", variant)
                } else {
                    let field_strs: Vec<String> = fields.iter().map(|v| v.to_string()).collect();
                    write!(f, "{}({})", variant, field_strs.join(", "))
                }
            }
            VmValue::List(l) => {
                let elem_strs: Vec<String> = l.iter().map(|v| v.to_string()).collect();
                write!(f, "[{}]", elem_strs.join(", "))
            }
            VmValue::Option(Some(v)) => write!(f, "Some({})", v),
            VmValue::Option(None) => write!(f, "None"),
            VmValue::Result(Ok(v)) => write!(f, "Ok({})", v),
            VmValue::Result(Err(e)) => write!(f, "Err({})", e),
            VmValue::Function(id) => write!(f, "<function {}>", id.0),
            VmValue::Closure { func, captured: _ } => write!(f, "<closure {}>", func.0),
            VmValue::Tuple(elems) => {
                let elem_strs: Vec<String> = elems.iter().map(|v| v.to_string()).collect();
                write!(f, "({})", elem_strs.join(", "))
            }
            VmValue::Range { start, end, inclusive } => {
                let end_str = if *inclusive { "=" } else { "" };
                write!(f, "{}..{}{}", start, end_str, end)
            }
            VmValue::Pointer(addr) => write!(f, "pointer@{:?}", addr),
            VmValue::Arc(arc) => write!(f, "<arc @{}>", arc.heap_id.0),
            VmValue::Weak(weak) => write!(f, "<weak @{}>", weak.heap_id.0),
            VmValue::Unowned(unowned) => write!(f, "<unowned @{}>", unowned.heap_id.0),
        }
    }
}

// ============================================================================
// UI-S0: Managed mode (ARC) runtime for NIR VM
// ============================================================================

/// Unique heap object identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VmHeapId(pub u64);

static VM_NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);

fn vm_alloc_heap_id() -> VmHeapId {
    VmHeapId(VM_NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed))
}

/// A heap-allocated object with reference counting.
#[derive(Debug)]
pub struct VmHeapObject {
    /// The actual value stored on the heap.
    pub value: RwLock<VmValue>,
    /// Strong reference count (Arc count).
    pub strong_count: AtomicU64,
    /// Weak reference count (Weak count).
    pub weak_count: AtomicU64,
}

impl VmHeapObject {
    pub fn new(value: VmValue) -> StdArc<Self> {
        StdArc::new(VmHeapObject {
            value: RwLock::new(value),
            strong_count: AtomicU64::new(1),
            weak_count: AtomicU64::new(1), // Arc itself holds a weak reference
        })
    }

    pub fn strong_count(&self) -> u64 {
        self.strong_count.load(Ordering::Relaxed)
    }

    pub fn weak_count(&self) -> u64 {
        self.weak_count.load(Ordering::Relaxed)
    }

    pub fn increment_strong(&self) {
        self.strong_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_strong(&self) -> u64 {
        self.strong_count.fetch_sub(1, Ordering::Relaxed) - 1
    }

    pub fn increment_weak(&self) {
        self.weak_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_weak(&self) -> u64 {
        self.weak_count.fetch_sub(1, Ordering::Relaxed) - 1
    }
}

/// Global heap registry for managed objects.
/// Maps HeapId to the heap object.
#[derive(Debug, Default)]
pub struct VmHeapRegistry {
    pub objects: HashMap<VmHeapId, StdArc<VmHeapObject>>,
}

impl VmHeapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a new managed object on the heap.
    /// Returns the HeapId and increments the strong count to 1.
    pub fn alloc(&mut self, value: VmValue) -> VmHeapId {
        let id = vm_alloc_heap_id();
        let obj = VmHeapObject::new(value);
        self.objects.insert(id, obj);
        id
    }

    /// Get a reference to the heap object (for reading/writing the value).
    pub fn get(&self, id: VmHeapId) -> Option<&StdArc<VmHeapObject>> {
        self.objects.get(&id)
    }

    /// Retain (increment strong count) for an Arc clone.
    pub fn retain(&self, id: VmHeapId) -> Result<(), String> {
        self.objects.get(&id)
            .map(|obj| obj.increment_strong())
            .ok_or_else(|| format!("Arc retain: heap object {} not found", id.0))
    }

    /// Release (decrement strong count). If count reaches 0, deallocate.
    pub fn release(&mut self, id: VmHeapId) -> Result<(), String> {
        let should_drop = self.objects.get(&id)
            .map(|obj| obj.decrement_strong() == 0)
            .ok_or_else(|| format!("Arc release: heap object {} not found", id.0))?;

        if should_drop {
            self.objects.remove(&id);
        }
        Ok(())
    }

    /// Create a weak reference to the object.
    pub fn weak(&self, id: VmHeapId) -> Result<VmWeakValue, String> {
        self.objects.get(&id)
            .map(|obj| {
                obj.increment_weak();
                VmWeakValue::new(StdArc::downgrade(obj), id)
            })
            .ok_or_else(|| format!("Weak creation: heap object {} not found", id.0))
    }

    /// Create an unowned back-reference. Deliberately does NOT touch
    /// any count — that is the entire difference from [`Self::weak`],
    /// and the reason a child can point at its parent without keeping
    /// the parent alive. Safety comes from the trap in
    /// `UnownedLoad`, not from the count.
    pub fn unowned(&self, id: VmHeapId) -> Result<VmUnownedValue, String> {
        if self.objects.contains_key(&id) {
            Ok(VmUnownedValue::new(id))
        } else {
            Err(format!(
                "Unowned creation: heap object {} not found",
                id.0
            ))
        }
    }
}

/// Strong reference (Arc) to a heap-allocated managed value.
#[derive(Debug, Clone)]
pub struct VmArcValue {
    pub heap_id: VmHeapId,
}

impl VmArcValue {
    pub fn new(heap_id: VmHeapId) -> Self {
        Self { heap_id }
    }
}

/// Weak reference to a heap-allocated managed value.
/// Upgrading returns `Option<VmArcValue>`.
#[derive(Debug, Clone)]
pub struct VmWeakValue {
    pub weak_ref: StdWeak<VmHeapObject>,
    pub heap_id: VmHeapId,
}

impl VmWeakValue {
    pub fn new(weak_ref: StdWeak<VmHeapObject>, heap_id: VmHeapId) -> Self {
        Self { weak_ref, heap_id }
    }

    /// Attempt to upgrade to a strong reference.
    /// Returns `None` if the object has been deallocated.
    pub fn upgrade(&self) -> Option<VmArcValue> {
        self.weak_ref.upgrade().map(|_| VmArcValue::new(self.heap_id))
    }
}

/// Unowned reference to a heap-allocated managed value.
/// Does not affect reference counts. Accessing a dangling unowned
/// reference traps at runtime (panic).
#[derive(Debug, Clone)]
pub struct VmUnownedValue {
    pub heap_id: VmHeapId,
}

impl VmUnownedValue {
    pub fn new(heap_id: VmHeapId) -> Self {
        Self { heap_id }
    }
}

// ── Phase 5/M4 host-IO state (VM mirrors of the interpreter) ───────────────
//
// Every helper and error string below mirrors `interp/src/lib.rs`
// exactly — the interpreter is the oracle, this module never invents
// its own messages or byte counts (see each item's note).

/// One registered HTTP route: exact method + path match, handler is a
/// function NAME resolved against the module at serve time.
#[derive(Debug, Clone)]
struct VmHttpRoute {
    method: String,
    path_pattern: String,
    handler: String,
}

struct VmServerState {
    listener: TcpListener,
    routes: Vec<VmHttpRoute>,
    shutdown: Arc<AtomicBool>,
}

static VM_SERVER_REGISTRY: LazyLock<Mutex<HashMap<u64, VmServerState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static VM_SERVER_COUNTER: AtomicU64 = AtomicU64::new(1);
/// Live per-connection VM server threads (mirrors the interpreter's
/// `live_connection_threads` hook for the Phase 5 leak pass).
static VM_CONN_THREADS: AtomicU64 = AtomicU64::new(0);
/// Currently open VM SQLite connections (mirrors
/// `live_db_connections`).
static VM_DB_OPEN: AtomicU64 = AtomicU64::new(0);
/// High-water marks for the gauges above.
static VM_CONN_PEAK: AtomicU64 = AtomicU64::new(0);
static VM_DB_PEAK: AtomicU64 = AtomicU64::new(0);

fn vm_track_up(gauge: &AtomicU64, peak: &AtomicU64) {
    let now = gauge.fetch_add(1, Ordering::Relaxed) + 1;
    peak.fetch_max(now, Ordering::Relaxed);
}

/// Live per-connection VM server threads right now.
pub fn vm_live_connection_threads() -> u64 {
    VM_CONN_THREADS.load(Ordering::Relaxed)
}

/// Currently open VM SQLite connections right now.
pub fn vm_live_db_connections() -> u64 {
    VM_DB_OPEN.load(Ordering::Relaxed)
}

/// Most live VM connection threads seen so far in this process.
pub fn vm_peak_connection_threads() -> u64 {
    VM_CONN_PEAK.load(Ordering::Relaxed)
}

/// Most open VM SQLite connections seen so far in this process.
pub fn vm_peak_db_connections() -> u64 {
    VM_DB_PEAK.load(Ordering::Relaxed)
}

/// Open SQLite connections for one VM instance. Mirrors the
/// interpreter's `DbRegistry` (`next` starts at 0 and pre-increments,
/// so the first handle is 1 — same shape, though cross-backend handle
/// equality is NOT promised).
#[derive(Debug, Default)]
struct VmDbRegistry {
    next: u64,
    conns: HashMap<u64, rusqlite::Connection>,
}

// ── Phase 6/Wave 0: monotonic clock + explicit-seed RNG (VM mirror) ──────
//
// Mirrors `interp/src/lib.rs` exactly (algorithm, messages, handle
// shape) — the interpreter is the oracle. ADR-020 (mono millis) +
// ADR-021 (no global entropy); same acceptance bar (same seed → same
// sequence across both runtimes).

/// Process-wide monotonic epoch (arbitrary — durations between reads
/// are the contract, never wall-clock interpretation).
static VM_TIME_EPOCH: LazyLock<std::time::Instant> =
    LazyLock::new(std::time::Instant::now);

/// Millis since `VM_TIME_EPOCH` (u64 range, fits `Int`).
fn vm_mono_ms() -> i128 {
    VM_TIME_EPOCH.elapsed().as_millis() as i128
}

/// SplitMix64 step (byte-identical to the interpreter's
/// `rng_next_u64`: sequential seeds diverge immediately).
fn vm_rng_next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Live RNG states by opaque handle id (one VM instance).
#[derive(Debug, Default)]
struct VmRngRegistry {
    next: u64,
    states: HashMap<u64, u64>,
}

impl Drop for Vm {
    /// Gauge reconcile for worker VMs (dropped, never `run`):
    /// mirrors the interpreter's `Drop` so the VM leak gauge cannot
    /// lie either. Skips on borrow failure rather than double-panic.
    fn drop(&mut self) {
        if let Ok(db) = self.db.try_borrow_mut() {
            let dropped = db.conns.len() as u64;
            if dropped > 0 {
                drop(db);
                VM_DB_OPEN.fetch_sub(dropped, Ordering::Relaxed);
            }
        }
    }
}

fn vm_ok(value: VmValue) -> VmValue {
    VmValue::Result(Ok(Box::new(value)))
}

fn vm_err(message: String) -> VmValue {
    VmValue::Result(Err(Box::new(VmValue::String(message))))
}

/// Read one directory level as sorted `DirEntry` values.
///
/// Mirrors the interpreter's listing exactly: byte-ordered UTF-8 basenames,
/// symlinks classified through their targets, and a point-in-time view in
/// which an entry that disappears before classification is neither a file
/// nor a directory.
fn vm_list_dir_entries(path: &str) -> Result<VmValue, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path)
        .map_err(|error| format!("cannot list directory `{path}`: {error}"))?
    {
        let entry = entry
            .map_err(|error| format!("cannot list directory `{path}`: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("directory `{path}` contains a non-UTF-8 file name"))?;
        names.push(name);
    }
    names.sort();

    let mut values = Vec::with_capacity(names.len());
    for name in names {
        let full_path = std::path::Path::new(path).join(&name);
        let (is_dir, is_file) = match std::fs::symlink_metadata(&full_path) {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    match std::fs::metadata(&full_path) {
                        Ok(target) => (target.is_dir(), target.is_file()),
                        Err(_) => (false, false),
                    }
                } else {
                    (file_type.is_dir(), file_type.is_file())
                }
            }
            Err(_) => (false, false),
        };
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), VmValue::String(name));
        fields.insert("is_dir".to_string(), VmValue::Bool(is_dir));
        fields.insert("is_file".to_string(), VmValue::Bool(is_file));
        values.push(VmValue::Struct {
            name: "DirEntry".to_string(),
            fields,
        });
    }
    Ok(VmValue::List(values))
}

/// Serve one accepted connection to completion on its worker
/// thread (VM mirror of the interpreter's worker): bounded read →
/// route → handler → one close-delimited response. Wire errors
/// answer without invoking a handler; handler failures (error,
/// non-String/non-Response, unknown name, panic) answer 500.
///
/// Status-aware handlers (Phase 6, mirrored byte-for-byte with the
/// interpreter): a handler may return an `HttpResponse` struct value
/// (`status: Int`, `reason: String`, `headers: [[String]]`,
/// `body: String`) or a plain `String` for 200-as-today. Rendering
/// and validation live in [`http_wire::encode_status_response`]
/// (shared); this function only extracts the plain parts, so the
/// runtimes cannot diverge on what a status means.
fn vm_serve_one_connection(mut worker: Vm, mut conn: std::net::TcpStream, routes: &[VmHttpRoute]) {
    let response_bytes = match http_wire::read_request(&mut conn) {
        Err(WireError::OverHeaderCap) | Err(WireError::OverBodyCap) => {
            http_wire::response(413, "Payload Too Large")
        }
        Err(_) => http_wire::response(400, "Bad Request"),
        Ok(req) => {
            let matched = routes
                .iter()
                .find(|r| r.method == req.method && r.path_pattern == req.path)
                .map(|r| r.handler.clone());
            match matched {
                None => http_wire::response(404, "Not Found"),
                Some(handler_name) => {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let fid = worker.module.get_function(&handler_name).map(|f| f.id);
                        match fid {
                            Some(id) => worker.call(id, vec![VmValue::String(req.body)]),
                            None => Err(VmError::FunctionNotFound(
                                crate::nir::types::FuncId::UNRESOLVED,
                            )),
                        }
                    }));
                    match outcome {
                        Ok(Ok(VmValue::String(s))) => http_wire::response(200, &s),
                        Ok(Ok(VmValue::Struct { name, fields })) if name == "HttpResponse" => {
                            match vm_decode_http_response(&fields) {
                                Some((status, reason, headers, body)) => {
                                    match http_wire::encode_status_response(
                                        status, &reason, &headers, &body,
                                    ) {
                                        Ok(encoded) => {
                                            for skipped in &encoded.skipped_wire_owned {
                                                eprintln!(
                                                    "http handler `{handler_name}` set wire-owned header `{skipped}`; skipped (framing is wire-owned)"
                                                );
                                            }
                                            encoded.bytes
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "http handler `{handler_name}` returned an unrenderable HttpResponse ({e:?}); answering 500"
                                            );
                                            http_wire::response(500, "Internal Server Error")
                                        }
                                    }
                                }
                                None => {
                                    eprintln!(
                                        "http handler `{handler_name}` returned a malformed HttpResponse (need status: Int, reason: String, headers: [[String]], body: String); answering 500"
                                    );
                                    http_wire::response(500, "Internal Server Error")
                                }
                            }
                        }
                        Ok(Ok(_)) => {
                            eprintln!(
                                "http handler `{handler_name}` returned a non-String value; answering 500"
                            );
                            http_wire::response(500, "Internal Server Error")
                        }
                        Ok(Err(e)) => {
                            eprintln!("http handler `{handler_name}` failed: {e}");
                            http_wire::response(500, "Internal Server Error")
                        }
                        Err(_) => {
                            eprintln!("http handler `{handler_name}` panicked; answering 500");
                            http_wire::response(500, "Internal Server Error")
                        }
                    }
                }
            }
        }
    };
    http_wire::write_bytes(&mut conn, &response_bytes);
}

/// Extract the plain `(status, reason, headers, body)` parts from an
/// `HttpResponse` struct value (VM mirror of the interpreter's
/// `decode_http_response`: same strictness, same `None`-means-500
/// contract — field plumbing only, every decision lives in
/// [`http_wire::encode_status_response`]).
fn vm_decode_http_response(
    fields: &HashMap<String, VmValue>,
) -> Option<(i128, String, Vec<(String, String)>, String)> {
    let status = match fields.get("status") {
        Some(VmValue::Int(n)) => *n,
        _ => return None,
    };
    let reason = match fields.get("reason") {
        Some(VmValue::String(s)) => s.clone(),
        _ => return None,
    };
    let body = match fields.get("body") {
        Some(VmValue::String(s)) => s.clone(),
        _ => return None,
    };
    let headers = match fields.get("headers") {
        Some(VmValue::List(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    VmValue::List(pair) if pair.len() == 2 => match (&pair[0], &pair[1]) {
                        (VmValue::String(k), VmValue::String(v)) => {
                            out.push((k.clone(), v.clone()))
                        }
                        _ => return None,
                    },
                    _ => return None,
                }
            }
            out
        }
        _ => return None,
    };
    Some((status, reason, headers, body))
}

fn vm_parse_http_url(url: &str) -> Option<(String, u16, String)> {
    // Plain HTTP only: no TLS stack, so `https://` is refused (None),
    // exactly like the interpreter.
    if url.strip_prefix("https://").is_some() {
        return None;
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let path = format!("/{path}");
        let (host, port_str) = authority.split_once(':').unwrap_or((authority, "80"));
        let port: u16 = port_str.parse().ok()?;
        Some((host.to_string(), port, path))
    } else {
        None
    }
}

fn vm_split_http_body(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let body_start = text.find("\r\n\r\n")?;
    Some(text[body_start + 4..].to_string())
}

/// Minimal JSON string quoting for SQLite result rendering (mirrors
/// the interpreter's `json_quote`: `\" \\ \n \r \t` plus `\uXXXX` for
/// other controls).
fn vm_json_quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render one SQLite value as JSON (mirrors the interpreter's
/// `render_json_value`: integers bare, reals shortest round-trip via
/// `{:?}`, text quoted, blobs as lowercase hex strings, NULL as
/// `null`).
fn vm_render_json_value(value: &rusqlite::types::Value) -> String {
    use rusqlite::types::Value as SqlValue;
    match value {
        SqlValue::Null => "null".to_string(),
        SqlValue::Integer(int) => int.to_string(),
        SqlValue::Real(float) => format!("{float:?}"),
        SqlValue::Text(text) => vm_json_quote(text),
        SqlValue::Blob(bytes) => {
            let mut out = String::from("\"");
            for byte in bytes {
                out.push_str(&format!("{byte:02x}"));
            }
            out.push('"');
            out
        }
    }
}

/// Parse the params JSON array of scalars into rusqlite bind values
/// (mirrors the interpreter's `parse_json_params`, including every
/// error string — params are scalars by contract, nesting is loud).
fn vm_parse_json_params(text: &str) -> Result<Vec<rusqlite::types::Value>, String> {
    use rusqlite::types::Value as SqlValue;
    // A small JSON-string scanner (mirrors the interpreter's
    // `parse_json_string` — escapes `\" \\ \/ \n \r \t` only).
    fn scan_string(text: &str) -> Result<(String, &str), String> {
        let mut chars = text.char_indices();
        if chars.next().map(|(_, c)| c) != Some('"') {
            return Err("expected JSON string".to_string());
        }
        let mut value = String::new();
        let mut escaped = false;
        for (index, ch) in chars {
            if escaped {
                value.push(match ch {
                    '"' | '\\' | '/' => ch,
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return Err("unsupported JSON escape".to_string()),
                });
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return Ok((value, &text[index + 1..]));
            } else {
                value.push(ch);
            }
        }
        Err("unterminated JSON string".to_string())
    }
    // First scalar separator: a comma or the closing bracket —
    // whichever comes first (same scan the interpreter does).
    fn scalar_end(body: &str) -> usize {
        body.find([',', ']']).unwrap_or(body.len())
    }
    let mut body = text.trim();
    body = body
        .strip_prefix('[')
        .ok_or_else(|| "params must be a JSON array like `[1, \"x\"]`".to_string())?
        .trim_start();
    if let Some(rest) = body.strip_prefix(']') {
        if rest.trim().is_empty() {
            return Ok(Vec::new());
        }
        return Err("params must be a JSON array like `[1, \"x\"]`".to_string());
    }
    let mut out = Vec::new();
    loop {
        if body.starts_with('"') {
            let (value, rest) = scan_string(body)?;
            out.push(SqlValue::Text(value));
            body = rest.trim_start();
        } else if let Some(rest) = body.strip_prefix("true") {
            out.push(SqlValue::Integer(1));
            body = rest.trim_start();
        } else if let Some(rest) = body.strip_prefix("false") {
            out.push(SqlValue::Integer(0));
            body = rest.trim_start();
        } else if let Some(rest) = body.strip_prefix("null") {
            out.push(SqlValue::Null);
            body = rest.trim_start();
        } else {
            let end = scalar_end(body);
            let token = body[..end].trim();
            if token.is_empty() {
                return Err("params must be a JSON array like `[1, \"x\"]`".to_string());
            }
            if let Ok(int) = token.parse::<i64>() {
                out.push(SqlValue::Integer(int));
            } else if {
                let digits = token.trim_start_matches('-');
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            } {
                // Integer-shaped but outside `i64`: reject loudly
                // instead of silently coercing through `f64`
                // (`docs/PHASE5_PRODUCTION.md` §4; mirrors the
                // interpreter).
                return Err(format!(
                    "integer param `{token}` out of range (binds as i64)"
                ));
            } else if let Ok(float) = token.parse::<f64>() {
                out.push(SqlValue::Real(float));
            } else {
                return Err(format!("unsupported JSON param `{token}` (scalars only)"));
            }
            body = body[end..].trim_start();
        }
        if let Some(rest) = body.strip_prefix(',') {
            body = rest.trim_start();
            continue;
        }
        if let Some(rest) = body.strip_prefix(']') {
            if !rest.trim().is_empty() {
                return Err("trailing bytes after JSON params array".to_string());
            }
            return Ok(out);
        }
        return Err("params must be a JSON array like `[1, \"x\"]`".to_string());
    }
}

/// The `KEY=VALUE` parsing core behind `DotenvLoad` (mirrors the
/// interpreter's `dotenv_load_file`: blank lines and `#` comments
/// skipped; lines without `=` and empty names warn on stderr and skip;
/// `KEY`/`VALUE` trimmed; the process environment always wins).
/// Returns the number of variables set, or the read failure.
fn vm_dotenv_load_file(path: &str) -> Result<i128, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read dotenv file `{path}`: {e}"))?;
    let mut loaded = 0i128;
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            eprintln!("dotenv: ignoring malformed line {} in `{path}`", index + 1);
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            eprintln!("dotenv: ignoring malformed line {} in `{path}`", index + 1);
            continue;
        }
        if std::env::var_os(name).is_none() {
            unsafe { std::env::set_var(name, value.trim()) };
            loaded += 1;
        }
    }
    Ok(loaded)
}

pub struct Vm {
    module: NirModule,
    call_stack: Vec<CallFrame>,
    globals: HashMap<FuncId, VmValue>,
    heap: Vec<VmValue>,
    /// UI-S0: Managed mode (ARC) heap registry
    managed_heap: RefCell<VmHeapRegistry>,
    /// Open SQLite connections by opaque handle id (Phase 5/M4:
    /// `Db*`). Interior mutability: `execute_instr` only has `&mut
    /// self` shared across arms, and the VM is single-threaded —
    /// same story as the interpreter's `RefCell<DbRegistry>`.
    db: RefCell<VmDbRegistry>,
    /// Explicit-seed RNG states by opaque handle id (Phase 6/Wave 0:
    /// `Rng*`). Same interior-mutability story as `db`.
    rng: RefCell<VmRngRegistry>,
}

#[derive(Debug)]
struct CallFrame {
    func: FuncId,
    block: BlockId,
    prev_block: Option<BlockId>,
    pc: usize,
    locals: Vec<VmValue>,
    #[allow(dead_code)]
    block_params: Vec<VmValue>,
    /// Destination register in the *caller* frame where the return value should be stored
    return_dst: Option<ValueId>,
}

impl Vm {
    pub fn new(module: NirModule) -> Self {
        let mut vm = Vm {
            module,
            call_stack: Vec::new(),
            globals: HashMap::new(),
            heap: Vec::new(),
            managed_heap: RefCell::new(VmHeapRegistry::new()),
            db: RefCell::new(VmDbRegistry::default()),
            rng: RefCell::new(VmRngRegistry::default()),
        };
        for func in &vm.module.functions {
            vm.globals.insert(func.id, VmValue::Function(func.id));
        }
        vm
    }

    pub fn run(&mut self) -> Result<VmValue, VmError> {
        let main_id = self.module.get_function("main")
            .map(|f| f.id)
            .ok_or(VmError::NoMainFunction)?;
        self.call(main_id, vec![])
    }

    pub fn call(&mut self, func_id: FuncId, args: Vec<VmValue>) -> Result<VmValue, VmError> {
        self.call_with_return_dst(func_id, args, None)
    }

    fn call_with_return_dst(&mut self, func_id: FuncId, args: Vec<VmValue>, return_dst: Option<ValueId>) -> Result<VmValue, VmError> {
        let func = self.module.get_function_by_id(func_id)
            .ok_or(VmError::FunctionNotFound(func_id))?;
        let entry_block = func.entry_block()
            .ok_or(VmError::NoEntryBlock(func_id))?
            .id;

        let mut locals = vec![VmValue::Unit; 1000];
        for (i, arg) in args.into_iter().enumerate() {
            locals[i] = arg;
        }

        let frame = CallFrame {
            func: func_id,
            block: entry_block,
            prev_block: None,
            pc: 0,
            locals,
            block_params: Vec::new(),
            return_dst,
        };
        self.call_stack.push(frame);

        self.run_current_frame()
    }

    fn run_current_frame(&mut self) -> Result<VmValue, VmError> {
        loop {
            let (func_id, block_id, pc) = {
                let frame = self.call_stack.last().unwrap();
                (frame.func, frame.block, frame.pc)
            };

            let func = self.module.get_function_by_id(func_id).unwrap();
            let block = func.blocks.iter().find(|b| b.id == block_id).unwrap();

            if pc < block.instrs.len() {
                let instr = block.instrs[pc].clone();
                self.call_stack.last_mut().unwrap().pc += 1;
                let mut frame = self.call_stack.pop().unwrap();
                self.execute_instr(&instr, &mut frame)?;
                self.call_stack.push(frame);
            } else if let Some(term) = block.terminator.clone() {
                let mut frame = self.call_stack.pop().unwrap();
                let ctrl = self.execute_terminator(&term, &mut frame)?;
                match ctrl {
                    ControlFlow::Continue => {
                        self.call_stack.push(frame);
                    }
                    ControlFlow::Return(result) => {
                        if self.call_stack.is_empty() {
                            return Ok(result);
                        }
                        return Ok(result);
                    }
                }
            } else {
                let func_name = func.name.clone();
                return Err(VmError::NoTerminator(block_id, func_name));
            }
        }
    }

    fn execute_instr(&mut self, instr: &Instr, frame: &mut CallFrame) -> Result<(), VmError> {
        match instr {
            Instr::Add { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.add_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Sub { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.sub_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Mul { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.mul_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Div { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.div_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Rem { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.rem_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::ICmp { dst, op, lhs, rhs } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.icmp_values(*op, a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::FCmp { dst, op, lhs, rhs } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.fcmp_values(*op, a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Neg { dst, src, ty: _ } => {
                let a = self.get_value(frame, *src)?;
                let result = self.neg_value(a)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Not { dst, src } => {
                let a = self.get_value(frame, *src)?;
                let result = self.not_value(a)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Const { dst, value, ty: _ } => {
                frame.locals[dst.0 as usize] = self.const_to_value(value)?;
            }
            Instr::StackAlloc { dst, ty: _ } => {
                let ptr = self.heap.len();
                self.heap.push(VmValue::Unit);
                frame.locals[dst.0 as usize] = VmValue::Pointer(ptr);
            }
            Instr::Load { dst, src, ty: _ } => {
                let ptr_val = self.get_value(frame, *src)?;
                if let VmValue::Pointer(idx) = ptr_val {
                    if idx < self.heap.len() {
                        frame.locals[dst.0 as usize] = self.heap[idx].clone();
                    } else {
                        frame.locals[dst.0 as usize] = VmValue::Unit;
                    }
                } else {
                    frame.locals[dst.0 as usize] = ptr_val;
                }
            }
            Instr::Store { val, ptr } => {
                let ptr_val = self.get_value(frame, *ptr)?;
                let store_val = self.get_value(frame, *val)?;
                if let VmValue::Pointer(idx) = ptr_val {
                    if idx < self.heap.len() {
                        self.heap[idx] = store_val;
                    }
                }
            }
            Instr::Move { dst, src } => {
                let val = self.get_value(frame, *src)?;
                frame.locals[dst.0 as usize] = val;
            }
            // UI-S0: Managed mode (ARC) instructions
            Instr::HeapAlloc { dst, src, ty: _ } => {
                let value = self.get_value(frame, *src)?;
                let mut managed_heap = self.managed_heap.borrow_mut();
                let heap_id = managed_heap.alloc(value);
                frame.locals[dst.0 as usize] = VmValue::Arc(VmArcValue::new(heap_id));
            }
            Instr::ArcRetain { src } => {
                let arc_val = self.get_value(frame, *src)?;
                // Wrong shape is a loud error. It used to be `if let`
                // with no else, so passing a non-Arc silently did
                // nothing and the program carried on with a refcount
                // it believed it had bumped.
                let VmValue::Arc(arc) = arc_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "arc_retain: expected Arc value, found {}",
                        arc_val.type_name()
                    )));
                };
                let managed_heap = self.managed_heap.borrow();
                managed_heap
                    .retain(arc.heap_id)
                    .map_err(VmError::ManagedPanic)?;
            }
            Instr::ArcRelease { src } => {
                let arc_val = self.get_value(frame, *src)?;
                let VmValue::Arc(arc) = arc_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "arc_release: expected Arc value, found {}",
                        arc_val.type_name()
                    )));
                };
                let mut managed_heap = self.managed_heap.borrow_mut();
                managed_heap
                    .release(arc.heap_id)
                    .map_err(VmError::ManagedPanic)?;
            }
            Instr::ArcLoad { dst, src, ty: _ } => {
                let arc_val = self.get_value(frame, *src)?;
                let VmValue::Arc(arc) = arc_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "arc_load: expected Arc value, found {}",
                        arc_val.type_name()
                    )));
                };
                let managed_heap = self.managed_heap.borrow();
                let Some(obj) = managed_heap.get(arc.heap_id) else {
                    // Unreachable while refcounts are honest, so treat it as
                    // the bug it is rather than defaulting to Unit.
                    return Err(VmError::ManagedPanic(format!(
                        "arc_load: heap object {} not found (dangling strong reference)",
                        arc.heap_id.0
                    )));
                };
                let value = obj.value.read().map_err(|_| {
                    VmError::ManagedPanic(format!(
                        "arc_load: heap object {} is poisoned",
                        arc.heap_id.0
                    ))
                })?.clone();
                frame.locals[dst.0 as usize] = value;
            }
            Instr::ArcId { dst, src } => {
                let arc_val = self.get_value(frame, *src)?;
                let VmValue::Arc(arc) = arc_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "arc_id: expected Arc value, found {}",
                        arc_val.type_name()
                    )));
                };
                frame.locals[dst.0 as usize] = VmValue::Int(arc.heap_id.0 as i128);
            }
            Instr::WeakCreate { dst, src, ty: _ } => {
                let arc_val = self.get_value(frame, *src)?;
                let VmValue::Arc(arc) = arc_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "weak_create: expected Arc value, found {}",
                        arc_val.type_name()
                    )));
                };
                let managed_heap = self.managed_heap.borrow();
                let weak = managed_heap
                    .weak(arc.heap_id)
                    .map_err(VmError::ManagedPanic)?;
                frame.locals[dst.0 as usize] = VmValue::Weak(weak);
            }
            Instr::WeakLoad { dst, src, ty: _ } => {
                let weak_val = self.get_value(frame, *src)?;
                // A non-Weak argument used to answer `None` — a silent
                // wrong answer that reads exactly like "the object was
                // freed". `None` must mean *dead*, never *wrong type*.
                let VmValue::Weak(weak) = weak_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "weak_load: expected Weak value, found {}",
                        weak_val.type_name()
                    )));
                };
                let result = match weak.upgrade() {
                    Some(arc) => VmValue::Option(Some(Box::new(VmValue::Arc(arc)))),
                    None => VmValue::Option(None),
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::UnownedCreate { dst, src, ty: _ } => {
                let arc_val = self.get_value(frame, *src)?;
                let VmValue::Arc(arc) = arc_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "unowned_create: expected Arc value, found {}",
                        arc_val.type_name()
                    )));
                };
                let managed_heap = self.managed_heap.borrow();
                let unowned = managed_heap
                    .unowned(arc.heap_id)
                    .map_err(VmError::ManagedPanic)?;
                frame.locals[dst.0 as usize] = VmValue::Unowned(unowned);
            }
            Instr::UnownedLoad { dst, src, ty: _ } => {
                let unowned_val = self.get_value(frame, *src)?;
                let VmValue::Unowned(unowned) = unowned_val else {
                    return Err(VmError::ManagedPanic(format!(
                        "unowned_load: expected Unowned value, found {}",
                        unowned_val.type_name()
                    )));
                };
                // The trap. A dangling unowned back-reference is a
                // program bug, so it stops the program — matching the
                // interpreter's panic, and deliberately NOT the `None`
                // that `weak_load` returns for the same condition.
                let managed_heap = self.managed_heap.borrow();
                let Some(obj) = managed_heap.get(unowned.heap_id) else {
                    return Err(VmError::ManagedPanic(format!(
                        "unowned_load: unowned reference trap - heap object {} deallocated",
                        unowned.heap_id.0
                    )));
                };
                let value = obj.value.read().map_err(|_| {
                    VmError::ManagedPanic(format!(
                        "unowned_load: heap object {} is poisoned",
                        unowned.heap_id.0
                    ))
                })?.clone();
                frame.locals[dst.0 as usize] = value;
            }
            Instr::StructNew { dst, fields, field_names, ty } => {
                let field_values: Vec<VmValue> = fields.iter()
                    .map(|f| self.get_value(frame, *f))
                    .collect::<Result<Vec<_>, _>>()?;
                // Lowering reuses StructNew for list/tuple literals (empty
                // field_names, List/Tuple type). They must come back out as
                // List/Tuple values — previously everything became Struct,
                // so `ListLen` saw length 0 and every for-loop over a
                // literal silently ran zero iterations.
                let value = match &ty.inner {
                    crate::hir::types::Ty::List(_) => VmValue::List(field_values),
                    crate::hir::types::Ty::Tuple(_) => VmValue::Tuple(field_values),
                    _ => {
                        let name = match &ty.inner {
                            crate::hir::types::Ty::Named(n, _) => n.clone(),
                            _ => "unknown".to_string(),
                        };
                        let mut fields_map = HashMap::new();
                        if field_names.is_empty() {
                            for (i, val) in field_values.into_iter().enumerate() {
                                fields_map.insert(format!("f{}", i), val);
                            }
                        } else {
                            for (i, val) in field_values.into_iter().enumerate() {
                                if i < field_names.len() {
                                    fields_map.insert(field_names[i].clone(), val);
                                } else {
                                    fields_map.insert(format!("f{}", i), val);
                                }
                            }
                        }
                        VmValue::Struct { name, fields: fields_map }
                    }
                };
                frame.locals[dst.0 as usize] = value;
            }
            Instr::FieldGet { dst, obj, field, ty: _ } => {
                let obj_val = self.get_value(frame, *obj)?;
                let result = if let VmValue::Struct { fields, .. } = obj_val {
                    fields.get(field).cloned().unwrap_or(VmValue::Unit)
                } else {
                    VmValue::Unit
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::FieldSet { dst, obj, field, val } => {
                let obj_val = self.get_value(frame, *obj)?;
                let val_val = self.get_value(frame, *val)?;
                let result = if let VmValue::Struct { name, fields } = obj_val {
                    let mut new_fields = fields.clone();
                    new_fields.insert(field.clone(), val_val);
                    VmValue::Struct { name, fields: new_fields }
                } else {
                    obj_val
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::ListLen { dst, src } => {
                let src_val = self.get_value(frame, *src)?;
                let len = match src_val {
                    VmValue::List(v) => v.len() as i128,
                    VmValue::String(s) => s.len() as i128,
                    VmValue::Tuple(v) => v.len() as i128,
                    _ => 0,
                };
                frame.locals[dst.0 as usize] = VmValue::Int(len);
            }
            Instr::ListIndex { dst, src, index } => {
                let src_val = self.get_value(frame, *src)?;
                let idx_val = self.get_value(frame, *index)?;
                let result = match (src_val, idx_val) {
                    (VmValue::List(v), VmValue::Int(i)) => {
                        v.get(i as usize).cloned().unwrap_or(VmValue::Unit)
                    }
                    (VmValue::String(s), VmValue::Int(i)) => {
                        s.chars().nth(i as usize).map(VmValue::Char).unwrap_or(VmValue::Unit)
                    }
                    (VmValue::Tuple(v), VmValue::Int(i)) => {
                        v.get(i as usize).cloned().unwrap_or(VmValue::Unit)
                    }
                    _ => VmValue::Unit,
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::EnumTag { dst, src } => {
                let src_val = self.get_value(frame, *src)?;
                let tag = if let VmValue::Enum { tag, .. } = src_val {
                    VmValue::Int(tag as i128)
                } else {
                    VmValue::Int(0)
                };
                frame.locals[dst.0 as usize] = tag;
            }
            Instr::EnumPayload { dst, src, index, ty: _ } => {
                let src_val = self.get_value(frame, *src)?;
                // Option/Result payloads extract just like enum-variant
                // payloads (used by `?`-on-Err lowering to forward the
                // original error value). Previously these fell through to
                // Unit, silently dropping payloads.
                let payload = match src_val {
                    VmValue::Enum { fields, .. } => {
                        fields.get(*index as usize).cloned().unwrap_or(VmValue::Unit)
                    }
                    VmValue::Option(Some(v)) => *v,
                    VmValue::Option(None) => VmValue::Unit,
                    VmValue::Result(Ok(v)) => *v,
                    VmValue::Result(Err(e)) => *e,
                    _ => VmValue::Unit,
                };
                frame.locals[dst.0 as usize] = payload;
            }
            Instr::EnumNew { dst, tag, fields, ty: _ } => {
                let tag_val = self.get_value(frame, *tag)?;
                let tag = if let VmValue::Int(i) = tag_val { i as u32 } else { 0 };
                let field_vals: Vec<VmValue> = fields.iter()
                    .map(|f| self.get_value(frame, *f))
                    .collect::<Result<Vec<_>, _>>()?;
                frame.locals[dst.0 as usize] = VmValue::Enum { variant: String::new(), tag, fields: field_vals };
            }
            Instr::Call { dst, func, args, ret_ty: _ } => {
                let arg_vals: Vec<VmValue> = args.iter()
                    .map(|v| self.get_value(frame, *v))
                    .collect::<Result<Vec<_>, _>>()?;
                // Call with return_dst so callee writes directly to our dst
                let result = self.call_with_return_dst(*func, arg_vals, Some(*dst))?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::CallIndirect { dst, func_ptr, args, ret_ty: _ } => {
                let ptr = self.get_value(frame, *func_ptr)?;
                if let VmValue::Function(fid) = ptr {
                    let arg_vals: Vec<VmValue> = args.iter()
                        .map(|v| self.get_value(frame, *v))
                        .collect::<Result<Vec<_>, _>>()?;
                    let result = self.call_with_return_dst(fid, arg_vals, Some(*dst))?;
                    frame.locals[dst.0 as usize] = result;
                } else {
                    frame.locals[dst.0 as usize] = VmValue::Unit;
                }
            }
            Instr::Print { val, newline } => {
                let v = self.get_value(frame, *val)?;
                // Byte-for-byte parity with the interpreter, which is the
                // oracle: `print` emits no newline, `println` emits
                // exactly one. The two were previously indistinguishable
                // here, so VM stdout ran every line together.
                if *newline {
                    println!("{}", v);
                } else {
                    print!("{}", v);
                }
            }
            // ── Phase 5/M4 host-IO builtins ──────────────────────────
            // Every arm mirrors `interp/src/lib.rs`'s `eval_builtin`
            // arm of the same name exactly (messages, shapes, blocking
            // behavior) — the interpreter is the oracle. Scalar-shaped
            // builtins additionally lower to native imports (see
            // `backends/cranelift`); aggregate-returning ones are
            // VM-only (no native value representation yet).
            Instr::Sleep { ms } => {
                // Cooperative BLOCKING sleep (the M4 floor, not a
                // timer): real wall-clock block, like the interpreter.
                let v = self.get_value(frame, *ms)?;
                match v {
                    VmValue::Int(n) if n >= 0 => {
                        std::thread::sleep(std::time::Duration::from_millis(n as u64));
                    }
                    _ => {
                        return Err(VmError::TypeMismatch(
                            "sleep_builtin requires a non-negative Int (milliseconds)"
                                .to_string(),
                        ));
                    }
                }
            }
            Instr::FsExists { dst, path } => {
                let v = self.get_value(frame, *path)?;
                // Non-String operand answers `false` (mirrors the
                // interpreter — typeck guarantees String, so this is
                // recovery-only, never a trap).
                let exists = match v {
                    VmValue::String(p) => std::path::Path::new(&p).exists(),
                    _ => false,
                };
                frame.locals[dst.0 as usize] = VmValue::Bool(exists);
            }
            Instr::IoWrite { src } => {
                let v = self.get_value(frame, *src)?;
                print!("{v}");
            }
            Instr::IoWriteLn { src } => {
                let v = self.get_value(frame, *src)?;
                println!("{v}");
            }
            Instr::EnvSet { name, value } => {
                let n = self.get_value(frame, *name)?;
                let v = self.get_value(frame, *value)?;
                match (n, v) {
                    (VmValue::String(n), VmValue::String(v)) => {
                        unsafe { std::env::set_var(n, v) };
                    }
                    _ => {
                        eprintln!("env_set_builtin: expected name and value Strings");
                    }
                }
            }
            Instr::LogEmit { level, message } => {
                let l = self.get_value(frame, *level)?;
                let m = self.get_value(frame, *message)?;
                match (l, m) {
                    (VmValue::String(level), VmValue::String(message)) => {
                        // One line, stderr, no timestamp (deterministic
                        // output — the level gate lives in `log/log.nv`).
                        eprintln!("[{}] {}", level.to_ascii_uppercase(), message);
                    }
                    _ => {
                        eprintln!("[ERROR] log_emit_builtin: expected level and message Strings");
                    }
                }
            }
            Instr::HttpServerRegister { server, method, path, handler } => {
                let s = self.get_value(frame, *server)?;
                let m = self.get_value(frame, *method)?;
                let p = self.get_value(frame, *path)?;
                let h = self.get_value(frame, *handler)?;
                if let (
                    VmValue::Int(id),
                    VmValue::String(method),
                    VmValue::String(path),
                    VmValue::String(handler),
                ) = (s, m, p, h)
                {
                    let mut reg =
                        VM_SERVER_REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(state) = reg.get_mut(&(id as u64)) {
                        state.routes.push(VmHttpRoute {
                            method,
                            path_pattern: path,
                            handler,
                        });
                    }
                }
            }
            Instr::HttpServerShutdown { server } => {
                let s = self.get_value(frame, *server)?;
                if let VmValue::Int(id) = s {
                    let mut reg =
                        VM_SERVER_REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(state) = reg.get(&(id as u64)) {
                        state.shutdown.store(true, Ordering::Relaxed);
                    }
                    reg.remove(&(id as u64));
                }
            }
            Instr::HttpSend { dst, method, url, headers, body } => {
                let m = self.get_value(frame, *method)?;
                let u = self.get_value(frame, *url)?;
                let h = self.get_value(frame, *headers)?;
                let b = self.get_value(frame, *body)?;
                let result = match (m, u, h, b) {
                    (
                        VmValue::String(method),
                        VmValue::String(url),
                        VmValue::List(headers),
                        VmValue::String(body),
                    ) => self.vm_http_send(&method, &url, &headers, &body),
                    _ => Err(
                        "http_send: expected (method, url, headers, body) Strings".to_string(),
                    ),
                };
                frame.locals[dst.0 as usize] = match result {
                    Ok(body) => vm_ok(VmValue::String(body)),
                    Err(e) => vm_err(e),
                };
            }
            Instr::HttpServerListen { dst, port } => {
                let p = self.get_value(frame, *port)?;
                let result = match p {
                    VmValue::Int(port) => {
                        let addr = format!("0.0.0.0:{}", port as u16);
                        match TcpListener::bind(&addr) {
                            Ok(listener) => {
                                listener.set_nonblocking(true).ok();
                                let handle =
                                    VM_SERVER_COUNTER.fetch_add(1, Ordering::Relaxed);
                                let shutdown = Arc::new(AtomicBool::new(false));
                                VM_SERVER_REGISTRY
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .insert(
                                        handle,
                                        VmServerState {
                                            listener,
                                            routes: Vec::new(),
                                            shutdown,
                                        },
                                    );
                                Ok(Box::new(VmValue::Int(handle as i128)))
                            }
                            Err(e) => Err(Box::new(VmValue::String(format!(
                                "http_server_listen: bind failed: {e}"
                            )))),
                        }
                    }
                    _ => Err(Box::new(VmValue::String(
                        "http_server_listen: expected port Int".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::HttpServerServeLoop { server } => {
                let s = self.get_value(frame, *server)?;
                if let VmValue::Int(id) = s {
                    self.vm_serve_loop(id as u64)?;
                }
            }
            Instr::DbOpen { dst, path } => {
                let p = self.get_value(frame, *path)?;
                let result = match p {
                    VmValue::String(path) => (|| {
                        let conn = rusqlite::Connection::open(&path).map_err(|e| {
                            Box::new(VmValue::String(format!(
                                "cannot open database `{path}`: {e}"
                            )))
                        })?;
                        // Same busy-timeout discipline as the
                        // interpreter (Phase 5/M4 concurrent handlers
                        // share one database file).
                        conn.busy_timeout(std::time::Duration::from_millis(5000))
                            .map_err(|e| {
                                Box::new(VmValue::String(format!(
                                    "cannot set busy timeout on `{path}`: {e}"
                                )))
                            })?;
                        conn.execute_batch("PRAGMA journal_mode=WAL;").map_err(|e| {
                            Box::new(VmValue::String(format!(
                                "cannot set WAL mode on `{path}`: {e}"
                            )))
                        })?;
                        let mut reg = self.db.borrow_mut();
                        reg.next += 1;
                        let id = reg.next;
                        reg.conns.insert(id, conn);
                        vm_track_up(&VM_DB_OPEN, &VM_DB_PEAK);
                        Ok(Box::new(VmValue::Int(id as i128)))
                    })(),
                    _ => Err(Box::new(VmValue::String(
                        "db_open_builtin: expected path String".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::DbExec { dst, handle, sql, params } => {
                let h = self.get_value(frame, *handle)?;
                let s = self.get_value(frame, *sql)?;
                let p = self.get_value(frame, *params)?;
                let result = match (h, s, p) {
                    (VmValue::Int(id), VmValue::String(sql), VmValue::String(params)) => (|| {
                        let reg = self.db.borrow();
                        let conn = reg.conns.get(&(id as u64)).ok_or_else(|| {
                            Box::new(VmValue::String(format!(
                                "unknown database handle `{id}` (was it closed?)"
                            )))
                        })?;
                        let values = vm_parse_json_params(&params)
                            .map_err(|e| Box::new(VmValue::String(e)))?;
                        let changed = conn
                            .execute(&sql, rusqlite::params_from_iter(values))
                            .map_err(|e| {
                                Box::new(VmValue::String(format!("exec failed: {e}")))
                            })?;
                        Ok(Box::new(VmValue::Int(changed as i128)))
                    })(),
                    _ => Err(Box::new(VmValue::String(
                        "db_exec_builtin: expected handle Int, sql String, params JSON String"
                            .to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::DbQuery { dst, handle, sql, params } => {
                let h = self.get_value(frame, *handle)?;
                let s = self.get_value(frame, *sql)?;
                let p = self.get_value(frame, *params)?;
                let result = match (h, s, p) {
                    (VmValue::Int(id), VmValue::String(sql), VmValue::String(params)) => (|| {
                        let reg = self.db.borrow();
                        let conn = reg.conns.get(&(id as u64)).ok_or_else(|| {
                            Box::new(VmValue::String(format!(
                                "unknown database handle `{id}` (was it closed?)"
                            )))
                        })?;
                        let values = vm_parse_json_params(&params)
                            .map_err(|e| Box::new(VmValue::String(e)))?;
                        let mut stmt = conn.prepare(&sql).map_err(|e| {
                            Box::new(VmValue::String(format!("prepare failed: {e}")))
                        })?;
                        let width = stmt.column_count();
                        let rows = stmt
                            .query_map(rusqlite::params_from_iter(values), move |row| {
                                let mut cells = Vec::with_capacity(width);
                                for i in 0..width {
                                    let value: rusqlite::types::Value = row.get(i)?;
                                    cells.push(vm_render_json_value(&value));
                                }
                                Ok(format!("[{}]", cells.join(", ")))
                            })
                            .map_err(|e| {
                                Box::new(VmValue::String(format!("query failed: {e}")))
                            })?;
                        let mut out = Vec::new();
                        for row in rows {
                            out.push(VmValue::String(row.map_err(|e| {
                                Box::new(VmValue::String(format!("row decode failed: {e}")))
                            })?));
                        }
                        Ok(Box::new(VmValue::List(out)))
                    })(),
                    _ => Err(Box::new(VmValue::String(
                        "db_query_builtin: expected handle Int, sql String, params JSON String"
                            .to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::DbClose { dst, handle } => {
                let h = self.get_value(frame, *handle)?;
                let result = match h {
                    VmValue::Int(id) => {
                        let removed = self.db.borrow_mut().conns.remove(&(id as u64));
                        match removed {
                            Some(_) => {
                                VM_DB_OPEN.fetch_sub(1, Ordering::Relaxed);
                                Ok(Box::new(VmValue::Unit))
                            }
                            None => Err(Box::new(VmValue::String(format!(
                                "unknown database handle `{id}` (was it closed?)"
                            )))),
                        }
                    }
                    _ => Err(Box::new(VmValue::String(
                        "db_close_builtin: expected handle Int".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            // Phase 6/Wave 0 (ADR-020): monotonic millis since a
            // process-wide arbitrary epoch. Mirrors the
            // interpreter exactly — same epoch helper, same `Int`
            // result; no wall-clock interpretation.
            Instr::TimeMonoMs { dst } => {
                frame.locals[dst.0 as usize] = VmValue::Int(vm_mono_ms());
            }
            // Phase 6/Wave 0 (ADR-021): explicit-seed RNG over the
            // handle registry — values, not ambient authority. Same
            // SplitMix64 step and same error strings as
            // `interp/src/lib.rs::rng_next_u64`, so a seeded sequence
            // replays identically across `run` and `run-vm`.
            Instr::RngSeed { dst, seed } => {
                let s = self.get_value(frame, *seed)?;
                match s {
                    VmValue::Int(seed) => {
                        let mut reg = self.rng.borrow_mut();
                        reg.next += 1;
                        let id = reg.next;
                        reg.states.insert(id, seed as u64);
                        frame.locals[dst.0 as usize] = VmValue::Int(id as i128);
                    }
                    _ => {
                        return Err(VmError::TypeMismatch(
                            "rng_seed_builtin: expected seed Int".to_string(),
                        ));
                    }
                }
            }
            Instr::RngNext { dst, handle } => {
                let h = self.get_value(frame, *handle)?;
                match h {
                    VmValue::Int(id) => {
                        let mut reg = self.rng.borrow_mut();
                        match reg.states.get_mut(&(id as u64)) {
                            Some(state) => {
                                let value = vm_rng_next_u64(state);
                                frame.locals[dst.0 as usize] = VmValue::Int(value as i128);
                            }
                            None => {
                                return Err(VmError::TypeMismatch(format!(
                                    "unknown rng handle `{id}` (was it seeded?)"
                                )));
                            }
                        }
                    }
                    _ => {
                        return Err(VmError::TypeMismatch(
                            "rng_next_builtin: expected handle Int".to_string(),
                        ));
                    }
                }
            }
            // Phase 6/M5 (ADR-024): cooperative task cancellation. The
            // VM owns no task registry and has no spawn instruction —
            // `noct run-vm` refuses every program with a `task`
            // declaration before lowering (CONCURRENCY.md §7) — so no
            // live handle can exist here, and every handle is exactly
            // the "never spawned" case the interpreter refuses too.
            // The refusal text core is shared with the interpreter's
            // `Interpreter::task_cancel` (the pinned contract); only
            // the tail names which backend said it.
            Instr::TaskCancel { handle } => {
                let v = self.get_value(frame, *handle)?;
                let id = match v {
                    VmValue::Int(id) => id,
                    _ => {
                        return Err(VmError::TypeMismatch(
                            "task_cancel_builtin: expected handle Int".to_string(),
                        ));
                    }
                };
                return Err(VmError::UnsupportedByBackend(format!(
                    "cancel of unknown task handle {id} (never spawned, already awaited, or from a previous run; the NIR VM spawns no tasks — see CONCURRENCY.md §7)"
                )));
            }
            Instr::FsRead { dst, path } => {
                let p = self.get_value(frame, *path)?;
                let result = match p {
                    VmValue::String(path) => match std::fs::read_to_string(&path) {
                        Ok(contents) => Ok(Box::new(VmValue::String(contents))),
                        Err(e) => Err(Box::new(VmValue::String(e.to_string()))),
                    },
                    _ => Err(Box::new(VmValue::String(
                        "fs_read_text: expected path String".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::FsWrite { dst, path, contents } => {
                let p = self.get_value(frame, *path)?;
                let c = self.get_value(frame, *contents)?;
                let result = match (p, c) {
                    (VmValue::String(path), VmValue::String(contents)) => {
                        match std::fs::write(&path, &contents) {
                            Ok(()) => Ok(Box::new(VmValue::Unit)),
                            Err(e) => Err(Box::new(VmValue::String(e.to_string()))),
                        }
                    }
                    _ => Err(Box::new(VmValue::String(
                        "fs_write_text: expected path and contents Strings".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::FsListDir { dst, path } => {
                let p = self.get_value(frame, *path)?;
                let result = match p {
                    VmValue::String(path) => vm_list_dir_entries(&path)
                        .map(Box::new)
                        .map_err(|message| Box::new(VmValue::String(message))),
                    _ => Err(Box::new(VmValue::String(
                        "fs_list_dir: expected path String".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::FsModifiedMillis { dst, path } => {
                let p = self.get_value(frame, *path)?;
                let result = match p {
                    VmValue::String(path) => std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .map(|d| VmValue::Int(d.as_millis() as i128))
                                .map_err(|e| e.to_string())
                        })
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                        .map(Box::new)
                        .map_err(|e| Box::new(VmValue::String(e))),
                    _ => Err(Box::new(VmValue::String(
                        "fs_modified_millis: expected path String".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::EnvGet { dst, name } => {
                let n = self.get_value(frame, *name)?;
                let value = match n {
                    VmValue::String(n) => std::env::var(&n)
                        .ok()
                        .map(|v| Box::new(VmValue::String(v))),
                    _ => None,
                };
                frame.locals[dst.0 as usize] = VmValue::Option(value);
            }
            Instr::ConfigGet { dst, path, key } => {
                let p = self.get_value(frame, *path)?;
                let k = self.get_value(frame, *key)?;
                let result = match (p, k) {
                    (VmValue::String(path), VmValue::String(key)) => {
                        match std::fs::read_to_string(&path) {
                            Ok(contents) => {
                                let mut found = None;
                                for line in contents.lines() {
                                    let line = line.trim();
                                    if line.is_empty() || line.starts_with('#') {
                                        continue;
                                    }
                                    if let Some((n, v)) = line.split_once('=') {
                                        if n.trim() == key {
                                            found = Some(v.trim().to_string());
                                            break;
                                        }
                                    }
                                }
                                match found {
                                    Some(v) => Ok(Box::new(VmValue::String(v))),
                                    None => Err(Box::new(VmValue::String(format!(
                                        "configuration key `{key}` not found"
                                    )))),
                                }
                            }
                            Err(e) => Err(Box::new(VmValue::String(e.to_string()))),
                        }
                    }
                    _ => Err(Box::new(VmValue::String(
                        "config_get_builtin: expected path and key Strings".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::DotenvLoad { dst, path } => {
                let p = self.get_value(frame, *path)?;
                let result = match p {
                    VmValue::String(path) => vm_dotenv_load_file(&path)
                        .map(|n| Box::new(VmValue::Int(n)))
                        .map_err(|e| Box::new(VmValue::String(e))),
                    _ => Err(Box::new(VmValue::String(
                        "dotenv_load_builtin: expected path String".to_string(),
                    ))),
                };
                frame.locals[dst.0 as usize] = VmValue::Result(result);
            }
            Instr::CondBranch { cond, then_block, else_block } => {
                let cond_val = self.get_value(frame, *cond)?;
                let target = if cond_val.is_truthy() { *then_block } else { *else_block };
                frame.block = target;
                frame.pc = 0;
            }
            Instr::Unreachable => {
                return Err(VmError::Unreachable);
            }
            Instr::ResultOk { dst, val, ty: _ } => {
                let v = self.get_value(frame, *val)?;
                frame.locals[dst.0 as usize] = VmValue::Result(Ok(Box::new(v)));
            }
            Instr::ResultErr { dst, val, ty: _ } => {
                let v = self.get_value(frame, *val)?;
                frame.locals[dst.0 as usize] = VmValue::Result(Err(Box::new(v)));
            }
            Instr::TryUnwrap { dst, src, ty: _ } => {
                // Contract: lowering guarantees the operand is Some/Ok here
                // (`?` splits control flow *before* reaching this instruction,
                // routing None/Err to an early return). Hitting None/Err means
                // a lowering bug — trap loudly instead of continuing with a
                // silent Unit, which used to produce wrong values downstream.
                let src_val = self.get_value(frame, *src)?;
                let result = match src_val {
                    VmValue::Option(Some(v)) => *v,
                    VmValue::Option(None) => {
                        return Err(VmError::OptionUnwrapNone);
                    }
                    VmValue::Result(Ok(v)) => *v,
                    VmValue::Result(Err(e)) => {
                        return Err(VmError::ResultUnwrapErr(e));
                    }
                    other => {
                        return Err(VmError::TypeMismatch(format!("expected Option/Result, got {:?}", other)));
                    }
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::OptionSome { dst, val, ty: _ } => {
                let v = self.get_value(frame, *val)?;
                frame.locals[dst.0 as usize] = VmValue::Option(Some(Box::new(v)));
            }
            Instr::OptionNone { dst, ty: _ } => {
                frame.locals[dst.0 as usize] = VmValue::Option(None);
            }
            Instr::ToString { dst, src, .. } => {
                let v = self.get_value(frame, *src)?;
                let s = format!("{}", v);
                frame.locals[dst.0 as usize] = VmValue::String(s);
            }
            Instr::Phi { dst, incoming, ty: _ } => {
                // Contract: a Phi only exists at a merge point that by construction
                // has been reached via some predecessor recorded in `prev_block`,
                // and every predecessor that can reach this Phi must have a
                // corresponding `incoming` entry (this is `lowering.rs`'s job to
                // guarantee — every merge-block Phi's `incoming` vec is built
                // alongside the branches that target it). Previously this defaulted
                // to `VmValue::Unit` in both failure modes below, silently
                // continuing with wrong data instead of surfacing the lowering bug —
                // exactly the failure signature NIR.md §4.1 warns "most of them
                // execute without errors." Trap loudly instead, matching
                // `TryUnwrap`'s existing discipline.
                let prev = frame.prev_block.ok_or_else(|| {
                    VmError::MalformedCfg(format!(
                        "phi {} reached with no predecessor block recorded", dst
                    ))
                })?;
                let (val, _) = incoming.iter().find(|(_, b)| *b == prev).ok_or_else(|| {
                    VmError::MalformedCfg(format!(
                        "phi {} has no incoming entry for predecessor {}", dst, prev
                    ))
                })?;
                frame.locals[dst.0 as usize] = self.get_value(frame, *val)?;
            }
            Instr::ClosureNew { dst, func, captured, ty: _ } => {
                let cap_vals: Vec<VmValue> = captured.iter()
                    .map(|c| self.get_value(frame, *c))
                    .collect::<Result<Vec<_>, _>>()?;
                frame.locals[dst.0 as usize] = VmValue::Closure { func: *func, captured: cap_vals };
            }
            Instr::ClosureCall { dst, closure, args, ret_ty: _ } => {
                let closure_val = self.get_value(frame, *closure)?;
                if let VmValue::Closure { func, captured } = closure_val {
                    let mut full_args = captured.clone();
                    for arg in args {
                        full_args.push(self.get_value(frame, *arg)?);
                    }
                    let result = self.call(func, full_args)?;
                    frame.locals[dst.0 as usize] = result;
                } else {
                    frame.locals[dst.0 as usize] = VmValue::Unit;
                }
            }
            Instr::Branch { target } => {
                let prev = frame.block;
                frame.block = *target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
            }
            Instr::Switch { .. } => {
                return Err(VmError::UnimplementedTerminator("Switch used as instruction, not terminator".to_string()));
            }
            Instr::Return { .. } => {}
            Instr::EarlyReturn { .. } => {
                return Err(VmError::Unreachable);
            }
        }
        Ok(())
    }

    fn execute_terminator(&mut self, term: &Instr, frame: &mut CallFrame) -> Result<ControlFlow, VmError> {
        match term {
            Instr::Return { val } => {
                let result = if let Some(v) = val {
                    self.get_value(frame, *v)?
                } else {
                    VmValue::Unit
                };
                // Propagate to caller if there is one
                let return_dst = frame.return_dst;
                self.call_stack.pop();
                if let Some(caller_frame) = self.call_stack.last_mut() {
                    if let Some(dst) = return_dst {
                        caller_frame.locals[dst.0 as usize] = result.clone();
                    }
                }
                Ok(ControlFlow::Return(result))
            }
            Instr::EarlyReturn { val } => {
                // Early return (for ? operator) - same as Return but explicit
                let result = self.get_value(frame, *val)?;
                let return_dst = frame.return_dst;
                self.call_stack.pop();
                if let Some(caller_frame) = self.call_stack.last_mut() {
                    if let Some(dst) = return_dst {
                        caller_frame.locals[dst.0 as usize] = result.clone();
                    }
                }
                Ok(ControlFlow::Return(result))
            }
            Instr::Branch { target } => {
                let prev = frame.block;
                frame.block = *target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
                Ok(ControlFlow::Continue)
            }
            Instr::CondBranch { cond, then_block, else_block } => {
                let cond_val = self.get_value(frame, *cond)?;
                let target = if cond_val.is_truthy() { *then_block } else { *else_block };
                let prev = frame.block;
                frame.block = target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
                Ok(ControlFlow::Continue)
            }
            Instr::Switch { val, cases, default } => {
                let val_val = self.get_value(frame, *val)?;
                let tag = if let VmValue::Int(i) = val_val { i as u32 } else { 0 };
                let target = cases.iter()
                    .find(|(t, _)| *t == tag)
                    .map(|(_, b)| *b)
                    .unwrap_or(*default);
                let prev = frame.block;
                frame.block = target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
                Ok(ControlFlow::Continue)
            }
            Instr::Unreachable => {
                Err(VmError::Unreachable)
            }
            _ => Err(VmError::UnimplementedTerminator(format!("{:?}", term))),
        }
    }

    fn get_value(&self, frame: &CallFrame, id: ValueId) -> Result<VmValue, VmError> {
        let idx = id.0 as usize;
        if idx < frame.locals.len() {
            Ok(frame.locals[idx].clone())
        } else {
            Err(VmError::InvalidValueId(id))
        }
    }

    fn add_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a + b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a + b)),
            (VmValue::String(a), VmValue::String(b)) => Ok(VmValue::String(a + &b)),
            _ => Err(VmError::TypeMismatch("add".to_string())),
        }
    }

    fn sub_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a - b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a - b)),
            _ => Err(VmError::TypeMismatch("sub".to_string())),
        }
    }

    fn mul_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a * b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a * b)),
            _ => Err(VmError::TypeMismatch("mul".to_string())),
        }
    }

    fn div_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => {
                if b == 0 { return Err(VmError::DivisionByZero); }
                Ok(VmValue::Int(a / b))
            }
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a / b)),
            _ => Err(VmError::TypeMismatch("div".to_string())),
        }
    }

    fn rem_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a % b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a % b)),
            _ => Err(VmError::TypeMismatch("rem".to_string())),
        }
    }

    fn icmp_values(&self, op: CmpOp, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        // Mirrors the interpreter's `values_equal` / `value_cmp` exactly:
        // structural equality for Eq/Ne (NOT just numerics — previously
        // everything non-numeric silently compared `false`), ordering for
        // Int/Float (mixed included), Char, and String. Anything else is a
        // runtime error, matching the interpreter's panic — never a quiet
        // `false`.
        let result = match (op, a, b) {
            (CmpOp::Eq, x, y) => Self::values_equal(&x, &y),
            (CmpOp::Ne, x, y) => !Self::values_equal(&x, &y),
            (CmpOp::Lt, x, y) => Self::value_cmp(&x, &y)? == std::cmp::Ordering::Less,
            (CmpOp::Le, x, y) => matches!(Self::value_cmp(&x, &y)?, std::cmp::Ordering::Less | std::cmp::Ordering::Equal),
            (CmpOp::Gt, x, y) => Self::value_cmp(&x, &y)? == std::cmp::Ordering::Greater,
            (CmpOp::Ge, x, y) => matches!(Self::value_cmp(&x, &y)?, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal),
        };
        Ok(VmValue::Bool(result))
    }

    fn values_equal(a: &VmValue, b: &VmValue) -> bool {
        match (a, b) {
            (VmValue::Int(x), VmValue::Int(y)) => x == y,
            (VmValue::Float(x), VmValue::Float(y)) => x == y,
            (VmValue::Bool(x), VmValue::Bool(y)) => x == y,
            (VmValue::Char(x), VmValue::Char(y)) => x == y,
            (VmValue::String(x), VmValue::String(y)) => x == y,
            (VmValue::Unit, VmValue::Unit) => true,
            (VmValue::Enum { variant: va, fields: fa, .. }, VmValue::Enum { variant: vb, fields: fb, .. }) => {
                va == vb && fa.len() == fb.len() && fa.iter().zip(fb.iter()).all(|(x, y)| Self::values_equal(x, y))
            }
            (VmValue::Option(x), VmValue::Option(y)) => match (x, y) {
                (None, None) => true,
                (Some(x), Some(y)) => Self::values_equal(x, y),
                _ => false,
            },
            (VmValue::List(x), VmValue::List(y)) => {
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(x, y)| Self::values_equal(x, y))
            }
            // NOTE: mirrors the interpreter, which has no Result arm either
            // (Result == Result is always false there too).
            _ => false,
        }
    }

    fn value_cmp(a: &VmValue, b: &VmValue) -> Result<std::cmp::Ordering, VmError> {
        match (a, b) {
            (VmValue::Int(x), VmValue::Int(y)) => Ok(x.cmp(y)),
            (VmValue::Float(x), VmValue::Float(y)) => {
                x.partial_cmp(y).ok_or_else(|| VmError::TypeMismatch("comparison of NaN".to_string()))
            }
            (VmValue::Float(x), VmValue::Int(y)) => {
                (*x).partial_cmp(&(*y as f64)).ok_or_else(|| VmError::TypeMismatch("comparison of NaN".to_string()))
            }
            (VmValue::Int(x), VmValue::Float(y)) => {
                (*x as f64).partial_cmp(y).ok_or_else(|| VmError::TypeMismatch("comparison of NaN".to_string()))
            }
            (VmValue::Char(x), VmValue::Char(y)) => Ok(x.cmp(y)),
            (VmValue::String(x), VmValue::String(y)) => Ok(x.cmp(y)),
            _ => Err(VmError::TypeMismatch("values are not comparable".to_string())),
        }
    }

    fn fcmp_values(&self, op: CmpOp, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        self.icmp_values(op, a, b)
    }

    fn neg_value(&self, a: VmValue) -> Result<VmValue, VmError> {
        match a {
            VmValue::Int(i) => Ok(VmValue::Int(-i)),
            VmValue::Float(f) => Ok(VmValue::Float(-f)),
            _ => Err(VmError::TypeMismatch("neg".to_string())),
        }
    }

    fn not_value(&self, a: VmValue) -> Result<VmValue, VmError> {
        match a {
            VmValue::Bool(b) => Ok(VmValue::Bool(!b)),
            VmValue::Int(i) => Ok(VmValue::Int(!i)),
            _ => Err(VmError::TypeMismatch("not".to_string())),
        }
    }

    fn const_to_value(&self, c: &ConstValue) -> Result<VmValue, VmError> {
        match c {
            ConstValue::Int(v) => Ok(VmValue::Int(*v)),
            ConstValue::Float(v) => Ok(VmValue::Float(*v)),
            ConstValue::Bool(v) => Ok(VmValue::Bool(*v)),
            ConstValue::Char(c) => Ok(VmValue::Char(*c)),
            ConstValue::String(s) => Ok(VmValue::String(s.clone())),
            ConstValue::Unit => Ok(VmValue::Unit),
        }
    }

    // ── Phase 5/M4 host-IO helpers ──────────────────────────────────────
    // Both mirror `interp/src/lib.rs` exactly (same request bytes,
    // same response slicing, same handler-body contract).

    /// Blocking plaintext-HTTP client over TCP (mirrors the
    /// interpreter's `http_send_builtin` byte-for-byte: header pairs
    /// are 2-lists of Strings, non-pair entries are skipped, a
    /// missing Content-Type is supplied for non-empty bodies).
    /// Returns the response BODY text only.
    fn vm_http_send(
        &self,
        method: &str,
        url: &str,
        headers: &[VmValue],
        body: &str,
    ) -> Result<String, String> {
        let (host, port, path) = vm_parse_http_url(url)
            .ok_or_else(|| format!("unsupported URL scheme (expected http://): {url}"))?;
        let mut pairs: Vec<(String, String)> = Vec::new();
        for header in headers {
            if let VmValue::List(pair) = header {
                if pair.len() == 2 {
                    if let (VmValue::String(k), VmValue::String(v)) = (&pair[0], &pair[1]) {
                        pairs.push((k.clone(), v.clone()));
                    }
                }
            }
        }
        if !body.is_empty() && !pairs.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
            pairs.push(("Content-Type".to_string(), "text/plain".to_string()));
        }
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n");
        for (k, v) in &pairs {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");
        req.push_str(body);
        let mut conn = std::net::TcpStream::connect((host.as_str(), port))
            .map_err(|e| format!("connection failed: {e}"))?;
        std::io::Write::write_all(&mut conn, req.as_bytes())
            .map_err(|e| format!("write failed: {e}"))?;
        let mut response = Vec::new();
        std::io::Read::read_to_end(&mut conn, &mut response)
            .map_err(|e| format!("read failed: {e}"))?;
        vm_split_http_body(&response)
            .ok_or_else(|| "malformed HTTP response: missing header terminator".to_string())
    }

    /// Blocking HTTP accept loop (mirrors the interpreter's
    /// `http_server_serve_loop` through the shared
    /// `crate::http_wire` layer: same limits, same codes, same byte
    /// shapes — only handler invocation differs, running nested
    /// frames via `call` on a per-connection worker VM). One OS
    /// thread per connection; the accept loop never blocks on a
    /// handler. Handler names resolve against the worker's module;
    /// unknown names and non-String results answer 500. Shutdown
    /// stops accepting and drains in-flight connections before
    /// returning.
    fn vm_serve_loop(&mut self, handle: u64) -> Result<(), VmError> {
        let (shutdown_flag, listener) = {
            let reg = VM_SERVER_REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            match reg.get(&handle) {
                Some(state) => match state.listener.try_clone() {
                    Ok(l) => (state.shutdown.clone(), l),
                    Err(_) => return Ok(()),
                },
                None => return Ok(()),
            }
        };
        listener.set_nonblocking(true).ok();
        let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
        loop {
            if shutdown_flag.load(Ordering::Relaxed) {
                break;
            }
            match listener.accept() {
                Ok((conn, _)) => {
                    // Blocking mode for the wire layer's read timeouts
                    // (mirrors the interpreter: accepted sockets
                    // inherit the nonblocking listener).
                    let _ = conn.set_nonblocking(false);
                    let routes = {
                        let reg = VM_SERVER_REGISTRY
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        match reg.get(&handle) {
                            Some(state) => state.routes.clone(),
                            None => break,
                        }
                    };
                    let worker = Vm::new(self.module.clone());
                    // 8 MiB stacks, matching the interpreter's workers:
                    // handlers run at `main`-depth call budgets.
                    workers.push(
                        std::thread::Builder::new()
                            .stack_size(8 * 1024 * 1024)
                            .spawn(move || {
                                vm_track_up(&VM_CONN_THREADS, &VM_CONN_PEAK);
                                vm_serve_one_connection(worker, conn, &routes);
                                VM_CONN_THREADS.fetch_sub(1, Ordering::Relaxed);
                            })
                            .expect("spawn connection worker"),
                    );
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        for w in workers {
            w.join().ok();
        }
        Ok(())
    }
}

enum ControlFlow {
    Continue,
    Return(VmValue),
}

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("no main function found")]
    NoMainFunction,
    #[error("function {0:?} not found")]
    FunctionNotFound(FuncId),
    #[error("no entry block for function {0:?}")]
    NoEntryBlock(FuncId),
    #[error("no terminator for block {0:?} in function {1}")]
    NoTerminator(BlockId, String),
    #[error("invalid value ID {0:?}")]
    InvalidValueId(ValueId),
    #[error("type mismatch in {0}")]
    TypeMismatch(String),
    #[error("division by zero")]
    DivisionByZero,
    #[error("unimplemented terminator: {0}")]
    UnimplementedTerminator(String),
    #[error("unreachable code executed")]
    Unreachable,
    #[error("option unwrap on None")]
    OptionUnwrapNone,
    #[error("result unwrap on Err")]
    ResultUnwrapErr(Box<VmValue>),
    /// A Phi (or, in the future, any other CFG-shape-dependent instruction)
    /// was reached in a state its `incoming`/predecessor bookkeeping
    /// doesn't cover — a lowering bug, never a valid program. Mirrors
    /// `TryUnwrap`'s existing "trap loudly, never default" discipline,
    /// which this variant previously lacked (see landmine (c)).
    #[error("malformed CFG: {0}")]
    MalformedCfg(String),
    /// The program reached an operation this backend cannot execute.
    /// Today: cooperative task cancellation, which needs the task
    /// registry the VM does not have (it spawns no tasks, and
    /// `noct run-vm` refuses programs carrying a `task` declaration —
    /// CONCURRENCY.md §7). Raised loudly by construction: the
    /// alternative, treating an unspawnable handle as a successful
    /// cancel, would report a cancellation that never happened.
    #[error("unsupported by this backend: {0}")]
    UnsupportedByBackend(String),
    /// A managed-heap (ARC) operation failed at runtime: a refcount
    /// bump or drop on a dead object, a `Weak`/`Unowned` of the wrong
    /// shape, or a dangling `Unowned` dereference. Mirrors the
    /// interpreter's `RuntimeError::Panic` for the same conditions —
    /// these are program errors, not backend limits, which is why they
    /// do NOT reuse `UnsupportedByBackend`. Loud by construction: a
    /// `Weak` argument that was not a `Weak` used to answer `None`,
    /// and a non-`Arc` argument to `ArcRetain` used to do nothing at
    /// all, both of which are silent wrong answers.
    #[error("managed heap: {0}")]
    ManagedPanic(String),
}
