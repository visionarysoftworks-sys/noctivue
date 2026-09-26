//! Phase 5/M4 production-shape tests (`docs/PHASE5_PRODUCTION.md`).
//!
//! - Wire limits: over-cap headers/bodies answer 413 with no handler
//!   invocation; malformed shapes answer 400 and the server survives;
//!   non-String handlers answer 500 (never the old 200
//!   `"handler error"` body).
//! - Concurrency: slow handlers overlap across connections (the
//!   accept loop never blocks on a handler).
//! - Load harness (§5): the reference CRUD app (public stdlib only,
//!   binding-only SQL, task-local pools) under N=16 clients × M=128
//!   requests mixing create/read/update/delete plus unknown-route
//!   (404) and malformed (400) trickles, then pool accounting,
//!   `integrity_check`, spot reads, and a second consecutive cycle.
//!   RSS trend is approximated without new tooling: identical second
//!   cycle plus zero thread/connection deltas (Windows-compatible,
//!   no ASan — the §5c leak pass by equivalent means).
//!
//! Server-touching tests serialize on `SERIAL` (one binary runs its
//! tests on threads; live-connection/db counters are process-global).

use compiler::diagnostics::DiagnosticSink;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

const STDLIB_LIBS: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/net/http/request.nv",
    "stdlib/net/http/response.nv",
    "stdlib/net/http/client.nv",
    "stdlib/net/http/server.nv",
    "stdlib/db/sqlite.nv",
    "stdlib/db/pool.nv",
];

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

fn join(sources: &[String]) -> String {
    let mut out = String::new();
    for s in sources {
        out.push_str(s);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn error_texts(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let code = d.code.as_deref().unwrap_or("?");
            format!("[{code}] {}", d.message)
        })
        .collect()
}

fn check(source: &str, what: &str) -> compiler::hir::Module {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let module = compiler::typeck::typecheck(program, &mut sink);
    let errors = error_texts(&sink);
    assert!(
        errors.is_empty(),
        "{what} produced error diagnostics:\n  {}",
        errors.join("\n  ")
    );
    module
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral bind")
        .local_addr()
        .expect("local addr")
        .port()
}

/// One raw request → full close-delimited response text.
fn raw_request(port: u16, bytes: &[u8]) -> String {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    conn.set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    conn.write_all(bytes).expect("write request");
    let mut out = Vec::new();
    conn.read_to_end(&mut out).expect("read response");
    String::from_utf8_lossy(&out).to_string()
}

fn status_line(resp: &str) -> &str {
    resp.lines().next().unwrap_or("")
}

fn response_body(resp: &str) -> &str {
    match resp.find("\r\n\r\n") {
        Some(i) => &resp[i + 4..],
        None => "",
    }
}

/// GET via raw TCP with a 5 s watchdog (fails fast, never hangs the
/// suite when the server misbehaves).
fn raw_get(port: u16, path: &str) -> String {
    raw_request(
        port,
        format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes(),
    )
}

