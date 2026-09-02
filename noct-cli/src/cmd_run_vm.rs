//! `noct run-vm` — execute a .nv program via the NIR bytecode VM (M1)

use compiler::{lexer, parser, resolver, typeck};
use compiler::diagnostics::DiagnosticSink;

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("usage: noct run-vm <file.nv>");
        return 1;
    }

    let path = &args[0];
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {}: {}", path, e);
            return 1;
        }
    };

    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(&source, &mut sink);
    let program = parser::parse(&tokens, &mut sink);
    let program = resolver::resolve(program, &mut sink);
    let module = typeck::typecheck(program, &mut sink);

    // Lower to NIR
    let nir_module = compiler::nir::lowering::lower(module);

    // Run via VM
    let mut vm = compiler::nir::vm::Vm::new(nir_module);
    match vm.run() {
        Ok(result) => {
            println!("VM result: {:?}", result);
            0
        }
        Err(e) => {
            eprintln!("VM error: {}", e);
            1
        }
    }
}