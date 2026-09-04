//! CLI-level differential tests: `noct run` (tree-walking interpreter)
//! vs `noct run-vm` (NIR bytecode VM).
//!
//! This is the Phase 2 exit-criteria harness
//! (IMPLEMENTATION_PLAN.md §4): the same program must produce
//! byte-identical stdout, the same exit code, and identical stderr on
//! both paths. Stdout equality covers statement ordering (post-branch
//! code must not run before the branch), short-circuit evaluation, and
//! value rendering; stderr equality covers compile-error reporting
//! (both CLIs share the diagnostic printer).
//!
//! Known boundary (documented, not asserted): *runtime*-failure stderr
//! text may differ (`VM error: ...` vs interpreter diagnostics) because
//! HIR carries no spans for the VM to cite. Exit codes and stdout must
//! still match. See TOOLCHAIN.md §6 / NIR.md §7.

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
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Seconds before a hung program fails the test instead of hanging CI.
/// Infinite loops are the characteristic VM-lowering failure mode, so the
/// harness must convert them into failures.
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
            "TIMEOUT (>{CASE_TIMEOUT_SECS}s): `noct {}` hung — likely an infinite loop in lowering/VM",
            args.join(" ")
        ),
    }
}

/// Write `source` to a unique temp file and return its path.
fn write_case(name: &str, source: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "noctivue-differential-{}-{}",
        std::process::id(),
        id
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(format!("{name}.nv"));
    let mut f = std::fs::File::create(&path).expect("create temp file");
    f.write_all(source.as_bytes()).expect("write temp file");
    path
}