fn wait_healthy(port: u16, path: &str, want_body: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(conn) = TcpStream::connect(("127.0.0.1", port)) {
            drop(conn);
            let resp = raw_get(port, path);
            if response_body(&resp) == want_body {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "server on {port} never became healthy"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

const SMALL_BUDGET_MS: i32 = 6000;

fn small_server_source(port: u16, routes: &str, handlers: &str) -> String {
    let libs: Vec<String> = STDLIB_LIBS.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"{handlers}

task serve(srv: Int):
    http_server_serve(srv)

fn main():
    match http_server_listen({port}):
        Ok(srv):
{routes}
            let t = serve(srv)
            sleep_builtin({budget})
            http_server_stop(srv)
            await t
        Err(e):
            assert(false, "listen failed")
"#,
        handlers = handlers,
        port = port,
        routes = routes,
        budget = SMALL_BUDGET_MS,
    ));
    join(&sources)
}

fn run_server(module: compiler::hir::Module) -> std::thread::JoinHandle<(i32, Vec<String>)> {
    std::thread::spawn(move || {
        let mut sink = DiagnosticSink::new();
        let mut interp = interp::Interpreter::new();
        let exit = interp.run(&module, &mut sink);
        (exit, error_texts(&sink))
    })
}

fn join_server(handle: std::thread::JoinHandle<(i32, Vec<String>)>, what: &str) {
    let (exit, errors) = handle.join().expect("server thread");
    assert_eq!(exit, 0, "[{what}] server exit {exit}");
    assert!(
        errors.is_empty(),
        "[{what}] server diagnostics:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn limits_413_without_handler_and_400_shapes() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"POST\", \"/echo\", \"echo_handler\")",
            "fn echo_handler(body: String) -> String:\n    \"echo:{body}\"",
        ),
        "limits server",
    );
    let server = run_server(module);
    wait_healthy(port, "/missing", "Not Found");

    // Over-cap body: declared past 1 MiB → 413, handler never runs
    // (an echo body would start with "echo:").
    let big = format!(
        "POST /echo HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        1024 * 1024 + 1
    );
    let resp = raw_request(port, big.as_bytes());
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 413"),
        "over-body status: {resp:?}"
    );
    assert!(
        response_body(&resp).contains("Payload Too Large"),
        "over-body reason: {resp:?}"
    );

    // Over-cap headers: > 8 KiB of header block → 413.
    let mut huge = format!("GET /echo HTTP/1.1\r\n");
    while huge.len() < 9000 {
        huge.push_str("X-Pad: abcdefgh\r\n");
    }
    huge.push_str("\r\n");
    let resp = raw_request(port, huge.as_bytes());
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 413"),
        "over-header status: {resp:?}"
    );

    // Malformed shapes → 400, each with a terminal response.
    for bad in [
        "GARBAGE\r\n\r\n",
        "GET /only-two\r\n\r\n",
        "GET /x HTTP/1.1 EXTRA\r\n\r\n",
        "POST /echo HTTP/1.1\r\nContent-Length: many\r\n\r\n",
        "POST /echo HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\nabcde",
    ] {
        let resp = raw_request(port, bad.as_bytes());
        assert!(
            status_line(&resp).starts_with("HTTP/1.1 400"),
            "malformed {bad:?} status: {resp:?}"
        );
    }
    // NUL byte in the request line → 400.
    let resp = raw_request(port, b"GE\x00T /x HTTP/1.1\r\n\r\n");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 400"),
        "NUL status: {resp:?}"
    );

    // Server survives everything above: valid traffic still 200.
    let resp = raw_request(port, b"POST /echo HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 200"),
        "post-fuzz status: {resp:?}"
    );
    assert_eq!(response_body(&resp), "echo:abc");

    join_server(server, "limits");
}

#[test]
fn non_string_handler_answers_500() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"GET\", \"/bad\", \"bad_handler\")",
            "fn bad_handler(body: String) -> Int:\n    42",
        ),
        "non-string handler server",
    );
    let server = run_server(module);
    wait_healthy(port, "/missing", "Not Found");
    let resp = raw_get(port, "/bad");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 500"),
        "non-string status: {resp:?}"
    );
    assert!(
        response_body(&resp).contains("Internal Server Error"),
        "non-string body: {resp:?}"
    );
    join_server(server, "non-string handler");
}

