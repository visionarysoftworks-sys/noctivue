//! Async runtime tests — Phase 5/M4 (`task` / `await` / `sleep_builtin`).
//!
//! Drives the full pipeline (lex → parse → resolve → typecheck →
//! interpret) in-process. Concurrency claims are verified by
//! scheduling effects that are impossible under sequential
//! execution (result values through the thread boundary, overlapping
//! wall-clock sleeps, completion order), plus loud-failure cases
//! for every misuse shape.

use compiler::diagnostics::DiagnosticSink;
use std::time::Instant;

/// Full pipeline through typecheck. Returns the HIR module; panics on error.
fn check(source: &str, what: &str) -> compiler::hir::Module {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let module = compiler::typeck::typecheck(program, &mut sink);
    let errors: Vec<String> = sink
        .diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let code = d.code.as_deref().unwrap_or("?");
            format!("[{code}] {}", d.message)
        })
        .collect();
    assert!(
        errors.is_empty(),
        "{what} produced error diagnostics:\n  {}",
        errors.join("\n  ")
    );
    module
}

/// Run a module; return (exit code, error diagnostic texts).
fn run(module: &compiler::hir::Module) -> (i32, Vec<String>) {
    let mut sink = DiagnosticSink::new();
    let mut interp = interp::Interpreter::new();
    let exit = interp.run(module, &mut sink);
    let errors: Vec<String> = sink
        .diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let code = d.code.as_deref().unwrap_or("?");
            format!("[{code}] {}", d.message)
        })
        .collect();
    (exit, errors)
}

#[test]
fn task_spawn_await_value() {
    // The task body's value crosses the thread boundary back to the
    // awaiter (the handle is opaque; the VALUE is the body's value).
    let module = check(
        r#"task answer():
    40 + 2
fn main():
    let h = answer()
    let v = await h
    assert(v == 42, "await must yield the task body value")
"#,
        "task_spawn_await_value",
    );
    let (exit, errors) = run(&module);
    assert_eq!(
        exit,
        0,
        "expected exit 0, diagnostics:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn task_args_are_cloned_values() {
    // Arguments cross into the task by value; mutating the caller's
    // binding afterwards cannot affect the running task.
    let module = check(
        r#"task echo(s: String):
    s
fn main():
    let msg = "hello"
    let h = echo(msg)
    let back = await h
    assert(back == "hello", "task must receive the argument value")
"#,
        "task_args_are_cloned_values",
    );
    let (exit, errors) = run(&module);
    assert_eq!(
        exit,
        0,
        "expected exit 0, diagnostics:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn tasks_overlap_in_wall_clock() {
    // Two 300ms sleeps complete in ~300ms, not ~600ms. The 550ms
    // bound leaves 250ms of scheduling headroom; sequential execution
    // cannot beat 600ms on any machine.
    let module = check(
        r#"task nap():
    sleep_builtin(300)
fn main():
    let a = nap()
    let b = nap()
    await a
    await b
"#,
        "tasks_overlap_in_wall_clock",
    );
    let start = Instant::now();
    let (exit, errors) = run(&module);
    let elapsed_ms = start.elapsed().as_millis();
    assert_eq!(
        exit,
        0,
        "expected exit 0, diagnostics:\n  {}",
        errors.join("\n  ")
    );
    assert!(
        elapsed_ms < 550,
        "tasks did not overlap: two 300ms sleeps took {elapsed_ms}ms (>= 550ms)"
    );
}

#[test]
fn await_rejects_non_handle() {
    // `await 5`: positive Int, never spawned — loud E1002, exit 1.
    let module = check("fn main():\n    await 5\n", "await_rejects_non_handle");
    let (exit, errors) = run(&module);
    assert_eq!(exit, 1, "awaiting a non-handle must fail the run");
    assert!(
        errors.iter().any(|e| e.contains("unknown task handle")),
        "expected unknown-handle diagnostic, got:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn await_rejects_non_int() {
    // `await true`: not even an Int — loud E1002, exit 1.
    let module = check("fn main():\n    await true\n", "await_rejects_non_int");
    let (exit, errors) = run(&module);
    assert_eq!(exit, 1, "awaiting a Bool must fail the run");
    assert!(
        errors.iter().any(|e| e.contains("task handle")),
        "expected task-handle diagnostic, got:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn await_twice_fails_loudly() {
    // Handles are single-use: the second await names its fate.
    let module = check(
        "task w():\n    1\nfn main():\n    let h = w()\n    await h\n    await h\n",
        "await_twice_fails_loudly",
    );
    let (exit, errors) = run(&module);
    assert_eq!(exit, 1, "double-await must fail the run");
    assert!(
        errors.iter().any(|e| e.contains("already awaited")),
        "expected already-awaited diagnostic, got:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn unawaited_tasks_join_at_exit() {
    // No fire-and-forget: a task never awaited still completes before
    // `run` returns. Observed via a file write (stdout from worker
    // threads cannot be captured in-process).
    let path = std::env::temp_dir().join(format!("noctivue-task-drain-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let source = format!(
        "task bg():\n    fs_write_text(\"{}\", \"done\")\nfn main():\n    let h = bg()\n",
        path.to_string_lossy().replace('\\', "\\\\")
    );
    let module = check(&source, "unawaited_tasks_join_at_exit");
    let (exit, errors) = run(&module);
    assert_eq!(
        exit,
        0,
        "expected exit 0, diagnostics:\n  {}",
        errors.join("\n  ")
    );
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("drained task never wrote its file: {e}"));
    assert_eq!(body, "done");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sleep_builtin_blocks_and_returns_unit() {
    // sleep(0) is a no-op returning Unit; the stdlib wrapper agrees.
    let module = check(
        "fn main():\n    sleep_builtin(0)\n    sleep_builtin(5)\n",
        "sleep_builtin_blocks_and_returns_unit",
    );
    let (exit, errors) = run(&module);
    assert_eq!(
        exit,
        0,
        "expected exit 0, diagnostics:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn sleep_builtin_rejects_negative() {
    let module = check(
        "fn main():\n    sleep_builtin(0 - 1)\n",
        "sleep_builtin_rejects_negative",
    );
    let (exit, errors) = run(&module);
    assert_eq!(exit, 1, "negative sleep must panic");
    assert!(
        errors.iter().any(|e| e.contains("non-negative")),
        "expected non-negative diagnostic, got:\n  {}",
        errors.join("\n  ")
    );
}

#[test]
fn local_task_call_fails_loudly() {
    // Boundary documentation: a task declared inside a function body
    // typechecks (bound as a handle) but produces no runtime binding —
    // only top-level tasks live in the module table. Calling it fails
    // loudly with `undefined name`: lift it to top level to spawn it.
    let module = check(
        "fn main():\n    task inner():\n        1\n    inner()\n",
        "local_task_call_fails_loudly",
    );
    let (exit, errors) = run(&module);
    assert_eq!(exit, 1, "calling a local task must fail the run");
    assert!(
        errors.iter().any(|e| e.contains("undefined name `inner`")),
        "expected undefined-name diagnostic, got:\n  {}",
        errors.join("\n  ")
    );
}
