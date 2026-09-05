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
    let _guard = BUILD_LOCK.lock().expect("build lock poisoned");
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
fn native_build_rejects_control_flow_loudly_not_silently() {
    // `if` is valid, interpretable Noctivue today (Phase 1/2) but is
    // Step-2 territory for native (`compile_function` rejects any
    // function with more than one block). This is the CONTRACT check
    // (NIR.md's "no silent recovery" discipline): `noct build` must fail
    // LOUDLY, naming the function and the reason, never silently
    // miscompile, hang, or drop the branch.
    let _guard = BUILD_LOCK.lock().expect("build lock poisoned");
    let source = "main():\n    let x = 5\n    if x > 3:\n        print(\"big\")\n    else:\n        print(\"small\")\n";
    let (src, exe) = write_case("diff3_unsupported_cf", source);

    let run = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("run").arg(&src);
        c
    });
    assert_eq!(
        run.status.code(), Some(0),
        "sanity: `if` must be valid, interpretable Noctivue today"
    );

    let build = run_with_timeout({
        let mut c = Command::new(noct_bin());
        c.arg("build").arg(&src).arg("-o").arg(&exe);
        c
    });
    let _ = std::fs::remove_dir_all(src.parent().expect("temp dir"));

    assert_eq!(
        build.status.code(), Some(1),
        "[diff3_unsupported_cf] `noct build` must fail loudly (exit 1), not silently succeed, on control flow Step 1 doesn't support yet"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("does not yet lower") && stderr.contains("main"),
        "[diff3_unsupported_cf] stderr must name the unsupported instruction and owning function: {stderr}"
    );
}
