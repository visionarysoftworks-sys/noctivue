//! Parser — token stream → unresolved AST.
//!
//! Responsibilities (COMPILER_ARCHITECTURE.md §3):
//! - Implement the formal EBNF grammar from SYNTAX.md §10.
//! - Produce [`crate::ast`] nodes.
//! - **Zero semantic reasoning**: the parser MUST NOT guess whether a bare
//!   `Identifier:` block is a struct, function, or component based on
//!   identifier casing or naming convention. All ambiguous declarations are
//!   emitted as [`crate::ast::BareDecl`] for the resolver to classify
//!   (COMPILER_ARCHITECTURE.md §4, DECISIONS.md Issue 1/2).
//!
//! The parser is a hand-written recursive-descent parser. No PEG or parser-
//! combinator library is used, per COMPILER_ARCHITECTURE.md §4.

pub mod grammar;

#[cfg(test)]
mod tests;

use crate::ast::Program;
use crate::diagnostics::DiagnosticSink;
use crate::lexer::Spanned;
use crate::lexer::Token;
use std::collections::HashSet;

/// Parse a token stream into an unresolved [`Program`] AST.
///
/// Errors are emitted into `sink`. The returned `Program` may be partially
/// constructed if errors were encountered (best-effort error recovery).
pub fn parse(tokens: &[Spanned<Token>], sink: &mut DiagnosticSink) -> Program {
    let known_types = collect_top_level_type_names(tokens);
    let mut p = grammar::Parser::new(tokens, sink, known_types);
    p.parse_program()
}

/// Collect identifiers that can serve as type names:
/// - Built-in primitive / stdlib types
/// - Top-level bare-declaration names (structs, components, functions used as types)
fn collect_top_level_type_names(tokens: &[Spanned<Token>]) -> HashSet<String> {
    let mut names: HashSet<String> = [
        "Int", "UInt", "Float", "Bool", "Char", "String", "Unit",
        "Option", "Result", "Map", "Set",
    ].iter().map(|s| s.to_string()).collect();

    let mut in_indented = false;
    let mut brace_depth = 0u32;
    for i in 0..tokens.len() {
        match &tokens[i].node {
            Token::Indent => in_indented = true,
            Token::Dedent => in_indented = false,
            Token::LBrace => brace_depth += 1,
            Token::RBrace => brace_depth = brace_depth.saturating_sub(1),
            Token::Ident(ref name) => {
                if !in_indented && brace_depth == 0 {
                    if i + 1 < tokens.len() {
                        if matches!(&tokens[i + 1].node, Token::Colon) {
                            names.insert(name.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    names
}
