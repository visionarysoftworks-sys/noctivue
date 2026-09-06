//! CLI tests for `noct publish --dry-run` (package manager, item 2).
//!
//! There is no registry and no signature verification (TOOLCHAIN.md
//! §3 hard precondition), so a real publish is a typed refusal that
//! writes nothing. `--dry-run` validates manifest + lockfile locally:
//! parse, presence rules, and lock-current drift. Harness mirrors
//! `add.rs` (scratch temp dirs, `run_cli_in`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 10;

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
    let dir = std::env::temp_dir().join(format!("noctivue-publish-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

const APP_MANIFEST: &str = "package:\n    name: myapp\n    version: 0.1.0\n";
const SIBLING_MANIFEST: &str = "package:\n    name: sibling\n    version: 0.3.5\n";

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn publish_dry_run_ready_on_fresh_project() {
    // No deps, no lock: nothing to lock, ready.
    let dir = scratch_dir();
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("manifest ok"), "got:\n{stdout}");
    assert!(stdout.contains("nothing to lock"), "got:\n{stdout}");
    assert!(
        stdout.contains("ready to publish (dry run"),
        "got:\n{stdout}"
    );
    cleanup(&dir);
}

#[test]
fn publish_dry_run_ready_after_add() {
    let dir = scratch_dir();
    std::fs::create_dir_all(dir.join("sibling")).expect("sibling dir");
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), SIBLING_MANIFEST)
        .expect("write sibling");
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lockfile current (1 packages)"),
        "got:\n{stdout}"
    );
    cleanup(&dir);
}

#[test]
fn publish_dry_run_reports_drift_and_missing_lock() {
    let dir = scratch_dir();
    std::fs::create_dir_all(dir.join("sibling")).expect("sibling dir");
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), SIBLING_MANIFEST)
        .expect("write sibling");
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    // Hand-edit the requirement past the locked version: drift.
    let edited = std::fs::read_to_string(dir.join("nestpkg.nvpm"))
        .expect("read")
        .replace("=0.3.5", "=9.9.9");
    std::fs::write(dir.join("nestpkg.nvpm"), edited).expect("rewrite");
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("drift"), "got:\n{stderr}");
    assert!(stderr.contains("sibling"), "got:\n{stderr}");

    // Deps but no lock at all: missing lock.
    std::fs::remove_file(dir.join("nestpkg.lock")).expect("drop lock");
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("lockfile missing"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&dir);
}

#[test]
fn publish_without_flag_is_typed_refusal_writing_nothing() {
    let dir = scratch_dir();
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    let out = run_cli_in(&dir, &["publish"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no package registry"), "got:\n{stderr}");
    assert!(
        stderr.contains("signing"),
        "must cite the signing precondition, got:\n{stderr}"
    );
    assert!(
        stderr.contains("--dry-run"),
        "must point at dry-run, got:\n{stderr}"
    );
    assert!(
        !dir.join("nestpkg.lock").exists(),
        "refusal must write nothing"
    );
    cleanup(&dir);
}

#[test]
fn publish_dry_run_needs_a_manifest() {
    let dir = scratch_dir();
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no nestpkg.nvpm"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&dir);
}
