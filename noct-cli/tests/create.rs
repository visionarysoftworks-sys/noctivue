//! CLI tests for `noct create` (project-creation spec, minimal slice).
//!
//! Each case runs `create` inside a scratch temp dir (the command
//! creates `./<name>/` under the CWD) and asserts exact tree bytes,
//! refusal behavior, and — strongest of all — that the generated
//! project actually RUNS (`noct run` on its entry point and smoke).
//! Follows the `add.rs`/`fmt.rs` temp-dir harness pattern.

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
    let dir = std::env::temp_dir().join(format!("noctivue-create-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).expect("read back generated file")
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn create_generates_canonical_tree_byte_exact() {
    let dir = scratch_dir();
    let out = run_cli_in(&dir, &["create", "hello_world"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "create exit != 0. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let root = dir.join("hello_world");
    assert_eq!(
        read(&dir, "hello_world/nestpkg.nvpm"),
        "package:\n    name: hello_world\n    version: 0.1.0\n    description: Created with `noct create`.\n"
    );
    assert_eq!(
        read(&dir, "hello_world/lib/main.nv"),
        "//! hello_world — created by `noct create`.\nmain():\n    println(\"Hello from hello_world!\")\n"
    );
    assert_eq!(
        read(&dir, "hello_world/tests/main_test.nv"),
        "//! Smoke test for hello_world (run: noct run tests/main_test.nv).\nfn double(x: Int) -> Int:\n    x * 2\n\nmain():\n    assert(double(21) == 42, \"double works\")\n    println(\"tests ok\")\n"
    );
    assert_eq!(
        read(&dir, "hello_world/docs/overview.md"),
        "# hello_world — overview\n\nShort project documentation lives here.\n"
    );
    assert_eq!(
        read(&dir, "hello_world/README.md"),
        "# hello_world\n\nCreated with `noct create`.\n\nRun it:\n\n    noct run lib/main.nv\n"
    );
    // No lockfile (established by add/build, never invented empty),
    // no target/platform extras (minimal stays minimal).
    assert!(!root.join("nestpkg.lock").exists());
    assert!(!root.join("windows").exists());
    cleanup(&dir);
}

#[test]
fn create_output_runs_and_tests_pass() {
    let dir = scratch_dir();
    assert_eq!(
        run_cli_in(&dir, &["create", "demo_app"]).status.code(),
        Some(0)
    );
    // Entry point runs.
    let out = run_cli_in(&dir, &["run", "demo_app/lib/main.nv"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "entry point failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Hello from demo_app!\n"
    );
    // Smoke test passes (exit 0 proves its asserts).
    let out = run_cli_in(&dir, &["run", "demo_app/tests/main_test.nv"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "smoke failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "tests ok\n");
    // Generated manifest parses under the package rules.
    let out = run_cli_in(&dir, &["diagnostics", "demo_app/lib/main.nv"]);
    assert_eq!(out.status.code(), Some(0));
    cleanup(&dir);
}

#[test]
fn create_refuses_bad_names_and_existing_paths() {
    let dir = scratch_dir();
    for bad in ["Hello", "9lives", "has-dash", "core"] {
        let out = run_cli_in(&dir, &["create", bad]);
        assert_eq!(out.status.code(), Some(1), "{bad} must fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("invalid project name"),
            "{bad} must cite the naming rule, got:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !dir.join(bad).exists(),
            "failed create must leave nothing behind"
        );
    }
    // Missing name.
    let out = run_cli_in(&dir, &["create"]);
    assert_eq!(out.status.code(), Some(1));

    // Existing destination (file or dir) is refused.
    assert_eq!(
        run_cli_in(&dir, &["create", "taken"]).status.code(),
        Some(0)
    );
    let out = run_cli_in(&dir, &["create", "taken"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already exists"));
    std::fs::write(dir.join("afile"), b"x").expect("write sentinel");
    let out = run_cli_in(&dir, &["create", "afile"]);
    assert_eq!(out.status.code(), Some(1));
    cleanup(&dir);
}

#[test]
fn create_is_deterministic() {
    let dir = scratch_dir();
    assert_eq!(run_cli_in(&dir, &["create", "same"]).status.code(), Some(0));
    let first: Vec<(String, String)> = ["nestpkg.nvpm", "lib/main.nv", "tests/main_test.nv"]
        .iter()
        .map(|f| (f.to_string(), read(&dir, &format!("same/{f}"))))
        .collect();
    cleanup(&dir);
    let dir = scratch_dir();
    assert_eq!(run_cli_in(&dir, &["create", "same"]).status.code(), Some(0));
    for (file, content) in &first {
        assert_eq!(
            &read(&dir, &format!("same/{file}")),
            content,
            "{file} differs across runs"
        );
    }
    cleanup(&dir);
}
