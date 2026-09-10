//! CLI tests for `noct fmt --v2` (Phase 4, Slice E / P-004 v2).
//!
//! v2 is the AST printer: expanded canonical form, comment
//! attachment, density heuristic, with blocking gates
//! (AST-equivalence, idempotency, no-new-errors). Tests: an exact
//! kitchen-sink golden, a corpus sweep (every repo `.nv` file must
//! pass all three gates), refusal modes, `--check`, and the
//! semicolon/density rules. Harness mirrors `fmt.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

const CASE_TIMEOUT_SECS: u64 = 15;

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
    let dir = std::env::temp_dir().join(format!("noctivue-fmt2-{}-{id}", std::process::id()));
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

fn fmt_v2_ok(name: &str, source: &[u8]) -> Vec<u8> {
    let path = write_case(name, source);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", "--v2", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "[{name}] fmt --v2 exit != 0. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&path).expect("read back");
    cleanup(&path);
    bytes
}

// ── Cases ───────────────────────────────────────────────────────────────────

const GOLDEN_IN: &[u8] = b"//! kitchen sink for v2.\nimport tank::engine\nimport tank::Sprite as Sprite\n\n/// User record.\nstruct User:\n    id: Int\n    name: String\n\n/// Kinds.\nenum Shape:\n    Circle(Float)\n    Rectangle(Float, Float)\n    Nothing()\n\n/// Showable things.\ntrait Show:\n    fn show() -> String\n\nimpl Show for User:\n    fn show() -> String:\n        \"user\"\n\nconst MAX_USERS: Int = 1000\n\n/// Add with default step.\nfn add(a: Int, b: Int = 1) -> Int:\n    a + b\n\nfn classify(n: Int) -> String:\n    match n:\n        0: \"zero\"\n        x if x < 0: \"negative\"\n        _: \"positive\"\n\nmain():\n    // seed the RNG-ish\n    let total = 0\n    var count = 0\n    let u = User { id: 7, name: \"Al\" }\n    let greeting = \"hi {u.name}, you are #{u.id}\"\n    let r = 1..10\n    let v = compute(u.id + 1) ?? 0\n    let w = try_parse(\"42\")?\n    let neg = -total\n    let flag = !false && true || false\n    let mixed = 1 + 2 * 3 - 4 / 2\n    let grouped = (1 + 2) * 3\n    let items = [1, 2, 3]\n    let f = |x| x * 2\n    let ch = 'z'\n    let pi = 3.14159\n    state running = true\n    total = add(total, count)\n    count += 1\n    if total > 10:\n        println(\"big\")\n    else if total > 5:\n        println(\"mid\")\n    else:\n        println(\"small\")\n    while count < 3:\n        count += 1\n    for i in r:\n        println(i)\n    loop:\n        break\n    for ever in 0..=1:\n        continue\n    engine(\"go\", speed: 3); text(\"done\")\n    return\n";

const GOLDEN_OUT: &[u8] = b"//! kitchen sink for v2.\n\nimport tank::engine\nimport tank::Sprite as Sprite\n\n/// User record.\nstruct User:\n    id: Int\n    name: String\n\n/// Kinds.\nenum Shape:\n    Circle(Float)\n    Rectangle(Float, Float)\n    Nothing()\n\n/// Showable things.\ntrait Show:\n    fn show() -> String\n\nimpl Show for User:\n    fn show() -> String:\n        \"user\"\n\nconst MAX_USERS: Int = 1000\n\n/// Add with default step.\nfn add(a: Int, b: Int = 1) -> Int:\n    a + b\n\nfn classify(n: Int) -> String:\n    match n:\n        0: \"zero\"\n        x if x < 0: \"negative\"\n        _: \"positive\"\n\nmain():\n    // seed the RNG-ish\n    let total = 0\n    var count = 0; let u = User { id: 7, name: \"Al\" }; let greeting = \"hi {u.name}, you are #{u.id}\"\n    let r = 1..10; let v = compute(u.id + 1) ?? 0; let w = try_parse(\"42\")?; let neg = -total\n    let flag = !false && true || false; let mixed = 1 + 2 * 3 - 4 / 2; let grouped = (1 + 2) * 3\n    let items = [1, 2, 3]; let f = |x| x * 2; let ch = 'z'; let pi = 3.14159; state running = true\n    total = add(total, count); count += 1\n    if total > 10:\n        println(\"big\")\n    else if total > 5:\n        println(\"mid\")\n    else:\n        println(\"small\")\n    while count < 3:\n        count += 1\n    for i in r:\n        println(i)\n    loop:\n        break\n    for ever in 0..=1:\n        continue\n    engine(\"go\", speed: 3); text(\"done\")\n    return\n";

