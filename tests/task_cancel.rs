//! Phase 6/M5 cooperative task-cancellation tests (ADR-024).
//!
//! `task_cancel_builtin` is the `.nv`-callable surface over the
//! per-worker cancel flag the M5 executor core already owned. What
//! these pin:
//!
//! 1. **Cancellation is real.** A cancelled task actually stops: the
//!    statements after its suspend point never run, and the `await`
//!    that joins it yields the pinned `Err(task {id} cancelled)`
//!    value (the same representation `await` already used for a
//!    failing task body, so `?` composes it unchanged).
//! 2. **The contract is the ADR's, not the code's:** idempotent
//!    cancel, cancel-after-completion is a no-op success, unknown
//!    handles are loud, an un-awaited cancellation is reported at
//!    drain, and a sibling task is unaffected.
//! 3. **Backend parity.** The lowering emits a real `Instr::TaskCancel`
//!    (not a silent fall-through to `Call`), and the VM refuses it
//!    loudly with the same refusal core the interpreter prints — the
//!    VM spawns no tasks, so every handle it can ever see is the
//!    never-spawned case. Native rejects the instruction through its
//!    normal `UnsupportedInstr` pre-check (covered by the CLI
//!    demonstration, not here: this package has no `noct` binary).
//!
//! The interpreter is the task-capable backend (CONCURRENCY.md §7);
//! `noct run-vm` and `noct build` refuse any program with a `task`
//! declaration outright, which is why the parity legs below run
//! handle-free programs.

use compiler::diagnostics::DiagnosticSink;
use compiler::nir::instr::Instr;
use std::path::PathBuf;
use std::time::Instant;

// ── Harness (mirrors tests/async_test.rs) ─────────────────────────────────────

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

fn typecheck_only(source: &str) -> Vec<String> {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let _module = compiler::typeck::typecheck(program, &mut sink);
    error_texts(&sink)
}

/// Full pipeline through typecheck. Panics on any error diagnostic.
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

/// Run a module through the interpreter. Returns `(exit, errors)`.
fn run(module: &compiler::hir::Module) -> (i32, Vec<String>) {
    let mut sink = DiagnosticSink::new();
    let mut interp = interp::Interpreter::new();
    let exit = interp.run(module, &mut sink);
    (exit, error_texts(&sink))
}

/// Run a module through the interpreter; require exit 0.
fn run_ok(module: &compiler::hir::Module, what: &str) {
    let (exit, errors) = run(module);
    assert_eq!(exit, 0, "[{what}] exit {exit}:\n  {}", errors.join("\n  "));
}

/// Run a module through the interpreter; require a loud failure whose
/// diagnostics include `needle`.
fn run_fails(module: &compiler::hir::Module, what: &str, needle: &str) {
    let (exit, errors) = run(module);
    assert_ne!(exit, 0, "[{what}] expected a loud failure, got exit 0");
    assert!(
        errors.iter().any(|e| e.contains(needle)),
        "[{what}] expected {needle:?} in:\n  {}",
        errors.join("\n  ")
    );
}

/// Run a module through the NIR VM. Returns the rendered result or the
/// error text.
fn run_vm(module: compiler::hir::Module) -> std::result::Result<String, String> {
    let nir = compiler::nir::lowering::LoweringContext::new(module).lower_module();
    let mut vm = compiler::nir::vm::Vm::new(nir);
    vm.run().map(|v| v.to_string()).map_err(|e| e.to_string())
}

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "noctivue-task-cancel-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// A Windows path as a Noctivue string literal (`\` is not an escape in
/// `.nv`, but the source is read verbatim — the doubled form is what
/// the existing task tests use, and it round-trips through the lexer).
fn path_literal(path: &PathBuf) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

// ── Cancellation is real and observable ──────────────────────────────────────

/// The headline claim: a cancelled task STOPS. The marker write after
/// the suspend point must never happen, and the `await` must hand the
/// awaiter the pinned cancelled value instead of the body value.
///
/// The wall-clock bound proves the sleep was cut SHORT, not merely
/// that the body was abandoned at the end. Starvation can only make
/// the run LONGER, never shorter, so a bound well under the full sleep
/// still holds on a loaded box — but it needs margin: the
/// deadline-based `sleep_builtin` overshoots when the OS deschedules
/// the worker, so the margin is ~1.7×, the same shape as
/// `tests/async_test.rs::tasks_overlap_in_wall_clock`'s. A failure
/// here means the cancel did not shorten the wait, which is worth 5 s.
#[test]
fn cancel_stops_the_task_at_its_suspend_point() {
    let marker = scratch("stops-marker");
    let out = scratch("stops-out");
    let module = check(
        &format!(
            r#"task slow():
    sleep_builtin(5000)
    fs_write_text("{}", "reached the end")

fn main():
    let h = slow()
    sleep_builtin(20)
    task_cancel_builtin(h)
    let v = await h
    fs_write_text("{}", "{{v}}")
"#,
            path_literal(&marker),
            path_literal(&out)
        ),
        "cancel_stops_the_task",
    );
    let start = Instant::now();
    run_ok(&module, "cancel_stops_the_task");
    let elapsed = start.elapsed();

    assert_eq!(read(&out), "Err(task 1 cancelled)");
    assert!(
        !marker.exists(),
        "the cancelled task ran past its suspend point (marker written)"
    );
    assert!(
        elapsed.as_millis() < 3_000,
        "the cancelled sleep was not cut short: {elapsed:?} for a 5000ms sleep"
    );
    let _ = std::fs::remove_file(&out);
}