#[test]
fn concurrent_slow_handlers_overlap() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"GET\", \"/slow\", \"slow_handler\")",
            "fn slow_handler(body: String) -> String:\n    sleep_builtin(400)\n    \"slow\"",
        ),
        "slow server",
    );
    let server = run_server(module);
    // Baseline BEFORE any traffic: the health poll's own worker has
    // not yet run, so no race with its teardown decrement.
    let base = interp::live_connection_threads();
    wait_healthy(port, "/missing", "Not Found");
    // Deterministic overlap proof (no wall-time race): both clients
    // release simultaneously onto 400 ms handlers, the main thread
    // samples live worker threads mid-flight, and a generous wall
    // bound stands guard against hangs only.
    let gate = std::sync::Arc::new(Barrier::new(3));
    let start = Instant::now();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                let resp = raw_get(port, "/slow");
                assert!(status_line(&resp).starts_with("HTTP/1.1 200"));
                assert_eq!(response_body(&resp), "slow");
            })
        })
        .collect();
    gate.wait();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        interp::live_connection_threads() >= base + 2,
        "handlers did not overlap mid-flight"
    );
    for h in handles {
        h.join().expect("client thread");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(15),
        "overlap run hung ({elapsed:?})"
    );
    join_server(server, "slow overlap");
}

#[test]
fn db_integer_overflow_rejected() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let libs: Vec<String> = STDLIB_LIBS.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(
        r#"fn main():
    match db_open(":memory:"):
        Ok(db): setup(db)
        Err(e): assert(false, "open failed")

fn setup(db: Db) -> Unit:
    match db_exec(db, "CREATE TABLE t(n INT)", "[]"):
        Ok(_): insert_plain(db)
        Err(e): assert(false, "create failed")

fn insert_plain(db: Db) -> Unit:
    match db_exec(db, "INSERT INTO t(n) VALUES (?)", "[42]"):
        Ok(_): insert_huge(db)
        Err(e): assert(false, "plain insert failed")

fn insert_huge(db: Db) -> Unit:
    match db_exec(db, "INSERT INTO t(n) VALUES (?)", "[99999999999999999999999]"):
        Ok(_): close_then_fail(db)
        Err(e): close_then_check(db, e)

fn close_then_fail(db: Db) -> Unit:
    match db_close(db):
        Ok(_): assert(false, "overflow insert passed")
        Err(e): assert(false, "close failed")

fn close_then_check(db: Db, e: String) -> Unit:
    match db_close(db):
        Ok(_): assert_true(string_contains(e, "out of range"), "names range")
        Err(c): assert(false, "close failed")
"#
        .to_string(),
    );
    let module = check(&join(&sources), "overflow");
    let mut sink = DiagnosticSink::new();
    let mut interp = interp::Interpreter::new();
    let exit = interp.run(&module, &mut sink);
    assert_eq!(exit, 0, "overflow run failed: {:?}", error_texts(&sink));
}

// ── Reference CRUD app + §5 load harness ────────────────────────────────────
//
// The app lives in `examples/reference-crud/lib/main.nv` (the
// human-runnable original); the harness derives its copy from that
// file with three token substitutions (temp database, ephemeral
// port, shorter budget) so proof and package can never diverge.
// The app uses public stdlib only, binding-only SQL (values bind
// from JSON arrays — a self-grep below pins that no handler
// interpolates into SQL text), and task-local pools: every handler
// opens, uses, and closes its own pool on its connection thread
// (PHASE5_PRODUCTION.md §1).

/// Load the reference app source with harness parameters baked in.
fn load_app_source(db_path: &str, port: u16, budget_ms: i32) -> String {
    let src = read("examples/reference-crud/lib/main.nv");
    src.replace("crud.db", db_path)
        .replace("18080", &port.to_string())
        .replace("60000", &budget_ms.to_string())
}
fn build_app(db_path: &str, port: u16, budget_ms: i32) -> String {
    let libs: Vec<String> = STDLIB_LIBS.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(load_app_source(db_path, port, budget_ms));
    join(&sources)
}

