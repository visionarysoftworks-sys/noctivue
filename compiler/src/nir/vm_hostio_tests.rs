//! In-process unit tests for the VM's Phase 5/M4 host-IO builtins
//! (`Sleep`, `Fs*`, `Io*`, `Env*`, `ConfigGet`, `DotenvLoad`,
//! `LogEmit`, `Http*`, `Db*`).
//!
//! Each case runs the same source through lowering + `Vm` and asserts
//! the RETURNED value (or the trap text) — stdout/stderr parity across
//! backends is covered at the CLI level (`noct-cli/tests/parity_hostio.rs`
//! and `native_build.rs`), where pipes can be captured. Everything here
//! is hermetic and parallel-safe: fresh `Vm` per case (DB registries are
//! per-instance, server handles come from a global atomic), ephemeral
//! ports, unique temp files, and unique process-env names.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::diagnostics::DiagnosticSink;
use crate::hir::Module as HirModule;
use crate::lexer::lex;
use crate::nir::lowering::LoweringContext;
use crate::nir::vm::Vm;
use crate::parser::parse;
use crate::resolver::resolve;
use crate::typeck::typecheck;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tag() -> String {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}_{}", std::process::id(), id)
}

/// Lower `source` and run it on a fresh VM. Returns the `main` return
/// value rendered via `Display`, or the `VmError` text on trap.
/// (Panics on frontend errors — every fixture here is valid Noctivue.)
fn run_vm_value(source: &str) -> Result<String, String> {
    let mut sink = DiagnosticSink::new();
    let tokens = lex(source, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    let module: HirModule = typecheck(program, &mut sink);
    let errors: Vec<String> = sink
        .diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect();
    assert!(errors.is_empty(), "fixture has frontend errors: {errors:?}");
    run_vm_module(module)
}

/// Lower + run without asserting frontend cleanliness (mirrors what
/// `interp`'s own unit tests do for shape-error cases: typeck emits
/// E0200 but HIR recovery still runs, and the builtin answers `Err`
/// at runtime instead of trapping).
fn run_vm_value_lenient(source: &str) -> Result<String, String> {
    let mut sink = DiagnosticSink::new();
    let tokens = lex(source, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    let module: HirModule = typecheck(program, &mut sink);
    run_vm_module(module)
}

fn run_vm_module(module: HirModule) -> Result<String, String> {
    let nir_module = LoweringContext::new(module).lower_module();
    let mut vm = Vm::new(nir_module);
    vm.run().map(|v| v.to_string()).map_err(|e| e.to_string())
}

/// A RAW temp path for the test only (parent dir created). Pass
/// through [`nv_string_literal`] for `.nv` sources; use directly for
/// Rust-side `std::fs` ops.
fn temp_path(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("case.txt").to_string_lossy().into_owned()
}

fn nv_string_literal(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn sleep_zero_returns_unit_value() {
    let out = run_vm_value("fn main():\n    sleep_builtin(0)\n    \"slept\"\n");
    assert_eq!(out, Ok("slept".to_string()));
}

#[test]
fn sleep_negative_traps_loudly() {
    let out = run_vm_value("fn main():\n    sleep_builtin(0 - 1)\n");
    let err = out.expect_err("negative sleep must trap");
    assert!(err.contains("non-negative"), "trap must name the contract: {err}");
}

#[test]
fn fs_write_then_read_round_trip() {
    let tag = unique_tag();
    let path = temp_path(&tag);
    let lit = nv_string_literal(&path);
    let source = format!(
        "fn main():\n    match fs_write_text({lit}, \"hello-vm\"):\n        Ok(_):\n            match fs_read_text({lit}):\n                Ok(body): body\n                Err(e): \"read:{{e}}\"\n        Err(e): \"write:{{e}}\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("hello-vm".to_string()));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}")));
}

#[test]
fn fs_read_missing_is_err_not_trap() {
    let tag = unique_tag();
    let path = temp_path(&tag);
    let lit = nv_string_literal(&format!("{path}-absent"));
    let source = format!(
        "fn main():\n    match fs_read_text({lit}):\n        Ok(_): \"unexpected-ok\"\n        Err(_): \"is-err\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("is-err".to_string()));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}")));
}

