use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::lex;

fn main() {
    let source = std::fs::read_to_string("examples/dashboard.nv").unwrap();
    let mut sink = DiagnosticSink::new();
    let tokens = lex(&source, &mut sink);
    
    // Find the token at byte 1830
    let mut start_idx = 0;
    for (i, tok) in tokens.iter().enumerate() {
        if tok.span.start >= 1830 {
            start_idx = i;
            break;
        }
    }
    
    println!("Starting from token {} at byte {}", start_idx, tokens[start_idx].span.start);
    for (i, tok) in tokens.iter().skip(start_idx).take(100).enumerate() {
        println!("[{}] {:?} span={:?}", i + start_idx, tok.node, tok.span);
    }
}
