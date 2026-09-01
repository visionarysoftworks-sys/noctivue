//! Noctivue compiler frontend.
//!
//! Pipeline: source → [`lexer`] → [`parser`] → [`ast`] → [`resolver`] → [`typeck`] → [`hir`]
//!
//! Structured diagnostics are emitted by every stage through [`diagnostics`] and are
//! available as machine-readable JSON via `noct diagnostics --json` (AI_TOOLING.md).

pub mod ast;
pub mod diagnostics;
pub mod hir;
pub mod lexer;
pub mod parser;
pub mod resolver;
pub mod typeck;