/// No handler may interpolate values into SQL text (PHASE5 §4:
/// binding is the only value path — convention + test, run against
/// the packaged app source so proof and package cannot diverge).
#[test]
fn reference_app_binds_never_interpolates() {
    let src = read("examples/reference-crud/lib/main.nv");
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("match db_exec(") || t.starts_with("match db_query(") {
            assert!(
                !t.contains("VALUES ('") && !t.contains("= '{"),
                "interpolated SQL value: {t}"
            );
        }
    }
    // Every value-carrying statement uses placeholders.
    for want in [
        "VALUES (?)",
        "SET name = ? WHERE id = ?",
        "DELETE FROM items WHERE id = ?",
    ] {
        assert!(src.contains(want), "missing placeholder statement: {want}");
    }
}

const LOAD_CLIENTS: usize = 16;
const LOAD_ITERS: usize = 128;
const LOAD_BUDGET_MS: i32 = 60000;
const REQ_TIMEOUT: Duration = Duration::from_secs(10);

fn load_db_path(cycle: u32) -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("noctivue-load-{}-{cycle}.db", std::process::id()));
    p.to_string_lossy().replace('\\', "/")
}

/// Timed raw POST with an exact body.
fn raw_post(port: u16, path: &str, body: &str) -> Result<String, String> {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    conn.set_read_timeout(Some(REQ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    conn.set_write_timeout(Some(REQ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    conn.write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut out = Vec::new();
    conn.read_to_end(&mut out)
        .map_err(|e| format!("read: {e}"))?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// Timed raw GET body (status must be 200).
fn load_get(port: u16, path: &str) -> Result<String, String> {
    let resp = raw_request_timed(port, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"))?;
    if !status_line(&resp).starts_with("HTTP/1.1 200") {
        return Err(format!("unexpected status: {}", status_line(&resp)));
    }
    Ok(response_body(&resp).to_string())
}

fn raw_request_timed(port: u16, text: &str) -> Result<String, String> {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    conn.set_read_timeout(Some(REQ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    conn.set_write_timeout(Some(REQ_TIMEOUT))
        .map_err(|e| e.to_string())?;
    conn.write_all(text.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut out = Vec::new();
    conn.read_to_end(&mut out)
        .map_err(|e| format!("read: {e}"))?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// §5 latency report: p50/p99 per endpoint in milliseconds,
/// printed for the record and never thresholded (the M4 floor is
/// blocking threads — gating on tuning would gate on the wrong
/// thing, so this function asserts nothing).
fn print_latency_report(cycle: u32, lat: &[(&'static str, u128)]) {
    for kind in ["create", "read", "update", "delete", "unknown", "malformed"] {
        let mut v: Vec<u128> = lat
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, t)| *t)
            .collect();
        if v.is_empty() {
            println!("cycle {cycle} latency {kind}: no samples");
            continue;
        }
        v.sort_unstable();
        let at = |q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)] as f64 / 1000.0;
        println!(
            "cycle {cycle} latency {kind}: n={} p50={:.1}ms p99={:.1}ms",
            v.len(),
            at(0.50),
            at(0.99)
        );
    }
}

fn run_load_cycle(cycle: u32) {
    let db_path = load_db_path(cycle);
    let _ = std::fs::remove_file(&db_path);
    let port = free_port();
    let module = check(&build_app(&db_path, port, LOAD_BUDGET_MS), "reference app");
    assert_eq!(
        interp::live_connection_threads(),
        0,
        "threads leak into cycle {cycle}"
    );
    assert_eq!(
        interp::live_db_connections(),
        0,
        "db handles leak into cycle {cycle}"
    );
    let server = std::thread::spawn(move || {
        let mut sink = DiagnosticSink::new();
        let mut interp = interp::Interpreter::new();
        let exit = interp.run(&module, &mut sink);
        (exit, error_texts(&sink))
    });
    wait_healthy(port, "/health", "ok");

    // One request per client first (warms routes), then the storm.
    // A barrier releases all clients at once so overlap is
    // structural, and peak gauges witness it (no timing luck).
    // Clients also record per-endpoint latencies for the §5 report
    // (recorded, never thresholded — the M4 floor is blocking
    // threads, so gating on tuning would be gating on the wrong
    // thing).
    let gate = std::sync::Arc::new(Barrier::new(LOAD_CLIENTS));
    let storms: Vec<_> = (0..LOAD_CLIENTS)
        .map(|i| {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                let mut lat: Vec<(&'static str, u128)> = Vec::new();
                let mut first_failure: Option<String> = None;
                for j in 0..LOAD_ITERS {
                    if first_failure.is_some() {
                        break;
                    }
                    let t0 = Instant::now();
                    let mut kind = "other";
                    let r: Result<(), String> = (|| {
                        if j % 64 == 63 {
                            // Malformed trickle (NUL variant): 400, survives.
                            kind = "malformed";
                            let resp = raw_request_timed(port, "GE\x00T /x HTTP/1.1\r\n\r\n")?;
                            if !status_line(&resp).starts_with("HTTP/1.1 400") {
                                return Err(format!("NUL status: {}", status_line(&resp)));
                            }
                        } else if j % 32 == 31 {
                            kind = "malformed";
                            let resp = raw_request_timed(port, "GARBAGE\r\n\r\n")?;
                            if !status_line(&resp).starts_with("HTTP/1.1 400") {
                                return Err(format!("garbage status: {}", status_line(&resp)));
                            }
                        } else if j % 16 == 15 {
                            kind = "unknown";
                            let resp =
                                raw_request_timed(port, "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n")?;
                            if !status_line(&resp).starts_with("HTTP/1.1 404") {
                                return Err(format!(
                                    "unknown route status: {}",
                                    status_line(&resp)
                                ));
                            }
                            if response_body(&resp) != "Not Found" {
                                return Err(format!(
                                    "unknown route body: {:?}",
                                    response_body(&resp)
                                ));
                            }
                        } else {
                            // Read-heavy CRUD mix (writes serialize in
                            // SQLite; reads fly in WAL — the mandated
                            // mix without a write convoy).
                            match j % 8 {
                                0 => {
                                    kind = "create";
                                    let name = format!("c{cycle}t{i}_{j}");
                                    let body = raw_post(port, "/items", &format!("name={name}"))?;
                                    let b = response_body(&body);
                                    if !(b.starts_with('[') && b.ends_with(']')) {
                                        return Err(format!("create body: {b:?}"));
                                    }
                                }
                                1 | 2 | 3 | 4 | 5 => {
                                    kind = "read";
                                    load_get(port, "/items")?;
                                }
                                6 => {
                                    kind = "update";
                                    let id = (i * 37 + j) % 64 + 1;
                                    let body = raw_post(
                                        port,
                                        "/update",
                                        &format!("id={id}&name=u{i}_{j}"),
                                    )?;
                                    if response_body(&body) != "ok" {
                                        return Err(format!("update body: {body:?}"));
                                    }
                                }
                                _ => {
                                    kind = "delete";
                                    let body =
                                        raw_post(port, "/delete", &format!("id={}", 900000 + i))?;
                                    if response_body(&body) != "ok" {
                                        return Err(format!("delete body: {body:?}"));
                                    }
                                }
                            }
                        }
                        Ok(())
                    })();
                    if r.is_ok() {
                        lat.push((kind, t0.elapsed().as_micros()));
                    }
                    if let Err(e) = r {
                        first_failure = Some(format!("client {i} iter {j}: {e}"));
                    }
                }
                // Handlers sleep 1 ms per request below; the barrier
                // keeps this thread's storm dense so overlap is certain.
                (first_failure, lat)
            })
        })
        .collect();

    let mut failures = Vec::new();
    let mut lat_all: Vec<(&'static str, u128)> = Vec::new();
    for s in storms {
        let (failure, mut lat) = s.join().expect("client thread");
        if let Some(e) = failure {
            failures.push(e);
        }
        lat_all.append(&mut lat);
    }
    assert!(
        failures.is_empty(),
        "load failures:\n  {}",
        failures.join("\n  ")
    );
    print_latency_report(cycle, &lat_all);

    // Peak gauges prove the storm really overlapped (barrier
    // release makes simultaneous flight structural, not lucky).
    assert!(
        interp::peak_connection_threads() >= 2,
        "no concurrent connection threads witnessed"
    );
    assert!(
        interp::peak_db_connections() >= 1,
        "no db handles witnessed mid-storm"
    );

    // §5a accounting: pool mechanics intact after the storm.
    let pool = load_get(port, "/poolcheck").expect("poolcheck");
    assert_eq!(pool, "1", "poolcheck body");
    // File-backed integrity + spot reads of every created row.
    let integrity = load_get(port, "/integrity").expect("integrity");
    assert!(
        integrity.contains("ok"),
        "integrity_check body: {integrity:?}"
    );
    let all = load_get(port, "/items").expect("final list");
    // Every row holds a name this cycle wrote (creates can be
    // renamed by updates, deletes target nonexistent ids): row
    // count proves nothing was lost, set membership proves nothing
    // was corrupted.
    let mut written = std::collections::HashSet::new();
    for i in 0..LOAD_CLIENTS {
        for j in (0..LOAD_ITERS).filter(|j| j % 8 == 0) {
            written.insert(format!("c{cycle}t{i}_{j}"));
        }
        for j in (0..LOAD_ITERS).filter(|j| j % 8 == 6) {
            written.insert(format!("u{i}_{j}"));
        }
    }
    // Quoted segments of `[[1,"name"],...]` are exactly the names
    // (ids render bare).
    let names: Vec<&str> = all.split('"').skip(1).step_by(2).collect();
    assert_eq!(
        names.len(),
        LOAD_CLIENTS * 16,
        "row count (lost rows?): {all:?}"
    );
    for n in names {
        assert!(written.contains(n), "unknown row {n:?}");
    }

    // Budget expiry stops the server; drain joins workers; the run
    // must exit cleanly with no leaked threads or connections.
    let (exit, errors) = server.join().expect("server thread");
    assert_eq!(exit, 0, "server exit {exit}");
    assert!(errors.is_empty(), "server errors: {errors:?}");
    assert_eq!(interp::live_connection_threads(), 0, "thread leak");
    assert_eq!(interp::live_db_connections(), 0, "db leak");
    std::fs::remove_file(&db_path).ok();
}

#[test]
fn load_crud_16x128_two_cycles() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Handlers sleep 5 ms so the mid-storm sample always overlaps.
    run_load_cycle(0);
    // Second consecutive cycle: identical success with zero deltas
    // is the no-growth leak pass (RSS trend by equivalent means —
    // no new toolchain; see module docs).
    run_load_cycle(1);
}

// ── Phase 6 status-aware handlers (deferred Phase5 §6 item) ────────────────
//
// A handler may return an `HttpResponse` struct value (status,
// reason, headers, body) for a status-aware response, or a plain
// `String` for 200-as-today. Unknown names, non-String/non-Response
// values, errors, and panics keep answering 500; unmatched stays
// 404; malformed stays 400. Every server-touching test below holds
// the SERIAL gate, uses small budgets, and drives fail-fast raw-TCP
// clients (5 s watchdog) — the same conventions as the Phase 5
// tests above.
//
// NOTE (scope): the status-aware CLIENT (`http_send_full_builtin`)
// and connection caps (`http_server_set_limits`) do NOT land here:
// a new builtin name lowers to `Call(FuncId::UNRESOLVED)` and traps
// with `FunctionNotFound` on the VM (`lowering.rs::lower_host_builtin`
// `_ => None` fallthrough), so an interp-only builtin would be a
// release-blocking run/run-vm divergence. Wiring them needs new
// `Instr` variants + lowering arms (lowering-track follow-up); the
// mirror rule says such behavior lands on both runtimes or not at
// all. Status visibility from the client side is still pinned below
// over raw TCP (status line + headers + body).

#[test]
fn status_struct_201_with_headers() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"POST\", \"/created\", \"created_handler\")\n            http_server_route(srv, \"GET\", \"/fallback\", \"fallback_handler\")",
            "fn created_handler(body: String) -> HttpResponse:\n    http_response_with_headers(201, [[\"X-Test\", \"yes\"]], \"made:{body}\")\n\nfn fallback_handler(body: String) -> HttpResponse:\n    HttpResponse {\n        status: 201,\n        reason: \"\",\n        headers: [],\n        body: \"fallback\"\n    }",
        ),
        "struct-return server",
    );
    let server = run_server(module);
    wait_healthy(port, "/missing", "Not Found");

    // Struct return: exact status, verbatim headers, body.
    let resp = raw_post(port, "/created", "abc").expect("post /created");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 201 Created"),
        "struct status: {resp:?}"
    );
    assert!(
        resp.contains("\r\nX-Test: yes\r\n"),
        "struct header: {resp:?}"
    );
    assert_eq!(response_body(&resp), "made:abc");

    // Empty reason falls back to the canonical phrase.
    let resp = raw_get(port, "/fallback");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 201 Created"),
        "fallback status: {resp:?}"
    );
    assert_eq!(response_body(&resp), "fallback");

    join_server(server, "struct return");
}

#[test]
fn string_return_still_200_with_content_type() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"GET\", \"/hi\", \"hi_handler\")",
            "fn hi_handler(body: String) -> String:\n    \"hello\"",
        ),
        "string-return server",
    );
    let server = run_server(module);
    wait_healthy(port, "/missing", "Not Found");

    // Regression pin: the M4 String shape is byte-stable
    // (200 + text/plain + body) now that structs exist alongside it.
    let resp = raw_get(port, "/hi");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 200 OK"),
        "string status: {resp:?}"
    );
    assert!(
        resp.contains("\r\nContent-Type: text/plain\r\n"),
        "string content-type: {resp:?}"
    );
    assert_eq!(response_body(&resp), "hello");

    join_server(server, "string return");
}

