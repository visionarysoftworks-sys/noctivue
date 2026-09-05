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
