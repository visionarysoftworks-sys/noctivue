//! CLI tests for `noct completions` (Phase 4).
//!
//! The stubs are honest and small by design: command names plus
//! file/path arguments only. Tests assert every supported shell exits
//! 0 and lists the command surface, that no per-command flags are
//! over-claimed, and that unknown/missing shells fail loudly.

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

fn scratch() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-completions-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// Every command name the stub must complete (mirrors main.rs).
const COMMANDS: &[&str] = &[
    "run",
    "run-vm",
    "test",
    "ast",
    "diagnostics",
    "build",
    "create",
    "fmt",
    "lint",
    "add",
    "audit",
    "publish",
    "doc",
    "vendor",
    "outdated",
    "completions",
];

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn completions_each_shell_lists_commands_and_files() {
    let dir = scratch();
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let out = run_cli_in(&dir, &["completions", shell]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{shell} exit != 0. stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        for cmd in COMMANDS {
            assert!(
                stdout.contains(cmd),
                "{shell} stub missing command `{cmd}`, got:\n{stdout}"
            );
        }
        // Honest stubs: command names + filenames only — per-command
        // flags must NOT be claimed.
        for flag in ["--opt-in", "--rotate-key", "--dry-run"] {
            assert!(
                !stdout.contains(flag),
                "{shell} stub over-claims flag completion `{flag}`, got:\n{stdout}"
            );
        }
        assert!(
            !stdout.trim().is_empty(),
            "{shell} stub must not be empty"
        );
    }
    cleanup(&dir);
}

#[test]
fn completions_rejects_unknown_shell() {
    let dir = scratch();
    let out = run_cli_in(&dir, &["completions", "csh"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unsupported shell"), "got:\n{stderr}");
    assert!(stderr.contains("csh"), "must echo the bad name, got:\n{stderr}");
    assert!(stderr.contains("bash"), "must list supported shells, got:\n{stderr}");
    assert!(out.stdout.is_empty(), "usage errors go to stderr, not stdout");
    cleanup(&dir);
}

#[test]
fn completions_needs_a_shell() {
    let dir = scratch();
    let out = run_cli_in(&dir, &["completions"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("shell name required"), "got:\n{stderr}");

    let out = run_cli_in(&dir, &["completions", "bash", "extra"]);
    assert_eq!(out.status.code(), Some(1));
    cleanup(&dir);
}