#[test]
fn shapes_404_500_unchanged() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"GET\", \"/err\", \"err_handler\")\n            http_server_route(srv, \"GET\", \"/ghost\", \"no_such_handler\")",
            "fn err_handler(body: String) -> Result<String, String>:\n    Err(\"boom\")",
        ),
        "shapes server",
    );
    let server = run_server(module);
    wait_healthy(port, "/missing", "Not Found");

    // Unmatched route: 404 with the exact legacy shape.
    let resp = raw_get(port, "/missing");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 404 Not Found"),
        "404 status: {resp:?}"
    );
    assert_eq!(response_body(&resp), "Not Found");

    // Handler failure value: 500, never 200.
    let resp = raw_get(port, "/err");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 500"),
        "err status: {resp:?}"
    );
    assert!(
        response_body(&resp).contains("Internal Server Error"),
        "err body: {resp:?}"
    );

    // Unknown handler name: 500 (the interpreter resolves it to
    // Unit, which is a non-String value — same 500 as ever).
    let resp = raw_get(port, "/ghost");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 500"),
        "unknown-handler status: {resp:?}"
    );

    join_server(server, "shapes");
}

#[test]
fn struct_bad_status_500() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_server_source(
            port,
            "            http_server_route(srv, \"GET\", \"/low\", \"low_handler\")\n            http_server_route(srv, \"GET\", \"/high\", \"high_handler\")",
            "fn low_handler(body: String) -> HttpResponse:\n    http_response(99, \"low\")\n\nfn high_handler(body: String) -> HttpResponse:\n    http_response(600, \"high\")",
        ),
        "bad-status server",
    );
    let server = run_server(module);
    wait_healthy(port, "/missing", "Not Found");

    // Out-of-range statuses never render: loud 500, server survives.
    for path in ["/low", "/high"] {
        let resp = raw_get(port, path);
        assert!(
            status_line(&resp).starts_with("HTTP/1.1 500"),
            "{path} status: {resp:?}"
        );
        assert!(
            response_body(&resp).contains("Internal Server Error"),
            "{path} body: {resp:?}"
        );
    }

    join_server(server, "bad status");
}

