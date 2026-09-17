//! CLI tests for `derive Serialize|Deserialize` (Phase 5/M4, ADR-018).
//!
//! - `fmt --v2` round-trips derive declarations byte-stable.
//! - `lint` is clean on deriving programs.
//! - `run` executes an encode/decode round trip end to end.
//! - `run-vm` refuses derived programs loudly (exit 1): the VM has
//!   no `doc_*` builtins (pre-existing gap, same as the rest of the
//!   Phase-5 stdlib surface).

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
    let dir = std::env::temp_dir().join(format!("noctivue-derivetest-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn libs() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let files = [
        "stdlib/encoding/json/serialize.nv",
        "stdlib/encoding/json/document.nv",
    ];
    let mut out = String::new();
    for f in files {
        out.push_str(&std::fs::read_to_string(root.join(f)).expect("read lib"));
        out.push('\n');
    }
    out
}

const SHAPE: &str = r#"User:
    name: String
    age: Int

derive Serialize for User:
derive Deserialize for User:
"#;

#[test]
fn fmt_v2_derive_roundtrip_cli() {
    let dir = scratch_dir();
    let src = dir.join("derive_fmt.nv");
    std::fs::write(&src, SHAPE).unwrap();
    let out = run_cli_in(&dir, &["fmt", "--v2", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let body = std::fs::read_to_string(&src).unwrap();
    assert!(body.contains("derive Serialize for User:\n"));
    assert!(body.contains("derive Deserialize for User:\n"));
    // Second run is stable (idempotency gate would also refuse).
    let out = run_cli_in(&dir, &["fmt", "--v2", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&src).unwrap(), body);
}

#[test]
fn lint_derive_clean_cli() {
    let dir = scratch_dir();
    let src = dir.join("derive_lint.nv");
    std::fs::write(&src, libs() + SHAPE + "fn main():\n    1\n").unwrap();
    let out = run_cli_in(&dir, &["lint", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("warning"));
}

#[test]
fn run_derive_roundtrip_cli() {
    let dir = scratch_dir();
    let src = dir.join("derive_run.nv");
    std::fs::write(
        &src,
        libs() + SHAPE
            + r#"fn main():
    let u = User { name: "Ada", age: 36 }
    match doc_parse(to_json_User(u)):
        Ok(d):
            match from_json_User(d):
                Ok(back):
                    println(to_json_User(back))
                Err(e):
                    println("DECODE ERR")
        Err(e):
            println("PARSE ERR")
"#,
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"Ada\""), "stdout: {stdout}");
    assert!(stdout.contains("36"), "stdout: {stdout}");
}

#[test]
fn run_vm_refuses_derive_cli() {
    let dir = scratch_dir();
    let src = dir.join("derive_vm.nv");
    std::fs::write(
        &src,
        libs() + SHAPE
            + "fn main():\n    let u = User { name: \"Ada\", age: 36 }\n    println(to_json_User(u))\n",
    )
    .unwrap();
    let out = run_cli_in(&dir, &["run-vm", src.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
}
