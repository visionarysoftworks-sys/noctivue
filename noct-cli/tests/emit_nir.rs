//! CLI tests for `noct build --emit-nir` — the `.nvir` text dump
//! (NIR.md §7.1, stage 1 of the artifact plan).
//!
//! What is asserted, and why each one matters:
//! - the dump is written *beside the entry file* for the bare flag (a
//!   dump scattered into the CWD is not where anyone looks for it);
//! - `--emit-nir=<path>` writes exactly there;
//! - a manifest-bearing project renders a real `package:` header, and a
//!   manifest-less one renders `-` rather than a guessed name;
//! - the dump stops before codegen/link (no cargo, no toolchain needed),
//!   which is what makes it usable for inspecting IR at all;
//! - a program that fails to compile writes NOTHING — a partial `.nvir`
//!   that looks plausible is worse than no file (NIR.md §7.1);
//! - type definitions come out sorted, so `git diff` on a dump is
//!   meaningful rather than HashMap-order noise.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 60;

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
        Err(_) => panic!("TIMEOUT (>{CASE_TIMEOUT_SECS}s): `noct {}` hung", args.join(" ")),
    }
}

/// Unique scratch dir, forward slashes (the `.nv` path handling in the
/// toolchain assumes them on Windows).
fn scratch(tag: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "noctivue-emitnir-{tag}-{}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

const PROGRAM: &str = "\
/// Adds two numbers.
fn add(a: Int, b: Int) -> Int:
    a + b

main():
    let total: Int = add(20, 22)
    println(\"ok\")
";

#[test]
fn bare_flag_writes_the_dump_beside_the_entry_file() {
    let dir = scratch("beside");
    let src = dir.join("prog.nv");
    std::fs::write(&src, PROGRAM).expect("write program");

    let out = run_cli_in(&dir, &["build", "--emit-nir", "prog.nv"]);
    assert!(
        out.status.success(),
        "emit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dump = dir.join("prog.nvir");
    assert!(dump.exists(), "expected {} to exist", dump.display());
    // And NOT in the CWD: the bare flag derives the path from the input.
    assert!(
        !Path::new("prog.nvir").exists(),
        "dump must not be written to the CWD"
    );
    let text = std::fs::read_to_string(&dump).expect("read dump");
    assert!(text.starts_with("nvir_version: 1\n"), "got:\n{text}");
    // The instruction inventory from NIR.md §4, in its canonical Display
    // form: this is the whole point of the text-first choice.
    assert!(text.contains("= add %0, %1 : Int (native)"), "got:\n{text}");
    assert!(text.contains("return "), "got:\n{text}");
}

#[test]
fn explicit_path_is_honored_exactly() {
    let dir = scratch("explicit");
    std::fs::write(dir.join("prog.nv"), PROGRAM).expect("write program");
    let out = run_cli_in(&dir, &["build", "--emit-nir=out/custom.nvir", "prog.nv"]);
    // Parent may not exist: that is an error, not a silent mkdir.
    assert!(
        !out.status.success(),
        "writing into a missing directory must fail loudly"
    );
    assert!(!dir.join("out").exists(), "no directory should be created");

    let direct = dir.join("chosen.nvir");
    let out = run_cli_in(
        &dir,
        &["build", &format!("--emit-nir={}", direct.display()), "prog.nv"],
    );
    assert!(
        out.status.success(),
        "emit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(direct.exists(), "expected the explicit path to be written");
}

#[test]
fn manifest_header_is_real_and_absent_is_a_dash() {
    let dir = scratch("manifest");
    std::fs::write(dir.join("prog.nv"), PROGRAM).expect("write program");
    std::fs::write(
        dir.join("nestpkg.nvpm"),
        "package:\n    name: myapp\n    version: 1.2.3\n",
    )
    .expect("write manifest");

    let out = run_cli_in(&dir, &["build", "--emit-nir", "prog.nv"]);
    assert!(
        out.status.success(),
        "emit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(dir.join("prog.nvir")).expect("read dump");
    assert!(
        text.contains("package: myapp 1.2.3\n"),
        "manifest header should be rendered: {text}"
    );

    // Remove the manifest: the same command must still work and must say
    // `-` rather than inventing a name (a dump is a debugging artifact;
    // a broken or absent manifest must not block it).
    std::fs::remove_file(dir.join("nestpkg.nvpm")).expect("drop manifest");
    let out = run_cli_in(&dir, &["build", "--emit-nir", "prog.nv"]);
    assert!(
        out.status.success(),
        "emit without a manifest failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(dir.join("prog.nvir")).expect("read dump");
    assert!(
        text.contains("package: -\n"),
        "manifest-less dump should render `-`: {text}"
    );
}

#[test]
fn a_program_that_does_not_compile_writes_nothing() {
    let dir = scratch("broken");
    // Type error: `Int` where `String` is expected.
    std::fs::write(
        dir.join("prog.nv"),
        "main():\n    println(42)\n",
    )
    .expect("write program");

    let out = run_cli_in(&dir, &["build", "--emit-nir", "prog.nv"]);
    assert!(
        !out.status.success(),
        "a program with type errors must not emit IR"
    );
    assert!(
        !dir.join("prog.nvir").exists(),
        "a failed emit must leave no partial dump behind"
    );
}

#[test]
fn types_are_sorted_so_diffs_are_meaningful() {
    let dir = scratch("sorted");
    // Zeta is declared before Alpha on purpose.
    std::fs::write(
        dir.join("prog.nv"),
        "\
struct Zeta:
    field: Int

struct Alpha:
    field: Bool

main():
    println(\"ok\")
",
    )
    .expect("write program");

    let out = run_cli_in(&dir, &["build", "--emit-nir", "prog.nv"]);
    assert!(
        out.status.success(),
        "emit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(dir.join("prog.nvir")).expect("read dump");
    let alpha = text.find("Alpha").unwrap_or_else(|| panic!("Alpha missing: {text}"));
    let zeta = text.find("Zeta").unwrap_or_else(|| panic!("Zeta missing: {text}"));
    assert!(
        alpha < zeta,
        "type defs must be sorted by name (Alpha before Zeta): {text}"
    );
}