/// VM server source: serve loop directly in `main` (the VM has no
/// async runtime, so `task`/`await` trap — the CLI parity suite pins
/// that refusal). Stops via the `/stop` route, whose handler sweeps
/// the small server-handle space (`http_server_shutdown` on an
/// unknown handle is a silent no-op `Unit`, so the sweep is exact
/// under the SERIAL gate with no handle plumbing).
fn small_vm_server_source(port: u16, routes: &str, handlers: &str) -> String {
    let libs: Vec<String> = STDLIB_LIBS.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    let sweep: Vec<String> = (1..=64)
        .map(|h| format!("    http_server_shutdown({h})"))
        .collect();
    sources.push(format!(
        r#"{handlers}

fn stop_handler(body: String) -> String:
{sweep}
    "stopping"

fn main():
    match http_server_listen({port}):
        Ok(srv):
{routes}
            http_server_register_route(srv, "GET", "/stop", "stop_handler")
            http_server_serve_loop(srv)
        Err(e):
            "listen-failed"
"#,
        handlers = handlers,
        port = port,
        routes = routes,
        sweep = sweep.join("\n"),
    ));
    join(&sources)
}

fn run_vm_server(module: compiler::hir::Module) -> std::thread::JoinHandle<Result<String, String>> {
    std::thread::spawn(move || {
        let nir = compiler::nir::lowering::LoweringContext::new(module).lower_module();
        let mut vm = compiler::nir::vm::Vm::new(nir);
        vm.run().map(|v| v.to_string()).map_err(|e| e.to_string())
    })
}

