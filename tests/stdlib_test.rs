//! Stdlib foundation tests — Task: Phase 4 stdlib track.
//!
//! Covers `stdlib/` per `stdlib/SPEC.md` §7 (gates 1–2):
//!   1. Every foundation file lexes/parses/resolves/typechecks with zero
//!      error diagnostics.
//!   2. Every reserved file parses with zero items (header comments only)
//!      and zero error diagnostics — locks the `reserved` state.
//!   3. Both smoke mains run concatenated with the foundation (SPEC.md
//!      §5.1 order) and exit 0. The smokes are self-verifying: every
//!      expectation is an `assert_*`, so exit 0 proves all values.
//!
//! Golden transcripts (`tests/golden/stdlib/*.stdout`) are compared via
//! the CLI (`noct run <foundation files> <smoke>`) per SPEC.md §7 — the
//! interpreter writes directly to process stdout, so byte comparison
//! lives outside this in-process harness. The differential gate
//! (`run` vs `run-vm`) is recorded blocked in SPEC.md §10: the VM
//! rejects `to_int`/`to_float` builtins, fn-refs-as-values, and
//! multi-file units independent of stdlib code.

use compiler::diagnostics::DiagnosticSink;

// ── File lists (SPEC.md §5.1 order + §9 reserved set) ────────────────────────

/// Runnable foundation modules, in canonical concatenation order.
const FOUNDATION: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/math/constants.nv",
    "stdlib/math/basic.nv",
    "stdlib/collections/list.nv",
    "stdlib/collections/iterator.nv",
    "stdlib/time/duration.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
];

/// Reserved (header-only) files. Each must parse to zero items.
const RESERVED: &[&str] = &[
    "stdlib/core/prelude.nv",
    "stdlib/core/types.nv",
    "stdlib/collections/map.nv",
    "stdlib/collections/set.nv",
    "stdlib/collections/stack.nv",
    "stdlib/collections/queue.nv",
    "stdlib/math/statistics.nv",
    "stdlib/math/trigonometry.nv",
    "stdlib/strings/builder.nv",
    "stdlib/strings/unicode.nv",
    "stdlib/testing/test.nv",
    "stdlib/testing/property.nv",
    "stdlib/testing/mock.nv",
];

/// (Smoke main, display name) pairs.
const SMOKES: &[(&str, &str)] = &[
    ("tests/fixtures/stdlib/smoke_core.nv", "smoke_core"),
    (
        "tests/fixtures/stdlib/smoke_lists_strings.nv",
        "smoke_lists_strings",
    ),
];

// ── Helpers ─────────────────────────────────────────────────────────────────

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

/// Join several sources like `noct run` does (libraries first, entry last).
fn join(sources: &[String]) -> String {
    let mut out = String::new();
    for s in sources {
        out.push_str(s);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn error_texts(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let code = d.code.as_deref().unwrap_or("?");
            format!("[{code}] {}", d.message)
        })
        .collect()
}

/// Full pipeline through typecheck. Returns the HIR module; panics on error.
fn check(source: &str, what: &str) -> compiler::hir::Module {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let module = compiler::typeck::typecheck(program, &mut sink);
    let errors = error_texts(&sink);
    assert!(
        errors.is_empty(),
        "{what} produced error diagnostics:\n  {}",
        errors.join("\n  ")
    );
    module
}

// ── Gate 1: foundation files are error-free ─────────────────────────────────

#[test]
fn stdlib_foundation_zero_errors() {
    for path in FOUNDATION {
        let source = read(path);
        let mut sink = DiagnosticSink::new();
        let tokens = compiler::lexer::lex(&source, &mut sink);
        assert!(
            !tokens.is_empty(),
            "{path} lexed to zero tokens (reserved files belong in RESERVED)"
        );
        let program = compiler::parser::parse(&tokens, &mut sink);
        assert!(
            !program.items.is_empty(),
            "{path} parsed to zero items (reserved files belong in RESERVED)"
        );
        let program = compiler::resolver::resolve(program, &mut sink);
        let _ = compiler::typeck::typecheck(program, &mut sink);
        let errors = error_texts(&sink);
        assert!(
            errors.is_empty(),
            "{path} produced error diagnostics:\n  {}",
            errors.join("\n  ")
        );
    }
}

// ── Gate 2: reserved files stay header-only ─────────────────────────────────

#[test]
fn stdlib_reserved_zero_items_zero_errors() {
    for path in RESERVED {
        let source = read(path);
        let mut sink = DiagnosticSink::new();
        let tokens = compiler::lexer::lex(&source, &mut sink);
        let program = compiler::parser::parse(&tokens, &mut sink);
        assert!(
            program.items.is_empty(),
            "{path} must stay header-only (zero items); found {} item(s) — \
             promote it to a runnable module with a SPEC.md amendment instead",
            program.items.len()
        );
        let errors = error_texts(&sink);
        assert!(
            errors.is_empty(),
            "{path} produced error diagnostics:\n  {}",
            errors.join("\n  ")
        );
    }
}

// ── Gate 3: smoke mains exit 0 over the concatenated foundation ─────────────

#[test]
fn stdlib_smoke_exit_zero() {
    let libs: Vec<String> = FOUNDATION.iter().map(|p| read(p)).collect();
    for (path, name) in SMOKES {
        let mut sources = libs.clone();
        sources.push(read(path));
        let module = check(&join(&sources), name);
        let mut sink = DiagnosticSink::new();
        let mut interp = interp::Interpreter::new();
        let exit = interp.run(&module, &mut sink);
        let errors = error_texts(&sink);
        assert!(
            exit == 0,
            "[{name}] interpreter exit code {exit} (expected 0):\n  {}",
            errors.join("\n  ")
        );
    }
}
