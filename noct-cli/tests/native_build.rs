//! `noct build` end-to-end tests: compile straight-line fixtures to native
//! binaries, run them, and assert observable behavior.
//!
//! This is the Phase 3 Step-1 exit proof at the CLI level (codegen
//! acceptance without linking is covered cheaper in
//! `compiler/tests/native_smoke.rs`). Each case shells out to `cargo`
//! through `noct build`, so these are the slowest tests in the workspace
//! by design: the first case warms the shared target cache
//! (`<temp>/noctivue-target-cache`, see `cmd_build.rs`), later cases reuse
//! it. Generous timeout accordingly — a cold cache compiles
//! `runtime-native` from scratch.
//!
//! Parity rule asserted here (driver entry convention): success always
//! exits 0 with the program's stdout (a source-level `main() -> Int`
//! return value is DROPPED, matching the interpreter — see
//! `compiler/src/backends/cranelift/driver.rs`); traps exit 1 with a
//! backend-identifying stderr prefix.
//!
//! ## Three-way differential cases (IMPLEMENTATION_PLAN.md Phase 3, item 1)
//!
//! `differential_three_way_*` below extend the above into a real
//! interp/VM/native differential. They live in THIS file, sharing its
//! `BUILD_LOCK`, rather than a separate test binary: `noct build`'s
//! shared cargo target dir is a hardcoded path, not configurable per
//! test binary, so a second file calling `noct build` concurrently with
//! this one would race on the same `noctivue_shim{EXE_SUFFIX}` output —
//! the exact bug this file's own `BUILD_LOCK` comment already warns
//! about, just cross-process instead of cross-case. Scope is
//! deliberately the same straight-line category as the rest of this
//! file: a source needing control flow/aggregates would fail `noct
//! build` for the CORRECT reason (Step 2/3 isn't built yet), which
//! would make it a false-negative differential failure, not a real bug
//! — `native_build_rejects_control_flow_loudly_not_silently` below
//! tests that gap on purpose, as a contract check, not a differential
//! case.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Serializes cases end-to-end (build AND copy AND run live under this
/// lock). `noct build` funnels every case through ONE shared cargo target
/// dir whose link output is always `noctivue_shim.exe`: two cases building
/// concurrently would each copy the same filename while the other
/// overwrites it, so case A can end up running case B's binary (observed:
/// div0 exiting 0 with another case's stdout). Cargo's own target-dir lock
/// serializes the rustc invocations but NOT our copy of the output, so
/// the harness must hold the wider lock itself. Cost: cases run
/// sequentially (~seconds each on a warm cache) — acceptable for three.
/// Poisoning is tolerated (`into_inner`): one failing case must not
/// cascade into false failures in the rest of the suite.
static BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

/// Seconds before a case fails instead of hanging CI. Cold-cache first
/// builds compile an entire Rust crate; 300s is generous but finite.
const CASE_TIMEOUT_SECS: u64 = 300;

fn run_with_timeout(mut cmd: Command) -> Output {
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn child process");
    // Take stdin so nothing blocks on it; drop immediately.
    drop(child.stdin.take());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(CASE_TIMEOUT_SECS)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => panic!("failed waiting on child: {e}"),
        Err(_) => panic!("TIMEOUT (>{CASE_TIMEOUT_SECS}s): child hung"),
    }
}

fn write_case(name: &str, source: &str) -> (PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "noctivue-native-build-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let src = dir.join(format!("{name}.nv"));
    let mut f = std::fs::File::create(&src).expect("create temp file");
    f.write_all(source.as_bytes()).expect("write temp file");
    let exe = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    (src, exe)
}

/// `noct build` the source, run the produced binary, return its output.
/// Cleans up the temp dir (source + binary); the shared cargo target
/// cache is intentionally kept for the next case.
fn build_and_run(name: &str, source: &str) -> Output {
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (src, exe) = write_case(name, source);
    let build = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("build").arg(&src).arg("-o").arg(&exe);
        c
    });
    assert!(
        build.status.success(),
        "[{name}] `noct build` failed (exit {:?}):\nstdout:\n{}\nstderr:\n{}",
        build.status.code(),
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    assert!(
        exe.exists(),
        "[{name}] `noct build` reported success but {} is missing",
        exe.display()
    );
    let run = run_with_timeout(Command::new(&exe));
    let _ = std::fs::remove_dir_all(src.parent().expect("temp dir"));
    run
}