#[test]
fn vm_status_struct_parity() {
    let _gate = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let port = free_port();
    let module = check(
        &small_vm_server_source(
            port,
            "            http_server_register_route(srv, \"GET\", \"/created\", \"created_handler\")\n            http_server_register_route(srv, \"GET\", \"/plain\", \"plain_handler\")\n            http_server_register_route(srv, \"GET\", \"/ghost\", \"no_such_handler\")",
            "fn created_handler(body: String) -> HttpResponse:\n    http_response_with_headers(201, [[\"X-VM\", \"1\"]], \"vm-made\")\n\nfn plain_handler(body: String) -> String:\n    \"plain\"",
        ),
        "vm struct-return server",
    );
    let server = run_vm_server(module);
    wait_healthy(port, "/missing", "Not Found");

    // Struct return through the VM serve loop: same bytes as interp.
    let resp = raw_get(port, "/created");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 201 Created"),
        "vm struct status: {resp:?}"
    );
    assert!(
        resp.contains("\r\nX-VM: 1\r\n"),
        "vm struct header: {resp:?}"
    );
    assert_eq!(response_body(&resp), "vm-made");

    // String return and unknown-handler 500 agree too.
    let resp = raw_get(port, "/plain");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 200 OK"),
        "vm string status: {resp:?}"
    );
    assert_eq!(response_body(&resp), "plain");
    let resp = raw_get(port, "/ghost");
    assert!(
        status_line(&resp).starts_with("HTTP/1.1 500"),
        "vm unknown-handler status: {resp:?}"
    );

    // Stop through the sweep route, then the drained VM run must
    // return Unit (its Display is `()`).
    let resp = raw_get(port, "/stop");
    assert_eq!(response_body(&resp), "stopping");
    let out = server.join().expect("vm server thread");
    assert_eq!(out, Ok("()".to_string()), "vm run result: {out:?}");
}
