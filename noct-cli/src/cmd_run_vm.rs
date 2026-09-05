//! `noct run-vm` — execute a .nv program via the NIR bytecode VM (M1)

use compiler::{lexer, parser, resolver, typeck};
use compiler::diagnostics::DiagnosticSink;

pub fn run(args: &[String]) -> i32 {
    // Multi-file like `noct run` (see cmd_run::read_sources): libraries
    // first, entry point last.
    let paths: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();
    if paths.is_empty() {
        eprintln!("usage: noct run-vm <file.nv>...");
        return 1;
    }

    let (source, files) = match crate::cmd_run::read_sources(&paths) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(&source, &mut sink);
    let program = parser::parse(&tokens, &mut sink);
    let program = resolver::resolve(program, &mut sink);
    let module = typeck::typecheck(program, &mut sink);

    // Check for errors before lowering/running — mirrors `noct run`
    // (cmd_run.rs). Without this, the VM path silently ran programs
    // with parse/resolve/typecheck errors instead of reporting them,
    // which is exactly the kind of interp-vs-VM behavioral divergence
    // Phase 2's exit criteria (IMPLEMENTATION_PLAN.md §4) requires
    // to be absent before Phase 3 can start.
    if sink.has_errors() {
        crate::cmd_run::print_diagnostics_multi(&files, &sink);
        return 1;
    }

    // Lower to NIR
    let nir_module = compiler::nir::lowering::lower(module);

    // Run via VM
    let mut vm = compiler::nir::vm::Vm::new(nir_module);
    match vm.run() {
        Ok(_) => {
            // Print any diagnostics emitted during lowering/execution,
            // matching cmd_run.rs's post-run diagnostic handling.
            if sink.has_errors() {
                crate::cmd_run::print_diagnostics_multi(&files, &sink);
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("VM error: {}", e);
            1
        }
    }
}