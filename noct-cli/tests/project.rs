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

/// Serializes `noct build` cases: every build funnels through one shared
/// cargo target dir (`<temp>/noctivue-target-cache`, see `cmd_build.rs`)
/// whose link output is always `noctivue_shim{EXE_SUFFIX}` — concurrent
/// builds would race on the copy (see `native_build.rs` `BUILD_LOCK`).
static BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 15;

/// Cold-cache `noct build` compiles `runtime-native` from scratch;
/// successful builds need the generous timeout (`native_build.rs` uses
/// the same 300s). Failing `--frozen` builds exit before cargo (fast)
/// but share the helper so they also serialize on `BUILD_LOCK`.
const BUILD_TIMEOUT_SECS: u64 = 300;

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

fn run_build_in(dir: &Path, args: &[&str]) -> Output {
    let _guard = BUILD_LOCK.lock().expect("build lock poisoned");
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
    match rx.recv_timeout(std::time::Duration::from_secs(BUILD_TIMEOUT_SECS)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => panic!("failed waiting on noct binary: {e}"),
        Err(_) => panic!(
            "TIMEOUT (>{BUILD_TIMEOUT_SECS}s): `noct {}` hung",
            args.join(" ")
        ),
    }
}

/// Scratch project plus one consistent path dependency (`sibling@0.3.5`):
/// manifest entry + lockfile entry agree, so `--frozen` passes.
fn shop_with_path_dep() -> PathBuf {
    let shop = scratch_project();
    std::fs::create_dir_all(shop.join("sibling")).expect("sibling dir");
    std::fs::write(
        shop.join("sibling").join("nestpkg.nvpm"),
        "package:\n    name: sibling\n    version: 0.3.5\n",
    )
    .expect("write sibling manifest");
    let out = run_cli_in(&shop, &["add", "sibling", "--path", "sibling"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "add failed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(shop.join("nestpkg.lock").exists(), "add must write a lock");
    shop
}

/// Hand-edit the requirement past the locked version (same drift pattern
/// as `publish.rs`): lock still pins 0.3.5, manifest demands =9.9.9.
fn make_stale(shop: &Path) {
    let manifest_path = shop.join("nestpkg.nvpm");
    let before = std::fs::read_to_string(&manifest_path).expect("read manifest");
    assert!(
        before.contains("=0.3.5"),
        "expected locked req =0.3.5 in:\n{before}"
    );
    std::fs::write(manifest_path, before.replace("=0.3.5", "=9.9.9")).expect("rewrite manifest");
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

// ── `noct build --frozen` (reproducible builds) ─────────────────────────────

#[test]
fn build_frozen_with_consistent_lock_succeeds() {
    let shop = shop_with_path_dep();
    let out = run_build_in(&shop, &["build", "lib/main.nv", "--frozen"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "frozen build with current lock must succeed. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Built"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    cleanup(&shop);
}

#[test]
fn build_frozen_missing_lock_fails() {
    let shop = shop_with_path_dep();
    std::fs::remove_file(shop.join("nestpkg.lock")).expect("drop lock");
    let out = run_build_in(&shop, &["build", "lib/main.nv", "--frozen"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("missing"), "must say missing, got:\n{stderr}");
    assert!(
        stderr.contains("nestpkg.lock"),
        "must name the lockfile, got:\n{stderr}"
    );
    assert!(stderr.contains("--frozen"), "must cite --frozen, got:\n{stderr}");

    // Hidden alias `--locked` enforces identically (fast-fail, no cargo).
    let out = run_build_in(&shop, &["build", "lib/main.nv", "--locked"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("missing"),
        "alias must fail the same way, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&shop);
}

#[test]
fn build_frozen_stale_lock_fails() {
    let shop = shop_with_path_dep();
    make_stale(&shop);
    let out = run_build_in(&shop, &["build", "lib/main.nv", "--frozen"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("sibling"), "must name the package, got:\n{stderr}");
    assert!(
        stderr.contains("does not satisfy") || stderr.contains("not current"),
        "must cite the version violation, got:\n{stderr}"
    );
    cleanup(&shop);
}

#[test]
fn build_without_frozen_ignores_stale_lock() {
    // Same stale setup as above, WITHOUT the flag: no new failure —
    // the build proceeds to codegen (proves the gate is opt-in).
    let shop = shop_with_path_dep();
    make_stale(&shop);
    let out = run_build_in(&shop, &["build", "lib/main.nv"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stale lock without --frozen must still build. stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Built"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    cleanup(&shop);
}