/// A task that finishes on its own is unaffected by a later cancel —
/// cancel-after-completion is a no-op success (the `completed`
/// tombstone), never an error and never a rewritten value.
#[test]
fn cancel_after_completion_is_a_no_op() {
    let out = scratch("after-out");
    let module = check(
        &format!(
            r#"task quick():
    42

fn main():
    let h = quick()
    let v = await h
    task_cancel_builtin(h)
    fs_write_text("{}", "{{v}}")
"#,
            path_literal(&out)
        ),
        "cancel_after_completion",
    );
    run_ok(&module, "cancel_after_completion");
    assert_eq!(read(&out), "42");
    let _ = std::fs::remove_file(&out);
}

/// Cancelling twice is idempotent: the second call observes the same
/// live entry, sets the same flag, and returns.
#[test]
fn cancel_is_idempotent() {
    let out = scratch("idempotent-out");
    let module = check(
        &format!(
            r#"task slow():
    sleep_builtin(3000)

fn main():
    let h = slow()
    sleep_builtin(20)
    task_cancel_builtin(h)
    task_cancel_builtin(h)
    task_cancel_builtin(h)
    let v = await h
    fs_write_text("{}", "{{v}}")
"#,
            path_literal(&out)
        ),
        "cancel_is_idempotent",
    );
    run_ok(&module, "cancel_is_idempotent");
    assert_eq!(read(&out), "Err(task 1 cancelled)");
    let _ = std::fs::remove_file(&out);
}

/// An unknown handle is LOUD, not a silent no-op — the returned `Unit`
/// could be dropped, so the error has to be the runtime's panic
/// channel or it would vanish.
#[test]
fn cancel_of_an_unknown_handle_fails_loudly() {
    let module = check(
        "fn main():\n    task_cancel_builtin(999)\n",
        "cancel_unknown_handle",
    );
    run_fails(&module, "cancel_unknown_handle", "cancel of unknown task handle 999");
}

/// Sibling isolation: cancelling one task leaves the other's value
/// untouched. Handles are per-task values, not a shared switch.
#[test]
fn cancelling_one_task_leaves_its_sibling_alone() {
    let out = scratch("sibling-out");
    let module = check(
        &format!(
            r#"task victim():
    sleep_builtin(3000)
    1

task survivor():
    sleep_builtin(20)
    2

fn main():
    let a = victim()
    let b = survivor()
    task_cancel_builtin(a)
    let va = await a
    let vb = await b
    fs_write_text("{}", "{{va}}|{{vb}}")
"#,
            path_literal(&out)
        ),
        "sibling_isolation",
    );
    run_ok(&module, "sibling_isolation");
    assert_eq!(read(&out), "Err(task 1 cancelled)|2");
    let _ = std::fs::remove_file(&out);
}

/// Join-on-cancel, not detach-by-cancel: a cancellation nobody awaits
/// is still joined by the scope drain, and the drain REPORTS it
/// (E1002, exit 1) rather than letting a cancelled background task
/// pass as success.
#[test]
fn drain_reports_an_unawaited_cancellation() {
    let module = check(
        r#"task bg():
    sleep_builtin(3000)

fn main():
    let h = bg()
    sleep_builtin(20)
    task_cancel_builtin(h)
"#,
        "drain_reports_cancellation",
    );
    run_fails(&module, "drain_reports_cancellation", "background task 1 cancelled");
}

// ── Typeck ───────────────────────────────────────────────────────────────────