#[test]
fn native_arithmetic_call_print_and_interp() {
    // Call + let-Move + string literal + ToString(Int) + concat + Print +
    // Unit main. The headline case: first native program output.
    let out = build_and_run(
        "arithmetic",
        "fn add(a: Int, b: Int) -> Int:\n    a + b\n\nmain():\n    let x = add(20, 22)\n    print(\"{x}\")\n",
    );
    assert_eq!(out.status.code(), Some(0), "[arithmetic] exit: {:?}", out.status.code());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "42",
        "[arithmetic] stdout"
    );
}

#[test]
fn native_int_main_return_is_dropped() {
    // Source-level `main() -> Int` computes 0, but the binary exits 0
    // with NO stdout — the value is dropped per the entry convention,
    // exactly like `noct run` (which ignores main's return).
    let out = build_and_run(
        "intmain",
        "fn main() -> Int:\n    let q = 100 / 4\n    q - 25\n",
    );
    assert_eq!(out.status.code(), Some(0), "[intmain] exit: {:?}", out.status.code());
    assert!(
        out.stdout.is_empty(),
        "[intmain] expected no stdout, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn native_division_by_zero_traps_loudly() {
    // Same trap class + exit code as `run-vm` (exit 1); stderr carries the
    // native backend's prefix, not the VM's — asserted loosely on purpose
    // (exact prefix text is a backend detail, the trap class is the contract).
    let out = build_and_run(
        "div0",
        "main():\n    let q = 10 / 0\n    print(\"{q}\")\n",
    );
    assert_eq!(out.status.code(), Some(1), "[div0] exit: {:?}", out.status.code());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("division by zero"),
        "[div0] stderr missing trap message: {stderr}"
    );
}

/// `noct run` / `noct run-vm` — no shared state to protect (only `noct
/// build` touches the shared cargo target dir), so these skip `BUILD_LOCK`.
fn run_cli(subcommand: &str, path: &PathBuf) -> Output {
    run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg(subcommand).arg(path);
        c
    })
}

/// Three-way differential check: `noct run` (interpreter), `noct run-vm`
/// (NIR VM), and `noct build`+run (Cranelift native) must agree on exit
/// code and stdout. Stderr is asserted per-case where relevant (trap-class
/// text), never for cross-backend equality — stderr prefixes differ by
/// backend BY DESIGN (see this file's and `differential.rs`'s header
/// comments). Returns the three `Output`s so callers can add their own
/// stderr/trap-class assertions on top.
fn check_three_way(name: &str, source: &str) -> (Output, Output, Output) {
    let (path, _exe) = write_case(name, source);
    let run = run_cli("run", &path);
    let run_vm = run_cli("run-vm", &path);
    let native = build_and_run(name, source);

    assert_eq!(
        run.status.code(), run_vm.status.code(),
        "[{name}] exit code differs: run={:?} run-vm={:?}\n  run stderr:\n{}\n  run-vm stderr:\n{}",
        run.status.code(), run_vm.status.code(),
        String::from_utf8_lossy(&run.stderr), String::from_utf8_lossy(&run_vm.stderr),
    );
    assert_eq!(
        run.status.code(), native.status.code(),
        "[{name}] exit code differs: run={:?} native={:?}\n  run stderr:\n{}\n  native stderr:\n{}",
        run.status.code(), native.status.code(),
        String::from_utf8_lossy(&run.stderr), String::from_utf8_lossy(&native.stderr),
    );
    assert_eq!(
        run.stdout, run_vm.stdout,
        "[{name}] stdout differs: run vs run-vm\n  run:\n{}\n  run-vm:\n{}",
        String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run_vm.stdout),
    );
    assert_eq!(
        run.stdout, native.stdout,
        "[{name}] stdout differs: run vs native\n  run:\n{}\n  native:\n{}",
        String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&native.stdout),
    );

    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }

    (run, run_vm, native)
}