#[test]
fn fs_exists_true_and_false() {
    let tag = unique_tag();
    let path = temp_path(&tag);
    std::fs::write(&path, b"x").expect("seed temp file");
    let lit = nv_string_literal(&path);
    let source = format!(
        "fn main():\n    let here = fs_exists({lit})\n    let gone = fs_exists({lit} + \"-absent\")\n    \"{{here}}-{{gone}}\"\n"
    );
    // Interpolation renders bools as `true`/`false`.
    assert_eq!(run_vm_value(&source), Ok("true-false".to_string()));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}")));
}

#[test]
fn fs_modified_millis_ok_and_missing() {
    let tag = unique_tag();
    let path = temp_path(&tag);
    std::fs::write(&path, b"x").expect("seed temp file");
    let lit = nv_string_literal(&path);
    let source = format!(
        "fn main():\n    match fs_modified_millis_builtin({lit}):\n        Ok(ms): ms > 0\n        Err(_): false\n"
    );
    assert_eq!(run_vm_value(&source), Ok("true".to_string()));
    let lit_missing = nv_string_literal(&format!("{path}-absent"));
    let source = format!(
        "fn main():\n    match fs_modified_millis_builtin({lit_missing}):\n        Ok(_): \"unexpected-ok\"\n        Err(_): \"is-err\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("is-err".to_string()));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}")));
}

