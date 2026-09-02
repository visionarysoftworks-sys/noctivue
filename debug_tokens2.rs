use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::lex;

fn main() {
    let source = std::fs::read_to_string("examples/dashboard.nv").unwrap();
    let mut sink = DiagnosticSink::new();
    let tokens = lex(&source, &mut sink);
    
    for (i, tok) in tokens.iter().enumerate() {
        if tok.span.start >= 2920 && tok.span.end <= 2940 {
            println!("[{}] {:?} span={:?}", i, tok.node, tok.span);
        }
    }
}