#[test]
fn fmt_v2_kitchen_sink_golden() {
    assert_eq!(fmt_v2_ok("golden", GOLDEN_IN), GOLDEN_OUT.to_vec());
}

#[test]
fn fmt_v2_corpus_passes_all_gates() {
    // Every repo .nv file: v2 must exit 0 (all three gates hold), OR
    // refuse loud with a parse-error message when the input itself
    // does not parse (v2 reasons about structure; v1 covers the
    // rest). Refusals for any other reason fail the test.
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in ["tests/fixtures", "stdlib", "examples"] {
        collect_nv(&root.join(dir), &mut files);
    }
    assert!(!files.is_empty(), "corpus walk found nothing");
    let mut failures = Vec::new();
    for path in &files {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("noctivue-fmt2c-{}-{id}", std::process::id()));
        std::fs::copy(path, &tmp).expect("copy corpus file");
        let s = tmp.to_string_lossy().to_string();
        let out = run_cli(&["fmt", "--v2", &s]);
        let short = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        match out.status.code() {
            Some(0) => {}
            Some(1) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !stderr.contains("parse error") {
                    failures.push(format!("{short}: unexpected refusal: {}", stderr.lines().next().unwrap_or("").trim()));
                }
            }
            other => failures.push(format!("{short}: exit {other:?}")),
        }
        let _ = std::fs::remove_file(&tmp);
    }
    assert!(failures.is_empty(), "v2 gate failures:\n{}", failures.join("\n"));
}

fn collect_nv(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_nv(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("nv") {
            out.push(path);
        }
    }
}

#[test]
fn fmt_v2_refuses_tabs_and_inline_block_comments() {
    // D1: tabs are loud, never laundered.
    let path = write_case("tab", b"main():\n\tprintln(\"hi\")\n");
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", "--v2", &s]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("tab"));
    assert_eq!(std::fs::read(&path).unwrap(), b"main():\n\tprintln(\"hi\")\n".to_vec());
    cleanup(&path);

    // Inline block comments inside code lines: refused (only own-line
    // and trailing comments are placeable).
    let path = write_case("inline_block", b"main():\n    println(/* note */ \"hi\")\n");
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", "--v2", &s]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("inline block comment"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&path);
}

#[test]
fn fmt_v2_check_and_semicolons() {
    // `;`-joined statements expand to one per line (D3) unless the
    // density heuristic rejoins them (tiny, uncommented, fits).
    let out = fmt_v2_ok("semis", b"main():\n    let a = 1; let b = 2\n    println(a + b)\n");
    assert_eq!(
        out,
        b"main():\n    let a = 1; let b = 2; println(a + b)\n".to_vec()
    );
    // A trailing comment binds to its own node (consumed as its
    // suffix before any later sibling looks), so it never orphans —
    // and it does NOT pin neighboring statements: tiny uncommented
    // neighbors still join under the density heuristic.
    let out = fmt_v2_ok(
        "semis_comment",
        b"main():\n    let a = 1; // first\n    let b = 2\n    println(a + b)\n",
    );
    assert_eq!(
        out,
        b"main():\n    let a = 1  // first\n    let b = 2; println(a + b)\n".to_vec()
    );

    // --check names dirty files without writing.
    let path = write_case("check", b"main():   \n    println(\"hi\")\n");
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["fmt", "--v2", "--check", &s]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("would reformat"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"main():   \n    println(\"hi\")\n".to_vec());
    cleanup(&path);
}