fn check_paths(name: &str, path: &std::path::Path, owned: bool) {
    let path_str = path.to_string_lossy().to_string();
    let run = run_cli(&["run", &path_str]);
    let run_vm = run_cli(&["run-vm", &path_str]);

    assert_eq!(
        run.status.code(),
        run_vm.status.code(),
        "[{name}] exit code differs:\n  run:    {:?}\n  run-vm: {:?}\n  run stderr:\n{}\n  run-vm stderr:\n{}",
        run.status.code(),
        run_vm.status.code(),
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run_vm.stderr),
    );
    assert_eq!(
        run.stdout, run_vm.stdout,
        "[{name}] stdout differs:\n  run:\n{}\n  run-vm:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run_vm.stdout),
    );
    assert_eq!(
        run.stderr, run_vm.stderr,
        "[{name}] stderr differs:\n  run:\n{}\n  run-vm:\n{}",
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run_vm.stderr),
    );

    // Best-effort cleanup — ONLY for temp files this harness created.
    // Never delete repo fixtures (an earlier revision did, and ate two).
    if owned {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn check_source(name: &str, source: &str) {
    let path = write_case(name, source);
    check_paths(name, &path, true);
}

fn check_fixture(rel: &str) {
    let path = workspace_root().join(rel);
    assert!(path.exists(), "fixture missing: {}", path.display());
    check_paths(rel, &path, false);
}

// ── Pure semantics ────────────────────────────────────────────────────

#[test]
fn arithmetic_and_bindings() {
    // NOTE: `print` takes String only; interpolating keeps the test on the
    // execution path instead of vacuously agreeing on a type error.
    check_source(
        "arithmetic",
        "fn add(a: Int, b: Int) -> Int:\n    a + b\n\nmain():\n    let x = add(20, 22)\n    print(\"{x}\")\n",
    );
}

#[test]
fn branch_ordering() {
    // Post-branch code must run AFTER the branch on both paths.
    check_source(
        "branch_order",
        "main():\n    let x = 5\n    if x > 3:\n        print(\"big\")\n    else:\n        print(\"small\")\n    print(\"done\")\n",
    );
}

#[test]
fn while_loop() {
    check_source(
        "while_loop",
        "main():\n    let i = 0\n    while i < 3:\n        print(\"{i}\")\n        i = i + 1\n    print(\"done\")\n",
    );
}

#[test]
fn for_loop() {
    check_source(
        "for_loop",
        "main():\n    for n in [10, 20, 30]:\n        print(\"{n}\")\n",
    );
}

#[test]
fn match_statement() {
    check_source(
        "match_stmt",
        "main():\n    let x = 2\n    match x:\n        1: print(\"one\")\n        2: print(\"two\")\n        _: print(\"other\")\n    print(\"after\")\n",
    );
}

// ── ? and ?? observability ────────────────────────────────────────────

#[test]
fn coalesce_short_circuits() {
    // RHS must NOT evaluate when LHS is Some (no "rhs" on stdout).
    check_source(
        "coalesce_some",
        "fn eff() -> Int:\n    print(\"rhs\")\n    99\n\nmain():\n    let x: Option<Int> = Some(1)\n    print(\"{x ?? eff()}\")\n",
    );
}

#[test]
fn coalesce_none_evaluates_rhs() {
    check_source(
        "coalesce_none",
        "fn eff() -> Int:\n    print(\"rhs\")\n    99\n\nmain():\n    let x: Option<Int> = None\n    print(\"{x ?? eff()}\")\n",
    );
}

#[test]
fn logical_and_false_false() {
    // The specific corner where the old `icmp eq` catch-all diverged from
    // correct `&&` semantics: eq(false,false) = true, but false&&false = false.
    check_source(
        "and_false_false",
        "main():\n    let a = false\n    let b = false\n    print(\"{a && b}\")\n",
    );
}

#[test]
fn logical_or_false_false() {
    // Same corner for `||`: eq(false,false) = true, but false||false = false.
    check_source(
        "or_false_false",
        "main():\n    let a = false\n    let b = false\n    print(\"{a || b}\")\n",
    );
}

#[test]
fn logical_and_short_circuits() {
    // RHS must NOT evaluate when LHS is false (no "rhs" on stdout).
    check_source(
        "and_short_circuit",
        "fn eff() -> Bool:\n    print(\"rhs\")\n    true\n\nmain():\n    let a = false\n    print(\"{a && eff()}\")\n",
    );
}

#[test]
fn logical_or_short_circuits() {
    // RHS must NOT evaluate when LHS is true (no "rhs" on stdout).
    check_source(
        "or_short_circuit",
        "fn eff() -> Bool:\n    print(\"rhs\")\n    false\n\nmain():\n    let a = true\n    print(\"{a || eff()}\")\n",
    );
}

#[test]
fn try_propagates_and_prints() {
    check_source(
        "try_print",
        "find_user(id: Int) -> Option<Int>:\n    if id == 1:\n        Some(id)\n    else:\n        None\n\nget(id: Int) -> Option<Int>:\n    let x = find_user(id)?\n    Some(x + 1)\n\nmain():\n    print(\"{get(1)}\")\n    print(\"{get(99)}\")\n",
    );
}

#[test]
fn try_result_err() {
    check_source(
        "try_err",
        "fail() -> Result<Int, String>:\n    Err(\"boom\")\n\nget2() -> Result<Int, String>:\n    let x = fail()?\n    Ok(x + 1)\n\nmain():\n    print(\"{get2()}\")\n",
    );
}

// ── Aggregates, interpolation, const ──────────────────────────────────

#[test]
fn structs_and_interpolation() {
    check_source(
        "structs",
        "User:\n    name: String\n    age: Int\n\nmain():\n    let u = User { name: \"Bob\", age: 30 }\n    print(\"User: {u.name}, age {u.age}\")\n",
    );
}

#[test]
fn enums_and_match_expr() {
    check_source(
        "enums",
        "enum Dir:\n    North\n    South\n\ndescribe(d: Dir) -> String:\n    match d:\n        North: \"north\"\n        South: \"south\"\n\nmain():\n    print(describe(North))\n    print(describe(South))\n",
    );
}

#[test]
fn consts_and_lists() {
    check_source(
        "consts_lists",
        "const MAX: Int = 3\n\nmain():\n    let ns = [1, 2, 3, 4, 5]\n    print(\"{ns}\")\n    print(\"{MAX}\")\n",
    );
}

// ── Compile-error parity (shared diagnostic printer) ──────────────────

#[test]
fn compile_error_parity() {
    check_source(
        "type_error",
        "fn main():\n    let x: String = 42\n    print(x)\n",
    );
}

// ── Repo fixtures ─────────────────────────────────────────────────────

#[test]
fn fixture_m0_demo() {
    check_fixture("examples/m0_demo.nv");
}

/// enums_match fixture (payload enums, literals, guards, wildcards) driven
/// through a synthesized main — the fixture itself is a library file.
#[test]
fn fixture_enums_match() {
    let mut source =
        std::fs::read_to_string(workspace_root().join("tests/fixtures/enums_match/basic.nv"))
            .expect("read enums fixture");
    source.push_str(
        "\nmain():\n    print(\"{area(Circle(2.0))}\")\n    print(\"{area(Rectangle(3.0, 4.0))}\")\n    print(\"{direction_label(North)}\")\n    print(\"{direction_label(East)}\")\n    print(\"{classify_number(0)}\")\n    print(\"{classify_number(-5)}\")\n    print(\"{classify_number(7)}\")\n    print(\"{unwrap_or(Just(9), 0)}\")\n    print(\"{unwrap_or(Nothing, 0)}\")\n",
    );
    check_source("enums_match", &source);
}

/// result_option fixture (Option/Result/?/??) driven through main.
#[test]
fn fixture_result_option() {
    let mut source =
        std::fs::read_to_string(workspace_root().join("tests/fixtures/result_option/basic.nv"))
            .expect("read result_option fixture");
    source.push_str(
        "\nmain():\n    print(\"{greet_user(1)}\")\n    print(\"{greet_user(2)}\")\n    print(\"{double_parsed(\"x\")}\")\n    print(\"{process(\"x\")}\")\n    print(\"{name_or_default(1)}\")\n    print(\"{name_or_default(2)}\")\n",
    );
    check_source("result_option", &source);
}

#[test]
fn fixture_dashboard_nonui() {
    check_fixture("tests/fixtures/dashboard_nonui.nv");
}

#[test]
fn fixture_simple_vm_test() {
    check_fixture("tests/fixtures/simple_vm_test.nv");
}
