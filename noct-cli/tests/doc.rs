//! CLI tests for `noct doc` (Phase 4, item 6).
//!
//! `doc` renders `///` doc comments plus hover-mirroring signatures
//! to stdout by default; `--output` persists the same bytes to a
//! file. Doc text comes from the shared `doc_comment_for` (one rule,
//! two consumers with hover). Harness mirrors `fmt.rs`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 10;

fn run_cli(args: &[&str]) -> Output {
    let child = Command::new(noct_bin())
        .args(args)
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

fn write_case(name: &str, source: &[u8]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-doc-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(format!("{name}.nv"));
    let mut f = std::fs::File::create(&path).expect("create temp file");
    f.write_all(source).expect("write temp file");
    path
}

fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

// ── Cases ───────────────────────────────────────────────────────────────────

const SRC: &[u8] = b"/// Adds two numbers.\nfn add(a: Int, b: Int) -> Int:\n    a + b\n";

#[test]
fn doc_renders_signature_and_docs_to_stdout_writing_nothing() {
    let path = write_case("doc", SRC);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["doc", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Function: `add`"), "got:\n{stdout}");
    assert!(
        stdout.contains("fn add(a: Int, b: Int) -> Int"),
        "signature must mirror hover, got:\n{stdout}"
    );
    assert!(stdout.contains("Adds two numbers."), "got:\n{stdout}");
    // Default writes no files beside the input.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1,
        "doc must not write files by default"
    );
    cleanup(&path);
}

#[test]
fn doc_output_flag_persists_stdout_bytes() {
    let path = write_case("doc_out", SRC);
    let s = path.to_string_lossy().to_string();
    let dest = path.with_extension("md");
    let d = dest.to_string_lossy().to_string();
    let out = run_cli(&["doc", &s, "--output", &d]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let file = std::fs::read_to_string(&dest).expect("read back docs");
    assert!(file.contains("## Function: `add`"), "got:\n{file}");
    assert!(file.contains("Adds two numbers."), "got:\n{file}");
    let _ = std::fs::remove_file(&dest);
    cleanup(&path);
}

#[test]
fn doc_requires_input() {
    let out = run_cli(&["doc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("input file required"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Hover-card sections (TOOLCHAIN.md §6.1) ──────────────────────────────────

const CARD_SRC: &[u8] = b"/// Adds two numbers.\nfn add(a: Int, b: Int) -> Int:\n    a + b\n\n/// A 2D point.\nstruct Point:\n    x: Float\n    y: Float\n\n/// A direction.\nenum Direction:\n    North\n    East(Int)\n\n/// The app name.\nconst APP_NAME: String = \"demo\"\n";

#[test]
fn doc_renders_signature_type_location_and_docs() {
    // Every item card carries the hover section order: signature,
    // Type (functions only), Declared-in, documentation.
    let path = write_case("doc_card", CARD_SRC);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["doc", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let file_label = path.file_name().unwrap().to_string_lossy().to_string();
    // Signatures mirror hover.
    for sig in [
        "fn add(a: Int, b: Int) -> Int",
        "struct Point:",
        "enum Direction:",
        "const APP_NAME: String",
    ] {
        assert!(stdout.contains(sig), "missing signature {sig:?}, got:\n{stdout}");
    }
    // Type line: functions only.
    assert!(
        stdout.contains("Type: (Int, Int) -> Int"),
        "function card must carry its Type line, got:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("Type:").count(),
        1,
        "only the function card carries a Type line, got:\n{stdout}"
    );
    // Declared-in: one per item (fn + struct + enum + const).
    assert_eq!(
        stdout.matches(&format!("Declared in {file_label}.")).count(),
        4,
        "every card must carry its location, got:\n{stdout}"
    );
    // Documentation text.
    for docs in [
        "Adds two numbers.",
        "A 2D point.",
        "A direction.",
        "The app name.",
    ] {
        assert!(stdout.contains(docs), "missing docs {docs:?}, got:\n{stdout}");
    }
    cleanup(&path);
}

#[test]
fn doc_renders_exported_items() {
    // `export` is visibility, not a separate item: exported entries
    // must render like their plain counterparts (regression: they
    // used to vanish because the resolver passes `Item::Export`
    // through and `doc` skipped it).
    let src = b"/// Public helper.\nexport fn pub_helper(x: Int) -> Int:\n    x\n\n/// Exported point.\nexport struct Exported:\n    field: Int\n";
    let path = write_case("doc_export", src);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["doc", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Function: `pub_helper`"), "got:\n{stdout}");
    assert!(stdout.contains("fn pub_helper(x: Int) -> Int"), "got:\n{stdout}");
    assert!(stdout.contains("Public helper."), "got:\n{stdout}");
    assert!(stdout.contains("## Struct: `Exported`"), "got:\n{stdout}");
    assert!(stdout.contains("Exported point."), "got:\n{stdout}");
    let file_label = path.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(
        stdout.matches(&format!("Declared in {file_label}.")).count(),
        2,
        "got:\n{stdout}"
    );
    cleanup(&path);
}
