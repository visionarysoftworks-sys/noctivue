//! CLI tests for `noct add` path flow (package manager slice 2).
//!
//! Offline only: every case builds a scratch project tree (a manifest
//! plus a path-dependency target) under the temp dir, runs `add`, and
//! asserts on exit codes plus exact `nestpkg.nvpm` / `nestpkg.lock`
//! bytes. Registry behavior is asserted as a loud typed stub.
//! Follows the `fmt.rs`/`differential.rs` temp-file harness pattern.

use std::io::Write;
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

/// Scratch project: `<dir>/nestpkg.nvpm` (the adder) plus
/// `<dir>/sibling/nestpkg.nvpm` (the path target).
fn scratch_project(manifest: &str, sibling_manifest: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-add-{}-{id}", std::process::id()));
    std::fs::create_dir_all(dir.join("sibling")).expect("create scratch dirs");
    std::fs::write(dir.join("nestpkg.nvpm"), manifest).expect("write manifest");
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), sibling_manifest)
        .expect("write sibling manifest");
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

fn read(dir: &PathBuf, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).expect("read back file")
}

const APP_MANIFEST: &str = "package:\n    name: myapp\n    version: 0.1.0\n";
const SIBLING_MANIFEST: &str = "package:\n    name: sibling\n    version: 0.3.5\n";

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn add_path_writes_manifest_and_lock() {
    let dir = scratch_project(APP_MANIFEST, SIBLING_MANIFEST);
    let out = run_cli_in(&dir, &["add", "sibling", "--path", "sibling"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "add exit != 0. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        read(&dir, "nestpkg.nvpm"),
        "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    sibling:\n        version: =0.3.5\n        path: sibling\n"
    );
    assert_eq!(
        read(&dir, "nestpkg.lock"),
        "lock_version: 1\npackages:\n    sibling:\n        version: 0.3.5\n        source:\n            path: sibling\n        tier: native\n"
    );
    cleanup(&dir);
}

#[test]
fn add_path_idempotent_second_run() {
    let dir = scratch_project(APP_MANIFEST, SIBLING_MANIFEST);
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    let first_manifest = read(&dir, "nestpkg.nvpm");
    let first_lock = read(&dir, "nestpkg.lock");
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(read(&dir, "nestpkg.nvpm"), first_manifest);
    assert_eq!(read(&dir, "nestpkg.lock"), first_lock);
    cleanup(&dir);
}

#[test]
fn add_path_records_tier_and_rejects_bare_opt_in() {
    let dir = scratch_project(APP_MANIFEST, SIBLING_MANIFEST);
    // foreign-runtime without --opt-in: refused before any write.
    let out = run_cli_in(
        &dir,
        &[
            "add",
            "sibling",
            "--path",
            "sibling",
            "--tier",
            "foreign-runtime",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--opt-in"),
        "must cite --opt-in, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dir.join("nestpkg.lock").exists(),
        "refused add must not write a lock"
    );

    // With --opt-in: recorded in both files.
    let out = run_cli_in(
        &dir,
        &[
            "add",
            "sibling",
            "--path",
            "sibling",
            "--tier",
            "foreign-runtime",
            "--opt-in",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(read(&dir, "nestpkg.nvpm").contains("tier: foreign-runtime"));
    assert!(read(&dir, "nestpkg.nvpm").contains("opt_in: true"));
    assert!(read(&dir, "nestpkg.lock").contains("tier: foreign-runtime"));
    cleanup(&dir);
}

#[test]
fn add_rejects_name_mismatch_and_bad_req() {
    let dir = scratch_project(APP_MANIFEST, SIBLING_MANIFEST);
    // Wrong name for the target tree.
    let out = run_cli_in(&dir, &["add", "other", "--path", "sibling"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("name mismatch"));
    // Unsatisfiable requirement.
    let out = run_cli_in(&dir, &["add", "sibling@=9.9.9", "--path", "sibling"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not satisfy"));
    // Missing target manifest.
    let out = run_cli_in(&dir, &["add", "ghost", "--path", "nosuchdir"]);
    assert_eq!(out.status.code(), Some(1));
    cleanup(&dir);
}

#[test]
fn add_without_manifest_and_registry_stub() {
    // No nestpkg.nvpm in cwd: loud error, no stub behavior.
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-add-empty-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create empty dir");
    let out = run_cli_in(&dir, &["add", "sibling", "--path", "sibling"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no nestpkg.nvpm"));

    // Registry requirement without --index: loud error naming the
    // missing index flag (no default registry exists), writes nothing.
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    let out = run_cli_in(&dir, &["add", "http@1.2.0"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--index"),
        "must name the --index flag, got:\n{stderr}"
    );
    assert!(
        !dir.join("nestpkg.lock").exists(),
        "stub must not write a lock"
    );
    cleanup(&dir);
}
