//! CLI tests for `noct lint` L-001 (stdlib/PROPOSALS.md P-002).
//!
//! L-001 (default-on): a non-final bare-`Ident` match arm naming a
//! visible enum variant is a binding that matches everything, not a
//! variant test. Warnings go to stderr; exit code stays 0 on
//! warnings. The two repo fixtures with genuine instances of the bug
//! pin the corpus audit (2 + 3 = the predicted 5 fires).

use std::io::Write;
use std::path::PathBuf;
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

const CASE_TIMEOUT_SECS: u64 = 10;

fn run_cli(args: &[&str]) -> Output {
    let child = Command::new(noct_bin())
        .args(args)
        .current_dir(workspace_root())
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
    let dir = std::env::temp_dir().join(format!("noctivue-lint-{}-{id}", std::process::id()));
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

fn lint_ok(path: &PathBuf) -> Output {
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["lint", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "lint must exit 0 on warnings. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

// ── Cases ───────────────────────────────────────────────────────────────────

const PROBE: &[u8] = b"enum Direction:\n    North\n    South\n\nfn direction_label(d: Direction) -> String:\n    match d:\n        North:\n            \"up\"\n        South:\n            \"down\"\n        _:\n            \"?\"\n";

#[test]
fn lint_l001_fires_on_nonfinal_variant_named_binding() {
    let path = write_case("probe", PROBE);
    let out = lint_ok(&path);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in ["North", "South"] {
        assert!(
            stderr.contains(&format!(
                "warning[L-001]: `{name}` here is a binding that matches everything, not a test for the `{name}` variant"
            )),
            "must fire on {name}, got:\n{stderr}"
        );
    }
    assert!(
        stderr.contains("arms below this one are unreachable"),
        "must carry the note, got:\n{stderr}"
    );
    assert!(
        stderr.contains("match the variant with `North():`"),
        "must carry the fix, got:\n{stderr}"
    );
    cleanup(&path);
}

#[test]
fn lint_l001_silent_on_correct_shapes() {
    // Variant() arms, trailing-None idiom, `_`, and ordinary
    // bindings are all silent — the rule fires only on the trap.
    let src = b"fn label_ok(d: Direction) -> String:\n    match d:\n        North():\n            \"up\"\n        _:\n            \"?\"\n\nfn get(o: Option<Int>) -> Int:\n    match o:\n        Some(v):\n            v\n        None:\n            0\n\nfn find(xs: List<Int>) -> Int:\n    match xs:\n        []:\n            0\n        x:\n            x\n";
    let path = write_case("silent", src);
    let with_enum = [
        b"enum Direction:\n    North\n    South\n\n".to_vec(),
        src.to_vec(),
    ]
    .concat();
    std::fs::write(&path, with_enum).expect("rewrite with enum");
    let out = lint_ok(&path);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "",
        "correct shapes must be silent, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&path);
}

#[test]
fn lint_l001_fires_on_known_fixture_bugs() {
    // Corpus audit (P-002): the only repo-wide fires are the genuine
    // bugs below — 2 + 3 = 5.
    let root = workspace_root();
    let basic = root.join("tests/fixtures/enums_match/basic.nv");
    let complex = root.join("tests/fixtures/m1_complex_test.nv");
    for (file, expected) in [
        (basic, vec!["North", "South"]),
        (complex, vec!["North", "South", "East"]),
    ] {
        let s = file.to_string_lossy().to_string();
        let out = run_cli(&["lint", &s]);
        assert_eq!(out.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&out.stderr);
        let fires = stderr.matches("warning[L-001]").count();
        assert_eq!(
            fires,
            expected.len(),
            "expected {} fires in {}, got:\n{stderr}",
            expected.len(),
            file.display()
        );
        for name in expected {
            assert!(
                stderr.contains(&format!("`{name}` here is a binding")),
                "missing {name} in:\n{stderr}"
            );
        }
    }
}

#[test]
fn lint_fix_applies_l001_autofix() {
    let path = write_case("fix", PROBE);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["lint", "--fix", &s]);
    assert_eq!(out.status.code(), Some(0), "got:\n{}", String::from_utf8_lossy(&out.stderr));
    let fixed = std::fs::read_to_string(&path).expect("re-read fixed file");
    // The bare `North:` and `South:` arms should now be `North():` and `South():`.
    assert!(fixed.contains("North():"), "expected North(): in:\n{fixed}");
    assert!(fixed.contains("South():"), "expected South(): in:\n{fixed}");
    cleanup(&path);
}

#[test]
fn lint_l002_fires_on_unused_import() {
    let src = b"import tank::engine\nimport tank::sprite as Sprite\n\nmain():\n    println(\"hi\")\n";
    let path = write_case("unused_import", src);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["lint", &s]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "lint must exit 0 on warnings. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("warning[L-002]: unused import `engine`"), "got:\n{stderr}");
    assert!(stderr.contains("warning[L-002]: unused import `Sprite`"), "got:\n{stderr}");
    cleanup(&path);
}

#[test]
fn lint_l002_silent_when_import_is_used() {
    // Expression use, type use, and struct-literal construction all count.
    let src = b"import tank::engine\nimport tank::Sprite\n\nfn draw(s: Sprite) -> Int:\n    engine(1)\n\nmain():\n    let s = Sprite { id: 1 }\n    println(draw(s))\n";
    let path = write_case("used_import", src);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["lint", &s]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "",
        "used imports must be silent, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&path);
}

#[test]
fn lint_json_reports_machine_readable_warnings() {
    let path = write_case("json", PROBE);
    let s = path.to_string_lossy().to_string();
    let out = run_cli(&["lint", "--json", &s]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with('[') && stdout.trim_end().ends_with(']'), "got:\n{stdout}");
    assert!(stdout.contains("\"rule\":\"L-001\""), "got:\n{stdout}");
    assert!(stdout.contains("\"line\":"), "got:\n{stdout}");
    assert!(stdout.contains("North"), "got:\n{stdout}");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "",
        "stderr must stay clean for piping, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&path);
}
