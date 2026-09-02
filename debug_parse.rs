use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::lex;
use compiler::parser::parse;

fn main() {
    let source = std::fs::read_to_string("examples/dashboard.nv").unwrap();
    let mut sink = DiagnosticSink::new();
    let tokens = lex(&source, &mut sink);
    let program = parse(&tokens, &mut sink);
    
    println!("Errors: {:?}", sink.diagnostics());
    println!("Items: {}", program.items.len());
    
    for (i, item) in program.items.iter().enumerate() {
        match item {
            compiler::ast::Item::BareDecl(bd) => {
                println!("[{}] BareDecl: {}", i, bd.name);
                println!("  Body stmts: {}", bd.body.stmts.len());
                for (j, stmt) in bd.body.stmts.iter().enumerate() {
                    match stmt {
                        compiler::ast::Stmt::State(s) => println!("    [{}] State: {}", j, s.name),
                        compiler::ast::Stmt::BareField(f) => println!("    [{}] BareField: {}", j, f.name),
                        compiler::ast::Stmt::Function(f) => println!("    [{}] Function: {}", j, f.name),
                        compiler::ast::Stmt::Expr(e) => println!("    [{}] Expr: {:?}", j, e),
                        other => println!("    [{}] {:?}", j, other),
                    }
                }
            }
            other => println!("[{}] {:?}", i, other),
        }
    }
}