/// The signature is the contract: one `Int` handle in, `Unit` out.
/// Both misuse shapes are compile-time `E0200`, so a bad call never
/// reaches the runtime on either backend.
#[test]
fn typeck_pins_the_handle_in_unit_out_signature() {
    let errors = typecheck_only("fn main():\n    task_cancel_builtin(\"nope\")\n");
    assert!(
        errors.iter().any(|e| e.contains("E0200") && e.contains("expected `Int`")),
        "expected a compile-time type error, got {errors:?}"
    );

    let errors = typecheck_only("fn main():\n    task_cancel_builtin()\n");
    assert!(
        errors.iter().any(|e| e.contains("E0200") && e.contains("1 argument(s)")),
        "expected a compile-time arity error, got {errors:?}"
    );
}

// ── stdlib wrapper ───────────────────────────────────────────────────────────

/// The `.nv` level contract callers actually depend on: the wrapper is
/// what user code calls, and it composes with `await` the same way.
#[test]
fn nv_wrapper_task_cancel_stops_the_task() {
    let out = scratch("wrapper-out");
    let mut source = read(&PathBuf::from("stdlib/concurrency/task.nv"));
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(&format!(
        r#"task slow():
    sleep(3000)

fn main():
    let h = slow()
    sleep(20)
    task_cancel(h)
    let v = await h
    fs_write_text("{}", "{{v}}")
"#,
        path_literal(&out)
    ));
    let module = check(&source, "nv_wrapper");
    run_ok(&module, "nv_wrapper");
    assert_eq!(read(&out), "Err(task 1 cancelled)");
    let _ = std::fs::remove_file(&out);
}

// ── Backend parity ───────────────────────────────────────────────────────────

/// Lowering must emit a real `Instr::TaskCancel`. A silent fall-through
/// to `Instr::Call` (or to the interpreter's builtin dispatch) would
/// look identical in the VM and be wrong everywhere else, so the NIR
/// shape itself is pinned.
#[test]
fn lowering_emits_a_task_cancel_instruction() {
    let module = check(
        "fn main():\n    task_cancel_builtin(1)\n",
        "lowering shape",
    );
    let nir = compiler::nir::lowering::LoweringContext::new(module).lower_module();
    let found = nir.functions.iter().any(|f| {
        f.blocks.iter().any(|b| {
            b.instrs
                .iter()
                .any(|i| matches!(i, Instr::TaskCancel { .. }))
        })
    });
    assert!(
        found,
        "no Instr::TaskCancel in the lowered module — the builtin fell through to Call"
    );
}

/// The VM refuses the operation loudly, with the interpreter's refusal
/// text as the shared core. The interpreter is the task-capable
/// backend (CONCURRENCY.md §7) and the VM spawns no tasks, so every
/// handle it can receive is the never-spawned case — a silent success
/// here would report a cancellation that never happened.
#[test]
fn vm_refuses_cancel_loudly_with_the_shared_refusal_core() {
    let module = check(
        "fn main():\n    task_cancel_builtin(1)\n",
        "vm refusal",
    );
    let interp_errors = {
        let (exit, errors) = run(&module);
        assert_ne!(exit, 0, "interpreter must refuse an unspawned handle");
        errors
    };
    let shared = "cancel of unknown task handle 1";
    assert!(
        interp_errors.iter().any(|e| e.contains(shared)),
        "interpreter refusal must name the handle: {interp_errors:?}"
    );

    let module = check(
        "fn main():\n    task_cancel_builtin(1)\n",
        "vm refusal",
    );
    let vm = run_vm(module);
    assert!(vm.is_err(), "VM must refuse, got {vm:?}");
    let err = vm.unwrap_err();
    assert!(
        err.contains(shared),
        "VM refusal must carry the same core text, got: {err}"
    );
    assert!(
        err.contains("spawns no tasks"),
        "the VM refusal must say why it cannot own a handle, got: {err}"
    );
}

/// Both backends fail the same way for a well-typed program, so the
/// differential gate sees one behavior: same exit status, same
/// refusal core, differing only in the documented per-backend
/// runtime-failure prefix (`error: [E1002] panic: …` vs `VM error:
/// …`, the same split `noct-cli/tests/parity_hostio.rs` pins).
#[test]
fn both_backends_refuse_the_same_way() {
    let module = check(
        "fn main():\n    task_cancel_builtin(1)\n",
        "both refuse",
    );
    let (exit, errors) = run(&module);
    assert_eq!(exit, 1, "interpreter exit: {errors:?}");

    let module = check(
        "fn main():\n    task_cancel_builtin(1)\n",
        "both refuse",
    );
    let vm = run_vm(module);
    assert_eq!(vm.is_err(), true, "VM must fail where the interpreter fails");
    let vm_err = vm.unwrap_err();
    let shared = "cancel of unknown task handle 1";
    assert!(
        errors.iter().any(|e| e.contains(shared)),
        "interpreter: {errors:?}"
    );
    assert!(vm_err.contains(shared), "VM: {vm_err}");
}
