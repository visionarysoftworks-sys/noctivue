//! End-to-end package flow (package manager, item 2): the exact
//! loop Phase 4's exit criteria demands of a second developer, minus
//! the two documented futures (project-aware `test` discovery and
//! native `build`, both covered elsewhere — `differential.rs` and
//! `native_build.rs`).
//!
//! One scratch project: `create` → `add --path` → `run` the entry
//! point → `fmt --check` clean → `lint` clean → `publish --dry-run`
//! ready. Every step asserts exit codes; the generated tree is byte
//! checked by `create.rs`, so this file checks the *composition*.

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

fn expect_ok(dir: &Path, args: &[&str]) -> Output {
    let out = run_cli_in(dir, args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "`noct {}` failed. stderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn second_developer_loop_create_add_run_fmt_lint_publish() {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-e2e-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");

    // 1. Scaffold the app and its path dependency.
    expect_ok(&dir, &["create", "shop"]);
    expect_ok(&dir, &["create", "money"]);
    // `add` runs inside the project dir; the sibling lives next to it.
    let shop = dir.join("shop");
    std::fs::rename(dir.join("money"), shop.join("money")).expect("nest sibling");

    // 2. Declare the dependency.
    expect_ok(&shop, &["add", "money", "--path", "money"]);
    assert!(shop.join("nestpkg.lock").exists());

    // 3. The entry point runs.
    let out = expect_ok(&shop, &["run", "lib/main.nv"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello from shop!\n");

    // 4. The generated tree is fmt-clean and lint-clean.
    expect_ok(
        &shop,
        &["fmt", "--check", "lib/main.nv", "tests/main_test.nv"],
    );
    expect_ok(&shop, &["lint", "lib/main.nv", "tests/main_test.nv"]);

    // 5. The package is publish-ready (dry run).
    let out = expect_ok(&shop, &["publish", "--dry-run"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ready to publish (dry run"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
