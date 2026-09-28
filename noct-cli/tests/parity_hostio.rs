//! CLI-level host-IO parity: `noct run` (tree-walking interpreter) vs
//! `noct run-vm` (NIR bytecode VM) — Phase 5/M4 builtins.
//!
//! Every case runs the same well-typed `.nv` source through both CLIs
//! and asserts identical exit code and byte-identical stdout. Stderr is
//! asserted per-case with `contains` (never cross-backend equality):
//! runtime-failure prefixes differ by backend BY DESIGN (`error:
//! [E1002] panic: …` vs `VM error: …`), while the trap message core
//! (e.g. `non-negative`) is shared.
//!
//! Native (`noct build`) coverage lives in `native_build.rs`, which owns
//! the `BUILD_LOCK` serializing the shared cargo target dir — this file
//! deliberately never invokes `noct build` (a second build-calling test
//! binary would race on the same `noctivue_shim` output; see that file's
//! header). VM-only builtins (aggregate returns) fail `noct build`
//! loudly; those contract checks are `native_build.rs`'s
//! `native_build_rejects_vm_only_hostio_loudly`, not here.
//!
//! Hermeticity: each case gets a fresh temp dir (used as the child
//! CWD, so `./*.nv.env` auto-load and relative paths cannot leak
//! between cases), ephemeral ports for sockets, `:memory:` SQLite
//! databases, and pid+counter-unique env names and temp paths, so
//! cases are parallel-safe.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

/// Seconds before a hung program fails the test instead of hanging CI.
const CASE_TIMEOUT_SECS: u64 = 15;

fn run_cli_in(dir: &Path, args: &[&str]) -> Output {
    let child = Command::new(noct_bin())
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn noct binary");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(CASE_TIMEOUT_SECS)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => panic!("failed waiting on noct binary: {e}"),
        Err(_) => panic!(
            "TIMEOUT (>{CASE_TIMEOUT_SECS}s): `noct {}` hung",
            args.join(" ")
        ),
    }
}

fn scratch_dir(name: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("noctivue-parity-{}-{}-{id}", std::process::id(), name));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write_source(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(format!("{name}.nv"));
    let mut f = std::fs::File::create(&path).expect("create temp file");
    f.write_all(source.as_bytes()).expect("write temp file");
    path
}

/// Run `source` through `run` and `run-vm` (same file, sequentially),
/// assert identical exit code and stdout, and return both outputs for
/// per-case stderr assertions. Cleans up the scratch dir.
fn check_parity(name: &str, source: &str) -> (Output, Output) {
    let dir = scratch_dir(name);
    let path = write_source(&dir, name, source);
    let path_str = path.to_string_lossy().to_string();
    let run = run_cli_in(&dir, &["run", &path_str]);
    let run_vm = run_cli_in(&dir, &["run-vm", &path_str]);
    assert_eq!(
        run.status.code(),
        run_vm.status.code(),
        "[{name}] exit code differs: run={:?} run-vm={:?}\n  run stderr:\n{}\n  run-vm stderr:\n{}",
        run.status.code(),
        run_vm.status.code(),
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run_vm.stderr),
    );
    assert_eq!(
        run.stdout, run_vm.stdout,
        "[{name}] stdout differs:\n  run:\n{}\n  run-vm:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run_vm.stdout),
    );
    let _ = std::fs::remove_dir_all(&dir);
    (run, run_vm)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Allocate a loopback port by binding `:0`, then release it. The test
/// rebinds it immediately (standard TOCTOU, negligible window — the
/// same pattern as the root `http_test.rs`).
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    listener.local_addr().expect("local addr").port()
}

// ── (a) sleep ─────────────────────────────────────────────────────────

#[test]
fn parity_sleep_ok() {
    let (run, _) = check_parity(
        "sleep_ok",
        "fn main():\n    sleep_builtin(0)\n    sleep_builtin(5)\n    print(\"slept\")\n",
    );
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "slept");
}

#[test]
fn parity_sleep_negative_traps_identically() {
    let (run, run_vm) = check_parity("sleep_neg", "fn main():\n    sleep_builtin(0 - 1)\n    print(\"unreached\")\n");
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stdout.is_empty() && run_vm.stdout.is_empty());
    assert!(stderr(&run).contains("non-negative"), "run: {}", stderr(&run));
    assert!(stderr(&run_vm).contains("non-negative"), "run-vm: {}", stderr(&run_vm));
}

// ── (d) fs ────────────────────────────────────────────────────────────