#[test]
fn differential_three_way_arithmetic_call_print() {
    check_three_way(
        "diff3_arithmetic",
        "fn add(a: Int, b: Int) -> Int:\n    a + b\n\nmain():\n    let x = add(20, 22)\n    print(\"{x}\")\n",
    );
}

#[test]
fn differential_three_way_int_main_return_dropped() {
    check_three_way(
        "diff3_intmain",
        "fn main() -> Int:\n    let q = 100 / 4\n    q - 25\n",
    );
}

#[test]
fn differential_three_way_division_by_zero_trap_class() {
    let (run, run_vm, native) = check_three_way(
        "diff3_div0",
        "main():\n    let q = 10 / 0\n    print(\"{q}\")\n",
    );
    assert_eq!(run.status.code(), Some(1), "[diff3_div0] expected exit 1");
    for (label, out) in [("run", &run), ("run-vm", &run_vm), ("native", &native)] {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("division by zero"),
            "[diff3_div0] {label} stderr missing trap message: {stderr}"
        );
    }
}

#[test]
fn native_build_supports_if_else_now() {
    // Was the Step-1 contract check ("if is rejected"); `if`/`else` moved
    // INTO scope with Step 2 (Branch/CondBranch/Phi). Keep this as a
    // positive regression instead of deleting it outright — a future
    // change that silently breaks control-flow codegen should fail a
    // named test, not just the differential suite.
    let out = build_and_run(
        "step2_if",
        "main():\n    let x = 5\n    if x > 3:\n        print(\"big\")\n    else:\n        print(\"small\")\n    print(\"done\")\n",
    );
    assert_eq!(out.status.code(), Some(0), "[step2_if] exit: {:?}", out.status.code());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "bigdone", "[step2_if] stdout");
}

#[test]
fn native_build_rejects_enum_match_loudly_not_silently() {
    // Enum/Option match (EnumTag/EnumPayload) is genuinely Step 3
    // territory — this is the CONTRACT check now: `noct build` must fail
    // LOUDLY naming the function and the unsupported instruction, never
    // silently miscompile, hang, or drop the arm.
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let source = "main():\n    let x: Option<Int> = Some(1)\n    match x:\n        Some(v): print(\"{v}\")\n        None: print(\"none\")\n";
    let (src, exe) = write_case("diff3_unsupported_enum", source);

    let run = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("run").arg(&src);
        c
    });
    assert_eq!(run.status.code(), Some(0), "sanity: Option match must be valid, interpretable Noctivue today");

    let build = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("build").arg(&src).arg("-o").arg(&exe);
        c
    });
    let _ = std::fs::remove_dir_all(src.parent().expect("temp dir"));

    assert_eq!(
        build.status.code(), Some(1),
        "[diff3_unsupported_enum] `noct build` must fail loudly (exit 1), not silently succeed, on aggregates Step 2 doesn't support yet"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("does not yet lower") && stderr.contains("main"),
        "[diff3_unsupported_enum] stderr must name the unsupported instruction and owning function: {stderr}"
    );
}

// ── Step 2: control-flow build-and-run, mirroring differential.rs's
// exact fixture sources so any VM/native behavioral gap shows as a
// stdout diff, not an "are these fixtures actually equivalent" question.

#[test]
fn native_while_loop() {
    let out = build_and_run(
        "step2_while",
        "main():\n    let i = 0\n    while i < 3:\n        print(\"{i}\")\n        i = i + 1\n    print(\"done\")\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "012done");
}

#[test]
fn native_build_rejects_for_loop_loudly_not_silently() {
    // `for` desugars through ListLen/ListIndex (lowering.rs), which are
    // Step 3 aggregate ops — no Cranelift list representation exists yet,
    // so this CANNOT work in Step 2 and must fail loudly instead. Second
    // contract test alongside the enum-match one: each names the exact
    // missing instruction, pinning the Step-3 boundary precisely.
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let source = "main():\n    for n in [10, 20, 30]:\n        print(\"{n}\")\n";
    let (src, exe) = write_case("diff3_unsupported_for", source);

    let run = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("run").arg(&src);
        c
    });
    assert_eq!(run.status.code(), Some(0), "sanity: `for` must be valid, interpretable Noctivue today");

    let build = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("build").arg(&src).arg("-o").arg(&exe);
        c
    });
    let _ = std::fs::remove_dir_all(src.parent().expect("temp dir"));

    assert_eq!(
        build.status.code(), Some(1),
        "[diff3_unsupported_for] `noct build` must fail loudly (exit 1), not silently succeed, on list ops Step 2 doesn't support yet"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("does not yet lower") && stderr.contains("main"),
        "[diff3_unsupported_for] stderr must name the unsupported instruction and owning function: {stderr}"
    );
}

