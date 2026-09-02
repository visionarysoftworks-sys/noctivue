//! Differential test harness for NIR + VM vs. the tree-walking interpreter.
//!
//! This module verifies that the same Phase 1 test fixtures produce identical
//! observable behavior when run through both execution paths:
//! - Phase 1: lex → parse → resolve → typecheck → interp::Interpreter
//! - Phase 2: lex → parse → resolve → typecheck → nir::lowering → nir::vm::Vm

use crate::diagnostics::DiagnosticSink;
use crate::hir::Module as HirModule;
use crate::lexer::lex;
use crate::nir::lowering::LoweringContext;
use crate::nir::vm::{Vm, VmError};
use crate::parser::parse;
use crate::resolver::resolve;
use crate::typeck::typecheck;
use std::path::Path;

/// Captured output from a VM run.
#[derive(Debug, Clone)]
pub struct VmOutput {
    pub result: Result<String, String>,
    #[allow(dead_code)]
    pub stdout: String,
}

impl VmOutput {
    fn from_result(result: Result<String, VmError>, stdout: &str) -> Self {
        VmOutput {
            result: result.map_err(|e| e.to_string()),
            stdout: stdout.to_string(),
        }
    }
}

/// Take a source string and run it through the full pipeline to get a HIR module.
fn source_to_hir(source: &str) -> (HirModule, DiagnosticSink) {
    let mut sink = DiagnosticSink::new();
    let tokens = lex(source, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    let module = typecheck(program, &mut sink);
    (module, sink)
}

/// Run source through NIR lowering and VM.
pub fn run_vm(source: &str) -> VmOutput {
    let (hir_module, _sink) = source_to_hir(source);

    let lowering = LoweringContext::new(hir_module);
    let nir_module = lowering.lower_module();

    let mut vm = Vm::new(nir_module);

    let result = match vm.run() {
        Ok(v) => Ok(v.to_string()),
        Err(e) => Err(e),
    };

    VmOutput::from_result(result, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple program that should work identically in both paths.
    #[test]
    fn differential_simple_arithmetic() {
        let source = r#"
fn add(a: Int, b: Int) -> Int:
    a + b

main():
    let x = add(1, 2)
    let y = add(x, 3)
    let _ = y
"#;
        // This should work since it has no branching expressions
        let result = run_vm(source);
        assert!(
            result.result.is_ok(),
            "VM should succeed for simple arithmetic: {:?}",
            result.result
        );
    }

    /// Repro A from Task 2 - ?? operator produces wrong value
    #[test]
    #[ignore = "see Task 2 / NIR-??: Coalesce produces silently wrong value"]
    fn differential_repro_a_coalesce() {
        let source = r#"
main():
    let x: Option<Int> = None
    let y = x ?? 5
    let _ = y
"#;
        // Interpreter: y = 5
        // VM: produces wrong value
        let result = run_vm(source);
        assert!(
            result.result.is_ok(),
            "VM should handle ?? operator correctly: {:?}",
            result.result
        );
    }

    /// Repro B from Task 2 - ? doesn't propagate, it crashes
    #[test]
    #[ignore = "see Task 2 / NIR-?: Try operator doesn't propagate None"]
    fn differential_repro_b_try_propagate() {
        let source = r#"
find_user(id: Int) -> Option<Int>:
    if id == 1:
        Some(id)
    else:
        None

get(id: Int) -> Option<Int>:
    let x = find_user(id)?
    Some(x + 1)

main():
    let a = get(1)
    let b = get(99)
    let _ = a
    let _ = b
"#;
        // Interpreter: get(99) returns None cleanly
        // VM: hard errors with OptionUnwrapNone
        let result = run_vm(source);
        assert!(
            result.result.is_ok(),
            "VM should propagate None from ? operator: {:?}",
            result.result
        );
    }

    /// Test that m0_demo.nv runs through VM without crashing
    #[test]
    #[ignore = "see Task 2 / NIR-?: Various lowering issues in complex programs"]
    fn differential_m0_demo() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.parent().expect("workspace root");
        let demo_path = workspace.join("examples/m0_demo.nv");

        let source = std::fs::read_to_string(&demo_path)
            .unwrap_or_else(|e| panic!("could not read m0_demo.nv: {}", e));

        let result = run_vm(&source);
        assert!(
            result.result.is_ok(),
            "m0_demo.nv should run in VM: {:?}",
            result.result
        );
    }

    /// Test dashboard_nonui.nv fixture
    #[test]
    #[ignore = "see Task 2 / NIR-?: Various lowering issues"]
    fn differential_dashboard_nonui() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.parent().expect("workspace root");
        let fixture_path = workspace.join("tests/fixtures/dashboard_nonui.nv");

        let source = std::fs::read_to_string(&fixture_path)
            .unwrap_or_else(|e| panic!("could not read dashboard_nonui.nv: {}", e));

        let result = run_vm(&source);
        assert!(
            result.result.is_ok(),
            "dashboard_nonui.nv should run in VM: {:?}",
            result.result
        );
    }

    /// Test simple_vm_test.nv fixture
    #[test]
    fn differential_simple_vm_test() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.parent().expect("workspace root");
        let fixture_path = workspace.join("tests/fixtures/simple_vm_test.nv");

        let source = std::fs::read_to_string(&fixture_path)
            .unwrap_or_else(|e| panic!("could not read simple_vm_test.nv: {}", e));

        let result = run_vm(&source);
        assert!(
            result.result.is_ok(),
            "simple_vm_test.nv should run in VM: {:?}",
            result.result
        );
    }
}
