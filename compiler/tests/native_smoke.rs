//! Phase 3 Step-1 exit smoke: well-typed straight-line programs lower to
//! NIR and compile to a non-empty native object file.
//!
//! This asserts codegen *accepts* the category and emits bytes — linking
//! those bytes against `runtime-native` and executing them is the next
//! step (`noct build`), not this file.

use compiler::backends::cranelift::driver::compile_to_object;
use compiler::diagnostics::DiagnosticSink;
use compiler::{lexer, parser, resolver, typeck};

/// Lower `source` through the full frontend to NIR, compile to an object
/// file in a unique temp dir, assert the object is non-empty, and return
/// its path (left on disk for inspection on failure).
fn compile_source_to_object(case: &str, source: &str) -> std::path::PathBuf {
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(source, &mut sink);
    let program = parser::parse(&tokens, &mut sink);
    let program = resolver::resolve(program, &mut sink);
    let module = typeck::typecheck(program, &mut sink);
    assert!(
        !sink.has_errors(),
        "[{case}] fixture failed to typecheck — smoke test bug, not a backend bug"
    );
    let nir_module = compiler::nir::lowering::lower(module);

    let dir = std::env::temp_dir().join(format!(
        "noctivue-native-smoke-{}-{case}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let obj = dir.join(format!("{case}.o"));
    compile_to_object(&nir_module, &obj).expect("[{case}] compile_to_object failed");
    let meta = std::fs::metadata(&obj).expect("stat emitted object");
    assert!(
        meta.len() > 0,
        "[{case}] emitted object file is empty — codegen produced no bytes"
    );
    obj
}

#[test]
fn arithmetic_call_print_and_interp() {
    // Exercises: intra-module Call, Move aliases from `let`, string
    // literal rodata, ToString(Int) + concat from interpolation, Print,
    // and a Unit-returning main (no Cranelift return value).
    // NOTE: `print` takes String only; interpolating keeps the test on the
    // execution path instead of vacuously agreeing on a type error.
    compile_source_to_object(
        "arithmetic",
        "fn add(a: Int, b: Int) -> Int:\n    a + b\n\nmain():\n    let x = add(20, 22)\n    print(\"{x}\")\n",
    );
}

#[test]
fn int_division_and_int_return() {
    // Exercises: checked_sdiv (NOT a bare sdiv — see lower.rs's Div arm),
    // Sub, and a non-Unit main (I64 Cranelift return).
    compile_source_to_object(
        "div",
        "fn main() -> Int:\n    let q = 100 / 4\n    q - 25\n",
    );
}
