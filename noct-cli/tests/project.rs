//! CLI tests for `noct test` project mode (Phase 4, Slice A).
//!
//! In a directory with `nestpkg.nvpm` (and no `tests/fixtures`),
//! `noct test` executes every `tests/**/*.nv` through the `run`
//! pipeline: pass = exit 0, fail names the file and exits 1,
//! `// @skip-test` skips. Workspace mode is unchanged (covered by
//! running the suite at the repo root, not here).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

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

fn scratch_project() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-projtest-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let out = run_cli_in(&dir, &["create", "shop"]);
    assert_eq!(out.status.code(), Some(0));
    dir.join("shop")
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn project_test_runs_generated_smoke() {
    let shop = scratch_project();
    let out = run_cli_in(&shop, &["test"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("PASS"), "got:\n{stdout}");
    assert!(stdout.contains("1 passed, 0 failed"), "got:\n{stdout}");
    cleanup(&shop);
}

#[test]
fn project_test_fail_skip_and_filter() {
    let shop = scratch_project();
    std::fs::write(
        shop.join("tests/broken_test.nv"),
        "main():\n    assert(1 == 2, \"boom\")\n",
    )
    .expect("write failing test");
    std::fs::write(
        shop.join("tests/skipped_test.nv"),
        "// @skip-test (concat-only demo)\nmain():\n    println(\"never runs\")\n",
    )
    .expect("write skipped test");

    let out = run_cli_in(&shop, &["test"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL") && stdout.contains("broken_test"), "got:\n{stdout}");
    assert!(stdout.contains("SKIP") && stdout.contains("skipped_test"), "got:\n{stdout}");
    assert!(stdout.contains("1 passed, 1 failed, 1 skipped"), "got:\n{stdout}");

    // Filter to the passing smoke only: green again.
    let out = run_cli_in(&shop, &["test", "main_test"]);
    assert_eq!(out.status.code(), Some(0));
    cleanup(&shop);
}

#[test]
fn project_test_empty_tests_dir_warns() {
    let shop = scratch_project();
    std::fs::remove_file(shop.join("tests/main_test.nv")).expect("drop smoke");
    let out = run_cli_in(&shop, &["test"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no .nv tests"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&shop);
}
