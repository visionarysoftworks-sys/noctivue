use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::lex;
use compiler::parser::parse;
use compiler::resolver::resolve;
use compiler::typeck::typecheck;

fn main() {
    let source = std::fs::read_to_string("examples/dashboard.nv").unwrap();
    let mut sink = DiagnosticSink::new();
    let tokens = lex(&source, &mut sink);
    let program = parse(&tokens, &mut sink);
    
    println!("Program items:");
    for (i, item) in program.items.iter().enumerate() {
        match item {
            compiler::ast::Item::Function(f) => println!("  [{}] Function: {}", i, f.name),
            compiler::ast::Item::BareDecl(bd) => println!("  [{}] BareDecl: {}", i, bd.name),
            compiler::ast::Item::Struct(s) => println!("  [{}] Struct: {}", i, s.name),
            compiler::ast::Item::Const(c) => println!("  [{}] Const: {}", i, c.name),
            other => println!("  [{}] {:?}", i, other),
        }
    }
    
    let program = resolve(program, &mut sink);
    let module = typecheck(program, &mut sink);
    
    println!("\nDiagnostics:");
    for d in sink.diagnostics() {
        println!("  {:?}", d);
    }
    
    println!("\nFunctions:");
    for f in &module.functions {
        println!("  {} ({} params)", f.name, f.params.len());
    }
}