#[test]
fn native_break_while() {
    // break-in-while exercises Branch-to-exit + merge Phis with no list
    // ops involved (unlike break-in-for, which needs Step 3).
    let out = build_and_run(
        "step2_break_while",
        "main():\n    let i = 0\n    while true:\n        print(\"{i}\")\n        i = i + 1\n        if i >= 3:\n            break\n    print(\"done\")\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "012done");
}

#[test]
fn native_continue_while() {
    // continue-in-while exercises the back-edge jump + loop-header Phis.
    let out = build_and_run(
        "step2_continue_while",
        "main():\n    let i = 0\n    while i < 3:\n        i = i + 1\n        if i == 2:\n            continue\n        print(\"{i}\")\n    print(\"done\")\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "13done");
}

#[test]
fn native_int_match() {
    let out = build_and_run(
        "step2_match",
        "main():\n    let x = 2\n    match x:\n        1: print(\"one\")\n        2: print(\"two\")\n        _: print(\"other\")\n    print(\"after\")\n",
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "twoafter");
}

#[test]
fn differential_three_way_while_loop() {
    check_three_way(
        "diff3_while",
        "main():\n    let i = 0\n    while i < 3:\n        print(\"{i}\")\n        i = i + 1\n    print(\"done\")\n",
    );
}

#[test]
fn differential_three_way_while_loop_with_break() {
    check_three_way(
        "diff3_while_break",
        "main():\n    let i = 0\n    while true:\n        print(\"{i}\")\n        i = i + 1\n        if i >= 3:\n            break\n    print(\"done\")\n",
    );
}

// ── Phase 5/M4 host-IO parity (three-way: run / run-vm / native) ──────
// Scalar-shaped builtins lower on all three paths and must agree on
// exit code + stdout (stderr prefixes differ by backend BY DESIGN —
// asserted loosely per-case, never for cross-backend equality).
// Aggregate-returning builtins (db_*, fs_read/write, env_get, …) are
// VM-only: `noct build` fails LOUDLY naming the instruction (pinned by
// `native_build_rejects_vm_only_hostio_loudly` below), never silently.
// Deeper run/run-vm coverage (DB CRUD, HTTP round trips with real
// servers, dotenv) lives in `tests/parity_hostio.rs`, which never
// calls `noct build` and therefore needs no `BUILD_LOCK`.

#[test]
fn differential_three_way_sleep_ok() {
    let (_run, _run_vm, native) = check_three_way(
        "diff3_sleep_ok",
        "fn main():\n    sleep_builtin(0)\n    sleep_builtin(5)\n    print(\"slept\")\n",
    );
    assert_eq!(native.status.code(), Some(0), "[diff3_sleep_ok] exit");
    assert_eq!(String::from_utf8_lossy(&native.stdout), "slept", "[diff3_sleep_ok] stdout");
}

#[test]
fn differential_three_way_sleep_negative_traps() {
    // Negative sleep traps on every path (exit 1, no stdout); the
    // message core (`non-negative`) is shared, the prefix is per
    // backend (`error: [E1002] panic:` / `VM error:` / `noctivue:`).
    let (run, run_vm, native) = check_three_way(
        "diff3_sleep_neg",
        "fn main():\n    sleep_builtin(0 - 1)\n    print(\"unreached\")\n",
    );
    assert_eq!(run.status.code(), Some(1), "[diff3_sleep_neg] run exit");
    for (label, out) in [("run", &run), ("run-vm", &run_vm), ("native", &native)] {
        assert!(out.stdout.is_empty(), "[diff3_sleep_neg] {label} stdout must be empty");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("non-negative"),
            "[diff3_sleep_neg] {label} stderr must name the contract: {stderr}"
        );
    }
}

#[test]
fn differential_three_way_fs_exists() {
    let (_run, _run_vm, native) = check_three_way(
        "diff3_fs_exists",
        "fn main():\n    let here = fs_exists(\"definitely-not-here-xyz\")\n    print(\"{here}\")\n",
    );
    assert_eq!(native.status.code(), Some(0), "[diff3_fs_exists] exit");
    assert_eq!(String::from_utf8_lossy(&native.stdout), "false", "[diff3_fs_exists] stdout");
}

#[test]
fn differential_three_way_io_writes() {
    let (_run, _run_vm, native) = check_three_way(
        "diff3_io_writes",
        "fn main():\n    io_write(\"ab\")\n    io_writeln(\"cd\")\n",
    );
    assert_eq!(native.status.code(), Some(0), "[diff3_io_writes] exit");
    assert_eq!(String::from_utf8_lossy(&native.stdout), "abcd\n", "[diff3_io_writes] stdout");
}

#[test]
fn differential_three_way_env_set() {
    // `env_set` mutates the binary's own process env (unobservable to
    // the harness); the parity contract is "lowers and runs cleanly on
    // all three paths" — semantic round-trips are pinned run/run-vm in
    // `parity_hostio.rs`, where `env_get` (Option-shaped, VM-only) runs.
    let (_run, _run_vm, native) = check_three_way(
        "diff3_env_set",
        "fn main():\n    env_set_builtin(\"NOCTIVUE_DIFF3_MARKER\", \"1\")\n    print(\"set-ok\")\n",
    );
    assert_eq!(native.status.code(), Some(0), "[diff3_env_set] exit");
    assert_eq!(String::from_utf8_lossy(&native.stdout), "set-ok", "[diff3_env_set] stdout");
}

#[test]
fn differential_three_way_log_emit() {
    let (run, run_vm, native) = check_three_way(
        "diff3_log_emit",
        "fn main():\n    log_emit_builtin(\"info\", \"hello-log\")\n    log_emit_builtin(\"warn\", \"careful\")\n    print(\"logged\")\n",
    );
    assert_eq!(run.status.code(), Some(0), "[diff3_log_emit] exit");
    assert_eq!(String::from_utf8_lossy(&native.stdout), "logged", "[diff3_log_emit] stdout");
    for (label, out) in [("run", &run), ("run-vm", &run_vm), ("native", &native)] {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("[INFO] hello-log"), "[diff3_log_emit] {label}: {stderr}");
        assert!(stderr.contains("[WARN] careful"), "[diff3_log_emit] {label}: {stderr}");
    }
}

