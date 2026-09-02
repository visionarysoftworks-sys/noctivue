use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::lex;

fn main() {
    let source = std::fs::read_to_string("examples/dashboard.nv").unwrap();
    let mut sink = DiagnosticSink::new();
    let tokens = lex(&source, &mut sink);
    
    let mut names = vec!["Int", "UInt", "Float", "Bool", "Char", "String", "Unit", "Option", "Result", "Map", "Set"];
    
    let mut in_indented = false;
    let mut brace_depth = 0u32;
    for i in 0..tokens.len() {
        match &tokens[i].node {
            compiler::lexer::Token::Indent => in_indented = true,
            compiler::lexer::Token::Dedent => in_indented = false,
            compiler::lexer::Token::LBrace => brace_depth += 1,
            compiler::lexer::Token::RBrace => brace_depth = brace_depth.saturating_sub(1),
            compiler::lexer::Token::Ident(ref name) => {
                if !in_indented && brace_depth == 0 {
                    if i + 1 < tokens.len() {
                        if matches!(tokens[i + 1].node, compiler::lexer::Token::Colon) {
                            names.push(name.as_str());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    
    println!("Known type names: {:?}", names);
}
