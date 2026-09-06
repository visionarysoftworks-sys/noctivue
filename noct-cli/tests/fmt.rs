//! CLI tests for `noct fmt` v1 (stdlib/PROPOSALS.md P-004).
//!
//! Strategy mirrors `differential.rs`: write inline sources to unique
//! temp files (best-effort cleanup, never repo fixtures), invoke the
//! built `noct` binary, assert on exit codes and exact file bytes.
//! Byte assertions use `\n` throughout (v1 canonicalizes endings).

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

/// Write `source` bytes verbatim to a unique temp file, return its path.
fn write_case(name: &str, source: &[u8]) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-fmt-{}-{id}", std::process::id()));
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

fn read_bytes(path: &PathBuf) -> Vec<u8> {
    std::fs::read(path).expect("read back formatted file")
}

/// Run `fmt` on `source`, expect success, return the resulting bytes.
fn fmt_ok(name: &str, source: &[u8]) -> Vec<u8> {
    let path = write_case(name, source);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "[{name}] fmt exit != 0. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = read_bytes(&path);
    cleanup(&path);
    bytes
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn fmt_clean_file_is_byte_identical() {
    let src = b"fn add(a: Int, b: Int) -> Int:\n    a + b\n";
    assert_eq!(fmt_ok("clean", src), src.to_vec());
}

#[test]
fn fmt_strips_trailing_whitespace_and_collapses_blanks() {
    let src = b"fn add(a: Int, b: Int) -> Int:   \n    a + b\n\n\n\nfn sub(a: Int, b: Int) -> Int:\n    a - b";
    let expected =
        b"fn add(a: Int, b: Int) -> Int:\n    a + b\n\nfn sub(a: Int, b: Int) -> Int:\n    a - b\n";
    assert_eq!(fmt_ok("trivia", src), expected.to_vec());
}

#[test]
fn fmt_adds_single_trailing_newline_and_drops_leading_blanks() {
    let src = b"\n\nmain():\n    println(\"hi\")";
    let expected = b"main():\n    println(\"hi\")\n";
    assert_eq!(fmt_ok("edges", src), expected.to_vec());
}

#[test]
fn fmt_preserves_triple_quoted_span_byte_identical() {
    // Trailing spaces INSIDE the span are values and must survive;
    // the opener line keeps its bytes too (conservative v1 call).
    let src = b"fn poem() -> String:\n    let s = \"\"\"line one   \nline two\n\"\"\"\n    s\n";
    assert_eq!(fmt_ok("triple", src), src.to_vec());
}

#[test]
fn fmt_ignores_quotes_in_comments() {
    // The `"""` here is comment text, not a span opener: the trailing
    // spaces on real code lines must still be stripped.
    let src = b"// a \"\"\" quoted comment   \nmain():\n    println(\"hi\")   \n";
    let expected = b"// a \"\"\" quoted comment\nmain():\n    println(\"hi\")\n";
    assert_eq!(fmt_ok("comment_quotes", src), expected.to_vec());
}

#[test]
fn fmt_normalizes_crlf_to_lf() {
    let src = b"main():\r\n    println(\"hi\")\r\n";
    let expected = b"main():\n    println(\"hi\")\n";
    assert_eq!(fmt_ok("crlf", src), expected.to_vec());
}

#[test]
fn fmt_check_reports_dirty_and_clean() {
    let dirty = write_case("check_dirty", b"main():   \n    println(\"hi\")\n");
    let clean = write_case("check_clean", b"main():\n    println(\"hi\")\n");
    let ds = dirty.to_string_lossy().to_string();
    let cs = clean.to_string_lossy().to_string();

    let out = run_cli(&["fmt", "--check", &ds, &cs]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "--check on dirty file must exit 1. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("would reformat"),
        "--check must name the dirty file, got:\n{stdout}"
    );
    // --check writes nothing.
    assert_eq!(
        read_bytes(&dirty),
        b"main():   \n    println(\"hi\")\n".to_vec()
    );

    let out = run_cli(&["fmt", "--check", &cs]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--check on clean file must exit 0. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    cleanup(&dirty);
    cleanup(&clean);
}

#[test]
fn fmt_is_idempotent_on_realistic_source() {
    // Exercises comments, interpolation, match, loops, blank runs.
    let src = b"//! Module docs.\n\n// A comment.\nUser:\n    id: Int\n    name: String\n\n\nfn label(u: User) -> String:\n    \"{u.id}: {u.name}\"   \n\nmain():\n    let u = User { id: 1, name: \"Al\" }\n    println(label(u))\n";
    let once = fmt_ok("idempotent", src);
    // Formatting the formatted output must be a byte-identical no-op
    // (the in-command guard asserts this too — this doubles it here).
    let path = write_case("idempotent2", &once);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", &s]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(read_bytes(&path), once);
    cleanup(&path);
}

#[test]
fn fmt_rejects_tab_indent_loudly_and_writes_nothing() {
    // D1 (P-004): tabs in leading whitespace are a loud error, never
    // a silent rewrite — the file must be left byte-identical.
    let src = b"main():\n\tprintln(\"hi\")\n";
    let path = write_case("tab_indent", src);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", &s]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "tab indent must exit 1. stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("tab"),
        "must cite tabs, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(read_bytes(&path), src.to_vec());
    cleanup(&path);
}