#[test]
fn differential_three_way_http_register_shutdown() {
    // Register + shutdown are unconditionally `Unit` (unknown handles
    // are ignored, never errors) — identical on all three paths.
    let (_run, _run_vm, native) = check_three_way(
        "diff3_http_reg_shutdown",
        "fn main():\n    http_server_register_route(1, \"GET\", \"/hi\", \"h\")\n    http_server_shutdown(1)\n    print(\"routes-ok\")\n",
    );
    assert_eq!(native.status.code(), Some(0), "[diff3_http_reg_shutdown] exit");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        "routes-ok",
        "[diff3_http_reg_shutdown] stdout"
    );
}

#[test]
fn native_build_rejects_vm_only_hostio_loudly() {
    // VM-only builtins (aggregate Result/Option/List returns, plus
    // `serve_loop`'s program-code callbacks) have no native value
    // representation yet (Step 3 boundary): `noct build` must fail
    // LOUDLY naming the function and the unsupported instruction —
    // never silently miscompile, hang, or drop the call. Each case is
    // well-typed (a type error would fail earlier with E0200, which
    // this assertion would NOT match), and each fails before cargo
    // link, so the whole table runs in milliseconds.
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cases: &[(&str, &str, &str)] = &[
        // NOTE: `http_send` takes a headers list (`[]`), and list
        // literals (`struct_new`) are Step 3 territory — so today this
        // case fails on `struct_new`, not `http_send`. The empty
        // `instr` means "any clean UnsupportedInstr rejection"; once
        // Step 3 lands, pin this back to `http_send`.
        ("hostio_nohttp_send", "fn main():\n    http_send_builtin(\"GET\", \"http://127.0.0.1:9/\", [], \"\")\n", ""),
        ("hostio_nolisten", "fn main():\n    http_server_listen(0)\n", "http_server_listen"),
        ("hostio_noserve", "fn main():\n    http_server_serve_loop(1)\n", "http_server_serve_loop"),
        ("hostio_nodb_open", "fn main():\n    db_open_builtin(\":memory:\")\n", "db_open"),
        ("hostio_nodb_exec", "fn main():\n    db_exec_builtin(1, \"SELECT 1\", \"[]\")\n", "db_exec"),
        ("hostio_nodb_query", "fn main():\n    db_query_builtin(1, \"SELECT 1\", \"[]\")\n", "db_query"),
        ("hostio_nodb_close", "fn main():\n    db_close_builtin(1)\n", "db_close"),
        ("hostio_nofs_read", "fn main():\n    fs_read_text(\"x\")\n", "fs_read"),
        ("hostio_nofs_write", "fn main():\n    fs_write_text(\"x\", \"y\")\n", "fs_write"),
        ("hostio_nofs_mtime", "fn main():\n    fs_modified_millis_builtin(\"x\")\n", "fs_modified_millis"),
        ("hostio_noenv_get", "fn main():\n    env_get_builtin(\"x\")\n", "env_get"),
        ("hostio_noconfig", "fn main():\n    config_get_builtin(\"x\", \"y\")\n", "config_get"),
        ("hostio_nodotenv", "fn main():\n    dotenv_load_builtin(\"x\")\n", "dotenv_load"),
    ];
    for (name, source, instr) in cases {
        let (src, exe) = write_case(name, source);
        let build = run_with_timeout({
            let mut c = Command::new(noct_bin());
            c.arg("build").arg(&src).arg("-o").arg(&exe);
            c
        });
        let _ = std::fs::remove_dir_all(src.parent().expect("temp dir"));
        assert_eq!(
            build.status.code(),
            Some(1),
            "[{name}] `noct build` must fail loudly (exit 1), not silently succeed"
        );
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(
            stderr.contains("does not yet lower") && stderr.contains("main"),
            "[{name}] stderr must be a clean unsupported-instruction rejection naming the owning function: {stderr}"
        );
        if !instr.is_empty() {
            assert!(
                stderr.contains(instr),
                "[{name}] stderr must name the unsupported instruction `{instr}`: {stderr}"
            );
        }
    }
}