#[test]
fn parity_fs_write_read_roundtrip() {
    let dir = scratch_dir("fs_rw");
    let file = dir.join("roundtrip.txt");
    let path_lit = file.to_string_lossy().replace('\\', "\\\\");
    let source = format!(
        "fn main():\n    match fs_write_text(\"{path_lit}\", \"hello-hostio\"):\n        Ok(_):\n            match fs_read_text(\"{path_lit}\"):\n                Ok(body): print(\"{{body}}\")\n                Err(e): print(\"read-err\")\n        Err(e): print(\"write-err\")\n"
    );
    let (run, _) = check_parity("fs_rw", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "hello-hostio");
    assert_eq!(std::fs::read_to_string(&file).expect("written file"), "hello-hostio");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_fs_read_missing_is_err() {
    let (run, _) = check_parity(
        "fs_missing",
        "fn main():\n    match fs_read_text(\"definitely-not-here-hostio\"):\n        Ok(_): print(\"unexpected-ok\")\n        Err(_): print(\"is-err\")\n",
    );
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "is-err");
}

#[test]
fn parity_fs_exists_both() {
    let dir = scratch_dir("fs_exists");
    let file = dir.join("present.txt");
    std::fs::write(&file, b"x").expect("seed file");
    let lit = file.to_string_lossy().replace('\\', "\\\\");
    let source = format!(
        "fn main():\n    let here = fs_exists(\"{lit}\")\n    let gone = fs_exists(\"{lit}-absent\")\n    print(\"{{here}}-{{gone}}\")\n"
    );
    let (run, _) = check_parity("fs_exists", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "true-false");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_fs_list_dir_names() {
    let dir = scratch_dir("fs_list_dir");
    std::fs::write(dir.join("zeta.txt"), b"z").expect("seed file");
    std::fs::write(dir.join("alpha.txt"), b"a").expect("seed file");
    std::fs::create_dir_all(dir.join("beta-dir")).expect("seed directory");
    let lit = dir.to_string_lossy().replace('\\', "\\\\");
    // The VM rejects the nested helper-call pattern for this aggregate
    // shape (`malformed CFG: phi ... has no incoming entry`), so parity is
    // intentionally limited to the shared count/error contract. Field
    // extraction is pinned interpreter-side in `interp_fs_list_dir_*` and
    // VM-side for the instruction shape in `vm_hostio_tests`.
    let source = format!(
        "fn main():\n    match fs_list_dir_builtin(\"{lit}\"):\n        Ok(entries):\n            if entries.length != 3:\n                print(\"wrong-count\")\n            else:\n                print(\"directory-ok\")\n        Err(_):\n            print(\"listing-failed\")\n"
    );
    let (run, _) = check_parity("fs_list_dir", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "directory-ok");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_fs_modified_millis() {
    let dir = scratch_dir("fs_mtime");
    let file = dir.join("mtime.txt");
    std::fs::write(&file, b"x").expect("seed file");
    let lit = file.to_string_lossy().replace('\\', "\\\\");
    let source = format!(
        "fn main():\n    match fs_modified_millis_builtin(\"{lit}\"):\n        Ok(ms): print(\"ok\")\n        Err(e): print(\"err\")\n    match fs_modified_millis_builtin(\"{lit}-absent\"):\n        Ok(_): print(\"unexpected-ok\")\n        Err(_): print(\"is-err\")\n"
    );
    let (run, _) = check_parity("fs_mtime", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "okis-err");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_io_writes() {
    let (run, _) = check_parity(
        "io_writes",
        "fn main():\n    io_write(\"ab\")\n    io_writeln(\"cd\")\n    io_writeln(\"ef\")\n",
    );
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "abcd\nef\n");
}

// ── (d) env ───────────────────────────────────────────────────────────

#[test]
fn parity_env_set_get_roundtrip() {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let var = format!("NOCTIVUE_PARITY_{}_{}", std::process::id(), id);
    let source = format!(
        "fn main():\n    env_set_builtin(\"{var}\", \"v42\")\n    match env_get_builtin(\"{var}\"):\n        Some(v): print(\"{{v}}\")\n        None: print(\"missing\")\n    match env_get_builtin(\"{var}_absent\"):\n        Some(_): print(\"unexpected\")\n        None: print(\"is-none\")\n"
    );
    let (run, _) = check_parity("env_roundtrip", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "v42is-none");
}

#[test]
fn parity_config_get() {
    let dir = scratch_dir("config");
    let file = dir.join("app.cfg");
    std::fs::write(&file, "# comment\n\nhost = example.com\nport=8080\n").expect("seed config");
    let lit = file.to_string_lossy().replace('\\', "\\\\");
    let source = format!(
        "fn main():\n    match config_get_builtin(\"{lit}\", \"port\"):\n        Ok(v): print(\"{{v}}\")\n        Err(e): print(\"err\")\n    match config_get_builtin(\"{lit}\", \"nope\"):\n        Ok(_): print(\"unexpected-ok\")\n        Err(e): print(\"{{e}}\")\n    match config_get_builtin(\"{lit}-absent\", \"k\"):\n        Ok(_): print(\"unexpected-ok\")\n        Err(_): print(\"is-err\")\n"
    );
    let (run, _) = check_parity("config", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "8080configuration key `nope` not foundis-err");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_dotenv_load() {
    let dir = scratch_dir("dotenv");
    let file = dir.join("case.nv.env");
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let var_a = format!("NOCTIVUE_PARITY_{id}_A");
    let var_b = format!("NOCTIVUE_PARITY_{id}_B");
    // B is preset in NEITHER backend (children start clean): assert the
    // count (2 fresh sets; the malformed line warns and skips) and both
    // values through `env_get`.
    std::fs::write(&file, format!("# c\n\n{var_a} = hello\n{var_b}=world\nmalformed line\n"))
        .expect("seed dotenv");
    let lit = file.to_string_lossy().replace('\\', "\\\\");
    let source = format!(
        "fn main():\n    match dotenv_load_builtin(\"{lit}\"):\n        Ok(n): print(\"{{n}}-\")\n        Err(e): print(\"err\")\n    match env_get_builtin(\"{var_a}\"):\n        Some(v): print(\"{{v}}-\")\n        None: print(\"missing-\")\n    match env_get_builtin(\"{var_b}\"):\n        Some(v): print(\"{{v}}\")\n        None: print(\"missing\")\n"
    );
    let (run, _) = check_parity("dotenv", &source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "2-hello-world");
    let (run2, _) = check_parity(
        "dotenv_missing",
        "fn main():\n    match dotenv_load_builtin(\"definitely-not-here-hostio.env\"):\n        Ok(_): print(\"unexpected-ok\")\n        Err(_): print(\"is-err\")\n",
    );
    assert_eq!(run2.status.code(), Some(0));
    assert_eq!(stdout(&run2), "is-err");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── (d) log ───────────────────────────────────────────────────────────

#[test]
fn parity_log_emit() {
    let (run, run_vm) = check_parity(
        "log_emit",
        "fn main():\n    log_emit_builtin(\"info\", \"hello-log\")\n    log_emit_builtin(\"warn\", \"careful\")\n    print(\"logged\")\n",
    );
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "logged");
    for (label, out) in [("run", &run), ("run-vm", &run_vm)] {
        let err = stderr(out);
        assert!(err.contains("[INFO] hello-log"), "[{label}] {err}");
        assert!(err.contains("[WARN] careful"), "[{label}] {err}");
    }
}

// ── (c) db ────────────────────────────────────────────────────────────

#[test]
fn parity_db_full_crud() {
    let source = "fn main():\n    match db_open_builtin(\":memory:\"):\n        Ok(id):\n            match db_exec_builtin(id, \"CREATE TABLE t (name TEXT, age INT)\", \"[]\"):\n                Ok(_): print(\"created-\")\n                Err(e): print(\"create-err-\")\n            match db_exec_builtin(id, \"INSERT INTO t VALUES (?, ?)\", \"[\\\"Alice\\\", 30]\"):\n                Ok(n): print(\"{n}-\")\n                Err(e): print(\"insert-err-\")\n            match db_query_builtin(id, \"SELECT name, age FROM t WHERE age > ?\", \"[18]\"):\n                Ok(rows):\n                    for r in rows:\n                        print(\"{r}-\")\n                Err(e): print(\"query-err-\")\n            match db_close_builtin(id):\n                Ok(_): print(\"closed\")\n                Err(e): print(\"close-err\")\n        Err(e): print(\"open-err\")\n";
    let (run, _) = check_parity("db_crud", source);
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "created-1-[\"Alice\", 30]-closed");
}

#[test]
fn parity_db_errors() {
    // Every misuse shape answers `Err` (exit 0, tag printed) — never a trap.
    let source = "fn main():\n    match db_open_builtin(\":memory:\"):\n        Ok(id):\n            match db_exec_builtin(id, \"NOT SQL\", \"[]\"):\n                Ok(_): print(\"A-unexpected-\")\n                Err(_): print(\"A-err-\")\n            match db_exec_builtin(999, \"SELECT 1\", \"[]\"):\n                Ok(_): print(\"B-unexpected-\")\n                Err(e): print(\"{e}-\")\n            match db_query_builtin(999, \"SELECT 1\", \"[]\"):\n                Ok(_): print(\"C-unexpected-\")\n                Err(e): print(\"{e}-\")\n            match db_exec_builtin(id, \"SELECT 1\", \"not-json\"):\n                Ok(_): print(\"D-unexpected-\")\n                Err(e): print(\"{e}-\")\n            match db_close_builtin(id):\n                Ok(_):\n                    match db_close_builtin(id):\n                        Ok(_): print(\"E-unexpected\")\n                        Err(e): print(\"{e}\")\n                Err(e): print(\"first-close-failed\")\n        Err(e): print(\"open-err\")\n";
    let (run, _) = check_parity("db_errors", source);
    assert_eq!(run.status.code(), Some(0));
    // Handles start at 1 in a fresh process, so the double-closed id
    // is exactly 1 — the full stdout is pinned byte-for-byte.
    let expected = "A-err-\
        unknown database handle `999` (was it closed?)-\
        unknown database handle `999` (was it closed?)-\
        params must be a JSON array like `[1, \"x\"]`-\
        unknown database handle `1` (was it closed?)";
    assert_eq!(stdout(&run), expected);
}

// ── (b) http client ───────────────────────────────────────────────────

/// Harness-side HTTP server: accepts exactly `n` connections,
/// routes GET /hi → `hello`, POST /echo → request body, anything else
/// → 404 `Not Found`. Returns the port and the join handle.
fn serve_harness(n: usize) -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(false).ok();
        for _ in 0..n {
            let Ok((mut conn, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let Ok(count) = conn.read(&mut buf) else {
                continue;
            };
            let text = String::from_utf8_lossy(&buf[..count]).to_string();
            let (method, path) = text
                .lines()
                .next()
                .and_then(|l| {
                    let mut parts = l.split_whitespace();
                    Some((parts.next()?.to_string(), parts.next()?.to_string()))
                })
                .unwrap_or_default();
            let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            let (status, payload) = match (method.as_str(), path.as_str()) {
                ("GET", "/hi") => ("HTTP/1.1 200 OK", "hello".to_string()),
                ("POST", "/echo") => ("HTTP/1.1 200 OK", body),
                _ => ("HTTP/1.1 404 Not Found", "Not Found".to_string()),
            };
            let resp = format!(
                "{status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            conn.write_all(resp.as_bytes()).ok();
        }
    });
    // Give the listener a moment to start accepting.
    std::thread::sleep(std::time::Duration::from_millis(20));
    (port, handle)
}

#[test]
fn parity_http_client_against_harness() {
    // Six accepts: three requests per backend run, two runs (run,
    // then run-vm — sequential, so ordering is deterministic).
    let (port, served) = serve_harness(6);
    let source = format!(
        "fn main():\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:{port}/hi\", [], \"\"):\n        Ok(b): print(\"{{b}}-\")\n        Err(e): print(\"get-err-\")\n    match http_send_builtin(\"POST\", \"http://127.0.0.1:{port}/echo\", [], \"ping\"):\n        Ok(b): print(\"{{b}}-\")\n        Err(e): print(\"post-err-\")\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:{port}/missing\", [], \"\"):\n        Ok(b): print(\"{{b}}\")\n        Err(e): print(\"404-err\")\n"
    );
    let (run, _) = check_parity("http_client", &source);
    served.join().ok();
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "hello-ping-Not Found");
}

#[test]
fn parity_http_refusals() {
    let port = free_port();
    let source = format!(
        "fn main():\n    match http_send_builtin(\"GET\", \"https://127.0.0.1:{port}/\", [], \"\"):\n        Ok(_): print(\"A-unexpected-\")\n        Err(e): print(\"{{e}}-\")\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:{port}/\", [], \"\"):\n        Ok(_): print(\"B-unexpected-\")\n        Err(e): print(\"{{e}}\")\n"
    );
    let (run, _) = check_parity("http_refusals", &source);
    assert_eq!(run.status.code(), Some(0));
    assert!(
        stdout(&run).starts_with("unsupported URL scheme (expected http://): https://"),
        "tls refusal names http: {}",
        stdout(&run)
    );
    assert!(stdout(&run).contains("connection failed"), "refused port: {}", stdout(&run));
}

// ── (b) http server lifecycle + cross-backend round trips ─────────────

#[test]
fn parity_http_server_lifecycle() {
    // Ephemeral port, register, shutdown, serve-after-shutdown (unknown
    // handle → immediate Unit): no tasks, no blocking, both paths.
    let (run, _) = check_parity(
        "http_lifecycle",
        "fn main():\n    match http_server_listen(0):\n        Ok(srv):\n            http_server_register_route(srv, \"GET\", \"/hi\", \"h\")\n            http_server_shutdown(srv)\n            http_server_serve_loop(srv)\n            print(\"ok\")\n        Err(e): print(\"listen-err\")\n",
    );
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(stdout(&run), "ok");
}

fn server_source(port: u16) -> String {
    format!(
        "fn hello_handler(body: String) -> String:\n    \"hello\"\n\nfn echo_handler(body: String) -> String:\n    body\n\nfn main():\n    match http_server_listen({port}):\n        Ok(srv):\n            http_server_register_route(srv, \"GET\", \"/hi\", \"hello_handler\")\n            http_server_register_route(srv, \"POST\", \"/echo\", \"echo_handler\")\n            http_server_serve_loop(srv)\n        Err(e):\n            panic(\"listen failed\")\n"
    )
}

fn client_source(port: u16) -> String {
    // NOTE: no `assert(...)` here — `assert` is a core builtin that the
    // VM/native backends do not lower yet (out of scope for the host-IO
    // families), so a VM client would die with `FunctionNotFound`.
    // Bodies print raw and the harness pins the exact stdout instead —
    // byte-identical rigor without the unported builtin.
    format!(
        "fn main():\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:{port}/hi\", [], \"\"):\n        Ok(b): print(\"{{b}}-\")\n        Err(e): print(\"get-err-\")\n    match http_send_builtin(\"POST\", \"http://127.0.0.1:{port}/echo\", [], \"ping\"):\n        Ok(b): print(\"{{b}}-\")\n        Err(e): print(\"post-err-\")\n    match http_send_builtin(\"GET\", \"http://127.0.0.1:{port}/missing\", [], \"\"):\n        Ok(b): print(\"{{b}}\")\n        Err(e): print(\"404-err\")\n"
    )
}

fn spawn_server(dir: &Path, backend: &str, port: u16) -> Child {
    let path = write_source(dir, &format!("server_{backend}"), &server_source(port));
    Command::new(noct_bin())
        .arg(backend)
        .arg(&path)
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn server")
}

fn wait_ready(port: u16) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            // The probe occupies one accept; the serve loop handles it
            // (400 Bad Request on the empty read) and continues.
            std::thread::sleep(std::time::Duration::from_millis(50));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("server on port {port} never became ready");
}

