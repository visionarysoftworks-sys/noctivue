//! `noct test` — run the test suite for a Noctivue package.
//!
//! [Phase 1] Discovers `*.nv` files in `tests/fixtures/` and runs each through
//! the full pipeline (lex → parse → resolve → typecheck). Reports pass/fail.
//!
//! A fixture **passes** when:
//!   - It has no errors, OR
//!   - It lives in a path matching `*/ambiguous_decls/cycle_*` AND it produces
//!     at least one E0010 diagnostic (the expected classification-cycle error).
//!
//! Usage:
//!   noct test [filter]

use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let filter = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str());

    let fixture_dir = Path::new("tests/fixtures");
    if !fixture_dir.exists() {
        eprintln!("error: tests/fixtures/ not found (run from the workspace root)");
        return 1;
    }

    let fixtures = collect_fixtures(fixture_dir);

    if fixtures.is_empty() {
        eprintln!("warning: no .nv fixtures found in tests/fixtures/");
        return 0;
    }

    let mut pass = 0usize;
    let mut fail = 0usize;

    for path in &fixtures {
    let path_str = path.to_string_lossy();

        // Apply optional filter
        if let Some(f) = filter {
            if !path_str.contains(f) {
                continue;
            }
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

    println!("\n{} passed, {} failed", pass, fail);
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