#[test]
fn env_set_get_round_trip_and_missing() {
    let tag = unique_tag();
    let var = format!("NOCTIVUE_VM_HOSTIO_{tag}");
    let source = format!(
        "fn main():\n    env_set_builtin(\"{var}\", \"v42\")\n    match env_get_builtin(\"{var}\"):\n        Some(v): v\n        None: \"missing\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("v42".to_string()));
    let source = format!(
        "fn main():\n    match env_get_builtin(\"{var}_absent\"):\n        Some(_): \"unexpected\"\n        None: \"is-none\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("is-none".to_string()));
    unsafe { std::env::remove_var(&var) };
}

#[test]
fn config_get_ok_missing_key_missing_file() {
    let tag = unique_tag();
    let path = temp_path(&tag);
    std::fs::write(&path, "# comment\n\nhost = example.com\nport=8080\n").expect("seed config");
    let lit = nv_string_literal(&path);
    let source = format!(
        "fn main():\n    match config_get_builtin({lit}, \"port\"):\n        Ok(v): v\n        Err(e): \"err:{{e}}\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("8080".to_string()));
    let source = format!(
        "fn main():\n    match config_get_builtin({lit}, \"nope\"):\n        Ok(_): \"unexpected-ok\"\n        Err(e): e\n"
    );
    assert_eq!(
        run_vm_value(&source),
        Ok("configuration key `nope` not found".to_string())
    );
    let lit_missing = nv_string_literal(&format!("{path}-absent"));
    let source = format!(
        "fn main():\n    match config_get_builtin({lit_missing}, \"k\"):\n        Ok(_): \"unexpected-ok\"\n        Err(_): \"is-err\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("is-err".to_string()));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}")));
}

#[test]
fn dotenv_load_counts_and_process_wins() {
    let tag = unique_tag();
    let dir = std::env::temp_dir().join(format!("noctivue-vm-hostio-{tag}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let env_path = dir.join("case.nv.env");
    let var_a = format!("NOCTIVUE_VM_HOSTIO_{tag}_A");
    let var_b = format!("NOCTIVUE_VM_HOSTIO_{tag}_B");
    unsafe { std::env::set_var(&var_b, "preset") };
    std::fs::write(
        &env_path,
        format!("# comment\n\n{var_a} = hello\n{var_b}=override\nmalformed line\n"),
    )
    .expect("seed dotenv");
    let lit = nv_string_literal(&env_path.to_string_lossy().replace('\\', "\\\\"));
    // Only A is newly set (B is preset so the process wins; the
    // malformed line warns and skips) — count is 1 either way.
    let source = format!(
        "fn main():\n    match dotenv_load_builtin({lit}):\n        Ok(n): n\n        Err(e): 0 - 99\n"
    );
    assert_eq!(run_vm_value(&source), Ok("1".to_string()));
    assert_eq!(std::env::var(&var_a).as_deref(), Ok("hello"));
    assert_eq!(std::env::var(&var_b).as_deref(), Ok("preset"));
    unsafe { std::env::remove_var(&var_a) };
    unsafe { std::env::remove_var(&var_b) };
    let lit_missing = nv_string_literal(&format!("{}-absent", env_path.to_string_lossy().replace('\\', "\\\\")));
    let source = format!(
        "fn main():\n    match dotenv_load_builtin({lit_missing}):\n        Ok(_): \"unexpected-ok\"\n        Err(_): \"is-err\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("is-err".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn log_emit_and_io_writes_return_unit() {
    // Observable in full at the CLI level (stderr/stdout pipes); here
    // the contract is "no trap, Unit value threads through".
    let out = run_vm_value("fn main():\n    log_emit_builtin(\"info\", \"unit-ok\")\n    \"ok\"\n");
    assert_eq!(out, Ok("ok".to_string()));
    let out = run_vm_value("fn main():\n    io_write(\"w\")\n    io_writeln(\"ln\")\n    \"ok\"\n");
    assert_eq!(out, Ok("ok".to_string()));
}

#[test]
fn db_memory_round_trip() {
    // One row comes back as one JSON-array string; every level
    // returns a String either way so arm types agree.
    let source = "fn main():\n    match db_open_builtin(\":memory:\"):\n        Ok(id):\n            match db_exec_builtin(id, \"CREATE TABLE t (name TEXT, age INT)\", \"[]\"):\n                Ok(_):\n                    match db_exec_builtin(id, \"INSERT INTO t VALUES (?, ?)\", \"[\\\"Alice\\\", 30]\"):\n                        Ok(n):\n                            match db_query_builtin(id, \"SELECT name, age FROM t\", \"[]\"):\n                                Ok(rows):\n                                    match db_close_builtin(id):\n                                        Ok(_): rows[0]\n                                        Err(e): \"close-failed\"\n                                Err(e): \"query-failed\"\n                        Err(e): \"insert-failed\"\n                Err(e): \"create-failed\"\n        Err(e): \"open-failed\"\n";
    assert_eq!(run_vm_value(source), Ok("[\"Alice\", 30]".to_string()));
}

#[test]
fn db_error_shapes_are_results() {
    // Bad SQL.
    let source = "fn main():\n    match db_open_builtin(\":memory:\"):\n        Ok(id):\n            match db_exec_builtin(id, \"NOT SQL AT ALL\", \"[]\"):\n                Ok(_): \"unexpected-ok\"\n                Err(e): \"is-err\"\n        Err(e): \"open-failed\"\n";
    assert_eq!(run_vm_value(source), Ok("is-err".to_string()));
    // Unknown handle on every op (999 is never issued: handles start at 1).
    for (op, tail) in [
        ("db_exec_builtin(999, \"SELECT 1\", \"[]\")", "exec"),
        ("db_query_builtin(999, \"SELECT 1\", \"[]\")", "query"),
        ("db_close_builtin(999)", "close"),
    ] {
        let source = format!(
            "fn main():\n    match {op}:\n        Ok(_): \"unexpected-ok-{tail}\"\n        Err(e): e\n"
        );
        let out = run_vm_value(&source).expect("must be a value, not a trap");
        assert!(out.contains("unknown database handle"), "{tail}: {out}");
    }
    // Malformed params JSON.
    let source = "fn main():\n    match db_open_builtin(\":memory:\"):\n        Ok(id):\n            match db_exec_builtin(id, \"SELECT 1\", \"not-json\"):\n                Ok(_): \"unexpected-ok\"\n                Err(e): e\n        Err(e): \"open-failed\"\n";
    let out = run_vm_value(source).expect("must be a value, not a trap");
    assert!(out.contains("JSON array"), "params shape: {out}");
    // Double close: the second close names the handle.
    let source = "fn main():\n    match db_open_builtin(\":memory:\"):\n        Ok(id):\n            match db_close_builtin(id):\n                Ok(_):\n                    match db_close_builtin(id):\n                        Ok(_): \"unexpected-ok\"\n                        Err(e): e\n                Err(e): \"first-close-failed\"\n        Err(e): \"open-failed\"\n";
    let out = run_vm_value(source).expect("must be a value, not a trap");
    assert!(out.contains("unknown database handle"), "double close: {out}");
}

#[test]
fn http_send_against_local_server() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let port = listener.local_addr().expect("local addr").port();
    let served = std::thread::spawn(move || {
        listener.set_nonblocking(false).ok();
        if let Ok((mut conn, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            if let Ok(n) = conn.read(&mut buf) {
                let _ = String::from_utf8_lossy(&buf[..n]).to_string();
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello";
                conn.write_all(resp.as_bytes()).ok();
            }
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(20));
    let source = format!(
        "fn main():\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:{port}/x\", [], \"\"):\n        Ok(body): body\n        Err(e): \"err:{{e}}\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("hello".to_string()));
    served.join().ok();
}

#[test]
fn http_send_refusals_are_results() {
    // https:// has no TLS stack — loud Err naming http://.
    let source = "fn main():\n    match http_send_builtin(\"GET\", \"https://127.0.0.1:9/\", [], \"\"):\n        Ok(_): \"unexpected-ok\"\n        Err(e): e\n";
    let out = run_vm_value(source).expect("must be a value, not a trap");
    assert!(out.contains("http://"), "tls refusal: {out}");
    // Nothing listens on port 9 (discard) — connection failure Err.
    let source = "fn main():\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:9/\", [], \"\"):\n        Ok(_): \"unexpected-ok\"\n        Err(e): e\n";
    let out = run_vm_value(source).expect("must be a value, not a trap");
    assert!(out.contains("connection failed"), "refused port: {out}");
    // Wrong shapes — the builtin's own Err, never a trap (typeck
    // rejects this statically, so it runs through HIR recovery like
    // the interpreter's own shape-error unit test does).
    let source = "fn main():\n    match http_send_builtin(42, \"http://x\", [], \"\"):\n        Ok(_): \"unexpected-ok\"\n        Err(_): \"is-err\"\n";
    assert_eq!(run_vm_value_lenient(source), Ok("is-err".to_string()));
}

#[test]
fn http_server_listen_register_shutdown_cycle() {
    // Ephemeral port (0): listen → register → shutdown → Unit threads.
    // Serve-after-shutdown hits an unknown handle and returns at once.
    let source = "fn main():\n    match http_server_listen(0):\n        Ok(srv):\n            http_server_register_route(srv, \"GET\", \"/hi\", \"h\")\n            http_server_shutdown(srv)\n            http_server_serve_loop(srv)\n            \"served\"\n        Err(e): \"listen-failed\"\n";
    assert_eq!(run_vm_value(source), Ok("served".to_string()));
    // Unknown handles are silent Units everywhere (never errors).
    let source = "fn main():\n    http_server_register_route(999, \"GET\", \"/\", \"h\")\n    http_server_shutdown(999)\n    http_server_serve_loop(999)\n    \"ok\"\n";
    assert_eq!(run_vm_value(source), Ok("ok".to_string()));
    // Double listen on one fixed port: the second bind fails as Err
    // (same `0.0.0.0:port` twice — the OS rejects the duplicate; a
    // Rust-side holder on `127.0.0.1` would NOT conflict on Windows,
    // so the conflict must be program-internal to be portable).
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let port = probe.local_addr().expect("local addr").port();
    drop(probe);
    let source = format!(
        "fn main():\n    match http_server_listen({port}):\n        Ok(srv1):\n            match http_server_listen({port}):\n                Ok(srv2):\n                    http_server_shutdown(srv1)\n                    http_server_shutdown(srv2)\n                    \"unexpected-ok\"\n                Err(_):\n                    http_server_shutdown(srv1)\n                    \"is-err\"\n        Err(_): \"first-failed\"\n"
    );
    assert_eq!(run_vm_value(&source), Ok("is-err".to_string()));
}