/// One server backend × one client backend round trip.
fn check_server_client(server_backend: &str, client_backend: &str) {
    let name = format!("http_{server_backend}_{client_backend}");
    let dir = scratch_dir(&name.replace('-', "_"));
    let port = free_port();
    let mut server = spawn_server(&dir, server_backend, port);
    wait_ready(port);
    let client = write_source(&dir, "client", &client_source(port));
    let out = run_cli_in(&dir, &[client_backend, &client.to_string_lossy()]);
    let _ = server.kill();
    let _ = server.wait();
    assert_eq!(
        out.status.code(),
        Some(0),
        "[{name}] client exit {:?}:\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        stdout(&out),
        stderr(&out),
    );
    assert_eq!(stdout(&out), "hello-ping-Not Found", "[{name}] stdout");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parity_http_vm_server_serves_both_clients() {
    // The VM dispatches registered Noctivue handlers (GET + POST echo
    // + 404) exactly like the interpreter — the core serve_loop proof.
    check_server_client("run-vm", "run");
    check_server_client("run-vm", "run-vm");
}

#[test]
fn parity_http_interp_server_serves_vm_client() {
    // Mirror direction: the interpreter's server answers the VM's
    // client (proves the VM's http_send against a real server).
    check_server_client("run", "run-vm");
}

// ── (a) tasks stay loud on run-vm ─────────────────────────────────────
// (`noct build`'s twin refusal is pinned in native_build.rs alongside
// the other build contract checks; `noct run` is task-capable.)

#[test]
fn parity_task_refused_on_vm() {
    // Deliberately NOT `check_parity` (exits differ BY DESIGN here).
    let dir = scratch_dir("task_vm_refusal");
    let path = write_source(&dir, "task_vm_refusal", "task w():\n    1\nfn main():\n    await w()\n");
    let path_str = path.to_string_lossy().to_string();
    let run = run_cli_in(&dir, &["run", &path_str]);
    let run_vm = run_cli_in(&dir, &["run-vm", &path_str]);
    // The interpreter runs tasks fine…
    assert_eq!(run.status.code(), Some(0), "run: {}", stderr(&run));
    // …while the VM refuses loudly (synchronous execution would run
    // the task inline and hand back the raw handle).
    assert_eq!(run_vm.status.code(), Some(1));
    assert!(
        stderr(&run_vm).contains("requires the async runtime"),
        "run-vm: {}",
        stderr(&run_vm)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
