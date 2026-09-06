//! `noct test` — run the test suite for a Noctivue package.
//!
//! Two modes, decided by the working directory:
//!
//! - **Workspace mode** (`tests/fixtures/` exists): the Phase-1
//!   fixture runner — lex → parse → resolve → typecheck per file,
//!   pass = no errors (or the expected E0010 for cycle fixtures).
//! - **Project mode** (`nestpkg.nvpm` exists, no fixtures dir): the
//!   Phase-4 package loop — every `tests/**/*.nv` file is executed
//!   through the full `run` pipeline and passes iff it exits 0
//!   (an `assert` failure or pipeline error is a FAIL naming the
//!   file). This is what `noct create`'s `tests/main_test.nv`
//!   smoke is written for.
//!
//! `// @skip-test` opts a file out in both modes (concat-gate
//! smokes whose callees resolve only in the concatenated run).
//!
//! Usage:
//!   noct test [filter]

use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let filter = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str());

    let fixture_dir = Path::new("tests/fixtures");
    if fixture_dir.exists() {
        return run_workspace_mode(filter);
    }
    if Path::new("nestpkg.nvpm").exists() {
        // Same package gate as build/run (P-003 §6).
        if let Err(message) = crate::registry::require_package_current(Path::new(".")) {
            eprintln!("noct test: {message}");
            return 1;
        }
        return run_project_mode(filter);
    }
    eprintln!("error: tests/fixtures/ not found (run from the workspace root)");
    eprintln!("       nor nestpkg.nvpm (run from a project created by `noct create`)");
    1
}

/// Project mode: execute every `tests/**/*.nv` through the `run`
/// pipeline; pass = exit 0. Reuses `cmd_run::run` verbatim (same
/// semantics as `noct run file`, including its diagnostics) so test
/// execution can never drift from real execution.
fn run_project_mode(filter: Option<&str>) -> i32 {
    let mut tests = Vec::new();
    collect_fixtures_rec(Path::new("tests"), &mut tests);
    tests.sort();
    if tests.is_empty() {
        eprintln!("warning: no .nv tests found in tests/");
        return 0;
    }

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut skip = 0usize;
    for path in &tests {
        let path_str = path.to_string_lossy().to_string();
        if let Some(f) = filter {
            if !path_str.contains(f) {
                continue;
            }
        }
        if is_skip_test(path) {
            println!("SKIP  {path_str}");
            skip += 1;
            continue;
        }
        let arg = path_str.clone();
        let code = crate::cmd_run::run(std::slice::from_ref(&arg));
        if code == 0 {
            println!("PASS  {path_str}");
            pass += 1;
        } else {
            println!("FAIL  {path_str} (exit {code})");
            fail += 1;
        }
    }

    println!("\n{pass} passed, {fail} failed, {skip} skipped");
    if fail > 0 { 1 } else { 0 }
}

fn run_workspace_mode(filter: Option<&str>) -> i32 {
    let fixture_dir = Path::new("tests/fixtures");
    let fixtures = collect_fixtures(fixture_dir);

    if fixtures.is_empty() {
        eprintln!("warning: no .nv fixtures found in tests/fixtures/");
        return 0;
    }

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut skip = 0usize;

    for path in &fixtures {
        let path_str = path.to_string_lossy();

        // Apply optional filter
        if let Some(f) = filter {
            if !path_str.contains(f) {
                continue;
            }
        }

        // Concat-gate smokes whose callees live in other files cannot
        // pass standalone (their identifiers resolve only in the
        // concatenated run); they opt out explicitly, never silently.
        if is_skip_test(path) {
            println!("SKIP  {path_str}");
            skip += 1;
            continue;
        }

        let result = run_fixture(path);
        match result {
            TestResult::Pass => {
                println!("PASS  {path_str}");
                pass += 1;
            }
            TestResult::Fail(reason) => {
                println!("FAIL  {path_str}");
                println!("      {reason}");
                fail += 1;
            }
        }
    }

    println!("\n{pass} passed, {fail} failed, {skip} skipped");
    if fail > 0 { 1 } else { 0 }
}

// ── Fixture runner ────────────────────────────────────────────────────────────

enum TestResult {
    Pass,
    Fail(String),
}

fn run_fixture(path: &Path) -> TestResult {
    let _path_str = path.to_string_lossy();

    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return TestResult::Fail(format!("cannot read file: {e}")),
    };

    // Check if this fixture should skip type checking (resolver-only test)
    let skip_typecheck = source.lines().any(|line| line.trim().starts_with("// @skip-typecheck"));

    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(&source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);

    if !skip_typecheck {
        let _module = compiler::typeck::typecheck(program, &mut sink);
    }

    let is_cycle_fixture = is_expected_cycle_fixture(path);

    if is_cycle_fixture {
        // Must produce at least one E0010
        let has_e0010 = sink.diagnostics().iter().any(|d| {
            d.code.as_deref() == Some("E0010")
        });
        if has_e0010 {
            TestResult::Pass
        } else {
            TestResult::Fail(format!(
                "expected E0010 (classification cycle) but got: {:?}",
                sink.diagnostics()
                    .iter()
                    .map(|d| d.code.as_deref().unwrap_or("(no code)"))
                    .collect::<Vec<_>>()
            ))
        }
    } else if sink.has_errors() {
        let msgs: Vec<String> = sink
            .diagnostics()
            .iter()
            .filter(|d| d.is_error())
            .map(|d| {
                if let Some(code) = &d.code {
                    format!("{code}: {}", d.message)
                } else {
                    d.message.clone()
                }
            })
            .collect();
        TestResult::Fail(msgs.join("; "))
    } else {
        TestResult::Pass
    }
}

/// Returns true if the fixture is expected to produce an E0010 cycle error.
fn is_expected_cycle_fixture(path: &Path) -> bool {
    // Matches paths like: .../ambiguous_decls/cycle_*.nv
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let in_ambiguous_decls = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        == "ambiguous_decls";
    in_ambiguous_decls && file_name.starts_with("cycle_")
}

/// Returns true if the fixture opts out of standalone `noct test`
/// via a `// @skip-test` marker line (concat-gate smokes whose
/// callees resolve only in the concatenated run).
fn is_skip_test(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(source) => source
            .lines()
            .any(|line| line.trim_start().starts_with("// @skip-test")),
        Err(_) => false,
    }
}

// ── Fixture discovery ─────────────────────────────────────────────────────────

fn collect_fixtures(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    collect_fixtures_rec(dir, &mut result);
    result.sort();
    result
}

fn collect_fixtures_rec(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures_rec(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("nv") {
            out.push(path);
        }
    }
}
