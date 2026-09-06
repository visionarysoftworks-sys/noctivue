//! CLI tests for `noct audit` (Phase 4, Slice C).
//!
//! Trust rows come from lock data alone: `name version tier
//! signed_by audit-status`. Statuses: `local` for path sources,
//! `signed` for registry/git (presence enforced at parse; values
//! verified at fetch), vuln column always `unknown` until M5.

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

fn scratch(keys: bool) -> (PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-audit-{}-{id}", std::process::id()));
    let dir = base.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let keydir = base.join("keys");
    if keys {
        std::fs::create_dir_all(&keydir).unwrap();
    }
    (base, dir)
}

fn cleanup(base: &PathBuf) {
    let _ = std::fs::remove_dir_all(base);
}

const APP_MANIFEST: &str = "package:\n    name: myapp\n    version: 0.1.0\n";
const SIBLING_MANIFEST: &str = "package:\n    name: sibling\n    version: 0.3.5\n";

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn audit_shows_path_row_as_local() {
    let (base, dir) = scratch(false);
    std::fs::create_dir_all(dir.join("sibling")).unwrap();
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).unwrap();
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), SIBLING_MANIFEST).unwrap();
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"]).status.code(),
        Some(0)
    );
    let out = run_cli_in(&dir, &["audit"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sibling"), "got:\n{stdout}");
    assert!(stdout.contains("0.3.5"), "got:\n{stdout}");
    assert!(stdout.contains("native"), "got:\n{stdout}");
    assert!(stdout.contains("local"), "path rows are local, got:\n{stdout}");
    assert!(stdout.contains("no vuln database"), "got:\n{stdout}");
    cleanup(&base);
}

#[test]
fn audit_needs_a_lockfile() {
    let (base, dir) = scratch(false);
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).unwrap();
    let out = run_cli_in(&dir, &["audit"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no nestpkg.lock"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&base);
}