#[test]
fn hostio_tasks_refused_on_vm_and_native() {
    // The task family stays loud on both non-interp paths (the
    // interpreter is the only task-capable backend): `run-vm` and
    // `build` refuse with the same "async runtime" contract text.
    // (`noct run` succeeds — pinned run-side by `tests/tasks.rs` and
    // `parity_hostio.rs`.)
    let source = "task w():\n    1\nfn main():\n    await w()\n";
    let (path, _exe) = write_case("hostio_task_vm", source);
    let run_vm = run_cli("run-vm", &path);
    assert_eq!(run_vm.status.code(), Some(1), "[hostio_task] run-vm exit");
    assert!(
        String::from_utf8_lossy(&run_vm.stderr).contains("requires the async runtime"),
        "[hostio_task] run-vm stderr: {}",
        String::from_utf8_lossy(&run_vm.stderr),
    );
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (src2, exe) = write_case("hostio_task_build", source);
    let build = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("build").arg(&src2).arg("-o").arg(&exe);
        c
    });
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    let _ = std::fs::remove_dir_all(src2.parent().expect("temp dir"));
    assert_eq!(build.status.code(), Some(1), "[hostio_task] build exit");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("requires the async runtime") && stderr.contains("does not support tasks"),
        "[hostio_task] build stderr: {stderr}"
    );
}
