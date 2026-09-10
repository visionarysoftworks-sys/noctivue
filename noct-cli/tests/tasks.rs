//! CLI tests for `task` / `await` / `sleep` surface (Phase 5/M4).
//!
//! Each case runs `noct run <file>` on a scratch `.nv` source; pass
//! = exit 0, fail = named diagnostics and exit 1. The `run` binary
//! is the only task-capable backend (`run-vm` and `build` refuse
//! tasks loudly).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 30;

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

fn scratch_dir() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-tasktest-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn task_spawn_await_value_cli() {
    let dir = scratch_dir();
    let src = dir.join("task_val.nv");
    std::fs::write(
        &src,
        r#"task answer():
    40 + 2
fn main():
    let h = answer()
    let v = await h
    println("value={v}")
"#,
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("value=42"), "stdout: {stdout}");
}

// NOTE: tasks_overlap timing test is in tests/async_test.rs
// (runs in-process without CLI cold-start overhead). CLI overhead
// dominates at the sub-second level, making parallelism claims
// flaky in a process-per-case harness.

#[test]
fn await_unknown_handle_cli() {
    let dir = scratch_dir();
    let src = dir.join("bad_await.nv");
    std::fs::write(&src, "fn main():\n    await 5\n").unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown task handle"));
}

#[test]
fn double_await_fails_cli() {
    let dir = scratch_dir();
    let src = dir.join("dbl_await.nv");
    std::fs::write(
        &src,
        "task w():\n    1\nfn main():\n    let h = w()\n    await h\n    await h\n",
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already awaited"));
}

#[test]
fn drain_unawaited_task_cli() {
    let dir = scratch_dir();
    let marker = dir.join("drained.txt");
    let src = dir.join("drain.nv");
    std::fs::write(
        &src,
        format!(
            r#"task bg():
    fs_write_text("{}", "drained")
fn main():
    let h = bg()
"#,
            marker.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let body = std::fs::read_to_string(&marker).expect("drained task never wrote");
    assert_eq!(body, "drained");
}

#[test]
fn sleep_builtin_cli() {
    let dir = scratch_dir();
    let src = dir.join("sleep.nv");
    std::fs::write(
        &src,
        "fn main():\n    sleep_builtin(0)\n    sleep_builtin(5)\n",
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn sleep_negative_panics_cli() {
    let dir = scratch_dir();
    let src = dir.join("sleep_bad.nv");
    std::fs::write(&src, "fn main():\n    sleep_builtin(0 - 1)\n").unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("non-negative"));
}

#[test]
fn run_vm_refuses_task_cli() {
    let dir = scratch_dir();
    let src = dir.join("task_vm.nv");
    std::fs::write(
        &src,
        r#"task w():
    1
fn main():
    await w()
"#,
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run-vm", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("requires the async runtime"));
}

#[test]
fn build_refuses_task_cli() {
    let dir = scratch_dir();
    let src = dir.join("task_build.nv");
    std::fs::write(
        &src,
        r#"task w():
    1
fn main():
    await w()
"#,
    )
    .unwrap();
    let out = run_cli_in(&dir, &["build", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support tasks"));
}
