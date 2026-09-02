use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::lex;
use compiler::parser::parse;
use compiler::resolver::resolve;
use compiler::typeck;

fn main() {
    let source = std::fs::read_to_string("examples/dashboard.nv").unwrap();
    let mut sink = DiagnosticSink::new();
    let tokens = lex(&source, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);

    fn find_row_calls(expr: &compiler::ast::Expr) {
        match expr {
            compiler::ast::Expr::Call(call) => {
                if let compiler::ast::Expr::Ident(name, _) = call.callee.as_ref() {
                    if name == "row" {
                        println!("Found row call: {:?}", call);
                    }
                }
                for arg in &call.args {
                    find_row_calls(&arg.value);
                }
                if let Some(ref block) = call.trailing_block {
                    for stmt in &block.stmts {
                        find_row_calls_in_stmt(stmt);
                    }
                }
            }
            compiler::ast::Expr::Member(me) => {
                find_row_calls(&me.object);
            }
            compiler::ast::Expr::UnaryOp(uo) => {
                find_row_calls(&uo.operand);
            }
            compiler::ast::Expr::BinOp(bo) => {
                find_row_calls(&bo.left);
                find_row_calls(&bo.right);
            }
            _ => {}
        }
    }

    fn find_row_calls_in_stmt(stmt: &compiler::ast::Stmt) {
        match stmt {
            compiler::ast::Stmt::Expr(e) => find_row_calls(e),
            compiler::ast::Stmt::Let(l) => find_row_calls(&l.value),
            compiler::ast::Stmt::Assign(a) => {
                find_row_calls(&a.target);
                find_row_calls(&a.value);
            }
            compiler::ast::Stmt::If(i) => {
                find_row_calls(&i.condition);
                for stmt in &i.then_block.stmts {
                    find_row_calls_in_stmt(stmt);
                }
                for (cond, block) in &i.else_if_clauses {
                    find_row_calls(cond);
                    for stmt in &block.stmts {
                        find_row_calls_in_stmt(stmt);
                    }
                }
                if let Some(ref block) = i.else_block {
                    for stmt in &block.stmts {
                        find_row_calls_in_stmt(stmt);
                    }
                }
            }
            compiler::ast::Stmt::Match(m) => {
                find_row_calls(&m.scrutinee);
                for arm in &m.arms {
                    if let Some(guard) = &arm.guard {
                        find_row_calls(guard);
                    }
                    match &arm.body {
                        compiler::ast::MatchBody::Expr(e) => find_row_calls(e),
                        compiler::ast::MatchBody::Block(block) => {
                            for stmt in &block.stmts {
                                find_row_calls_in_stmt(stmt);
                            }
                        }
                    }
                }
            }
            compiler::ast::Stmt::For(f) => {
                find_row_calls(&f.iterable);
                for stmt in &f.body.stmts {
                    find_row_calls_in_stmt(stmt);
                }
            }
            compiler::ast::Stmt::While(w) => {
                find_row_calls(&w.condition);
                for stmt in &w.body.stmts {
                    find_row_calls_in_stmt(stmt);
                }
            }
            compiler::ast::Stmt::Loop(l) => {
                for stmt in &l.body.stmts {
                    find_row_calls_in_stmt(stmt);
                }
            }
            compiler::ast::Stmt::State(s) => {
                find_row_calls(&s.value);
            }
            compiler::ast::Stmt::BareField(_) => {}
            compiler::ast::Stmt::Function(_) => {}
            compiler::ast::Stmt::Struct(_) => {}
            compiler::ast::Stmt::Return(r) => {
                if let Some(ref v) = r.value {
                    find_row_calls(v);
                }
            }
            compiler::ast::Stmt::Break(b) => {
                if let Some(ref v) = b.value {
                    find_row_calls(v);
                }
            }
            compiler::ast::Stmt::Continue(_) => {}
            compiler::ast::Stmt::Var(_) => {}
        }
    }

    for item in &program.items {
        match item {
            compiler::ast::Item::Function(f) => {
                match &f.body {
                    compiler::ast::FunctionBody::Block(block) => {
                        for stmt in &block.stmts {
                            find_row_calls_in_stmt(stmt);
                        }
                    }
                    compiler::ast::FunctionBody::Expr(e) => {
                        find_row_calls(e);
                    }
                }
            }
            compiler::ast::Item::BareDecl(bd) => {
                for stmt in &bd.body.stmts {
                    find_row_calls_in_stmt(stmt);
                }
            }
            _ => {}
        }
    }

    let module = typeck::typecheck(program, &mut sink);

    println!("\nDiagnostics:");
    for d in sink.diagnostics() {
        println!("  {:?}", d);
    }
}
