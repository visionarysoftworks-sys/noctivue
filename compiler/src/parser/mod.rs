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

/// Parse a token stream into an unresolved [`Program`] AST.
///
/// Errors are emitted into `sink`. The returned `Program` may be partially
/// constructed if errors were encountered (best-effort error recovery).
pub fn parse(tokens: &[Spanned<Token>], sink: &mut DiagnosticSink) -> Program {
    let mut p = grammar::Parser::new(tokens, sink);
    p.parse_program()
}
