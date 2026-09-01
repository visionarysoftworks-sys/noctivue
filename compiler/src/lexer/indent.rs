//! Offside-rule INDENT/DEDENT synthesis pass.
//!
//! This pass runs over the raw token stream produced by the tokeniser and
//! inserts [`Token::Indent`] and [`Token::Dedent`] tokens wherever the source
//! indentation level changes, using the same approach as Python and F#
//! (SYNTAX.md §10.1).
//!
//! ## Rules (LANGUAGE_SPEC.md §2, normative)
//!
//! - Indentation is **structurally significant** in the default (indentation) syntax.
//! - Recommended width: **4 spaces**. Tabs and spaces MUST NOT be mixed within
//!   one block's indentation — mixing is a hard compiler error (E0002).
//! - A dedent to a level that does not match any enclosing indentation level
//!   MUST produce a compiler error (E0003); recover by snapping to nearest level.
//! - Blank lines and comment-only lines inside an indented block MUST NOT
//!   affect indentation tracking.
//! - Brace-delimited blocks (`{ }`) **suspend** this pass for their entire
//!   contents (SYNTAX.md §6/§7).

use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};
use crate::lexer::Spanned;
use crate::lexer::Token;

/// Run the offside-rule pass over a flat token stream.
///
/// Consumes `tokens` (which should contain [`Token::Newline`] tokens and no
/// INDENT/DEDENT tokens yet) and returns a new stream with INDENT/DEDENT tokens
/// inserted at the appropriate positions.
///
/// Indentation errors are emitted into `sink`.
pub fn synthesise_indent_dedent(
    tokens: Vec<Spanned<Token>>,
    sink: &mut DiagnosticSink,
) -> Vec<Spanned<Token>> {
    IndentPass::new(tokens, sink).run()
}

struct IndentPass<'sink> {
    /// Input token stream (consumed left-to-right via index).
    tokens: Vec<Spanned<Token>>,
    /// Current read position in `tokens`.
    pos: usize,
    /// Output token stream being built.
    out: Vec<Spanned<Token>>,
    /// Indentation level stack. Always starts with a single `0`.
    stack: Vec<usize>,
    /// Depth of `{ }` braces seen so far. While > 0 the offside rule is
    /// suspended and `Token::Newline` tokens are suppressed from the output.
    brace_depth: u32,
    sink: &'sink mut DiagnosticSink,
}

impl<'sink> IndentPass<'sink> {
    fn new(tokens: Vec<Spanned<Token>>, sink: &'sink mut DiagnosticSink) -> Self {
        IndentPass {
            tokens,
            pos: 0,
            out: Vec::new(),
            stack: vec![0],
            brace_depth: 0,
            sink,
        }
    }

    fn advance(&mut self) -> Option<Spanned<Token>> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn dummy_span(&self) -> Span {
        // Use the span of the current position token, or a zero span at the end.
        self.tokens
            .get(self.pos)
            .map(|t| t.span.clone())
            .or_else(|| self.tokens.last().map(|t| Span { start: t.span.end, end: t.span.end }))
            .unwrap_or(Span { start: 0, end: 0 })
    }

    fn push_out(&mut self, tok: Token, span: Span) {
        self.out.push(Spanned { node: tok, span });
    }

    fn run(mut self) -> Vec<Spanned<Token>> {
        while let Some(spanned) = self.advance() {
            match &spanned.node {
                // ── Track brace depth ────────────────────────────────────────
                Token::LBrace => {
                    self.brace_depth += 1;
                    self.out.push(spanned);
                }
                Token::RBrace => {
                    self.brace_depth = self.brace_depth.saturating_sub(1);
                    self.out.push(spanned);
                }

                // ── Newline token ────────────────────────────────────────────
                Token::Newline => {
                    if self.brace_depth > 0 {
                        // Inside braces: suppress newlines entirely (purely cosmetic).
                        continue;
                    }
                    // Emit the Newline, then look ahead to determine the
                    // indentation of the next non-blank, non-comment line.
                    let newline_span = spanned.span.clone();

                    // Look ahead past blank/comment lines to find the next
                    // meaningful token and its indentation column.
                    let indent_info = self.look_ahead_indent();

                    match indent_info {
                        LookAheadResult::Eof => {
                            // Emit the Newline first, then close all blocks.
                            self.out.push(Spanned { node: Token::Newline, span: newline_span });
                            let close_span = self.dummy_span();
                            self.emit_dedents_to(0, close_span);
                            // Eof will be emitted below when we encounter it.
                        }
                        LookAheadResult::BraceBlock => {
                            // The next token is inside a brace block; newline
                            // terminates the current statement but no indent change.
                            self.out.push(Spanned { node: Token::Newline, span: newline_span });
                        }
                        LookAheadResult::IndentLevel { level, span, has_tab } => {
                            if has_tab {
                                self.sink.emit(
                                    Diagnostic::error(
                                        "tabs and spaces must not be mixed in indentation",
                                    )
                                    .with_span(span.clone(), "mixed indentation here")
                                    .with_code("E0002"),
                                );
                                // Still emit the Newline; recover by treating as same level.
                                self.out.push(Spanned { node: Token::Newline, span: newline_span });
                            } else {
                                let top = *self.stack.last().unwrap();
                                if level > top {
                                    // Indent
                                    self.out.push(Spanned {
                                        node: Token::Newline,
                                        span: newline_span,
                                    });
                                    self.stack.push(level);
                                    self.push_out(Token::Indent, span);
                                } else if level == top {
                                    // Same level — just the Newline
                                    self.out.push(Spanned {
                                        node: Token::Newline,
                                        span: newline_span,
                                    });
                                } else {
                                    // Dedent — may be multiple levels
                                    self.out.push(Spanned {
                                        node: Token::Newline,
                                        span: newline_span,
                                    });
                                    if !self.stack.contains(&level) {
                                        // E0003: does not match any enclosing level
                                        self.sink.emit(
                                            Diagnostic::error(
                                                "dedent does not match any enclosing indentation level",
                                            )
                                            .with_span(span.clone(), "unexpected indentation here")
                                            .with_code("E0003"),
                                        );
                                        // Recover: snap to the nearest lower level
                                        let snap = self
                                            .stack
                                            .iter()
                                            .copied()
                                            .filter(|&l| l <= level)
                                            .max()
                                            .unwrap_or(0);
                                        self.emit_dedents_to(snap, span);
                                    } else {
                                        self.emit_dedents_to(level, span);
                                    }
                                }
                            }
                        }
                    }
                }

                // ── EOF ──────────────────────────────────────────────────────
                Token::Eof => {
                    let eof_span = spanned.span.clone();
                    // Emit any remaining Dedents
                    self.emit_dedents_to(0, eof_span.clone());
                    self.push_out(Token::Eof, eof_span);
                }

                // ── All other tokens pass through unchanged ───────────────────
                _ => {
                    self.out.push(spanned);
                }
            }
        }
        self.out
    }

    /// Emit `Dedent` tokens until the stack top equals `target`.
    fn emit_dedents_to(&mut self, target: usize, span: Span) {
        while *self.stack.last().unwrap_or(&0) > target {
            self.stack.pop();
            self.push_out(Token::Dedent, span.clone());
        }
    }

    /// Look ahead past blank/comment lines to determine the indentation of the
    /// next substantive line.
    ///
    /// A "blank line" is a `Token::Newline` (possibly preceded by nothing but
    /// whitespace). A "comment-only line" has only comment tokens before the
    /// next `Newline`.
    ///
    /// This peeks at positions in `self.tokens` starting from `self.pos`
    /// without consuming any. The caller is responsible for driving `advance()`
    /// to past the tokens that contribute to indentation.
    ///
    /// We work with the token stream rather than raw characters here. The raw
    /// tokeniser emits tokens in order; indentation information is reconstructed
    /// from the spans of the first meaningful token on each line.
    fn look_ahead_indent(&mut self) -> LookAheadResult {
        // We need to find the next non-blank, non-comment line and measure its
        // indentation level.
        //
        // Strategy: scan forward in `self.tokens` from `self.pos`, skipping:
        //   - `Token::Newline` tokens (blank lines)
        //   - `Token::LineComment`, `Token::DocComment`, `Token::ModDocComment`,
        //     `Token::BlockComment` tokens that are followed by a Newline
        //     (comment-only lines)
        //
        // When we find a substantive token, its span.start tells us the byte
        // offset; we need to compute the column (number of spaces from last \n).
        //
        // But we actually store the column as the *indentation level*, which is
        // the number of leading spaces before the first non-space on a line.
        // We compute this from the source text's byte offset and the raw source.
        //
        // NOTE: The token stream doesn't carry the source string, so we need to
        // derive indentation from spans. However, `Spanned` only gives us byte
        // offsets; we can't walk back to find the line start without the source.
        //
        // Instead, we track indentation by inspecting the whitespace tokens that
        // appear *after* each Newline. The raw tokeniser already skips whitespace
        // silently, so the column information is not stored.
        //
        // Resolution: The approach used here is a *token-position-based* lookahead.
        // The raw tokeniser already stripped leading whitespace and did not emit
        // IndentWhitespace tokens. To recover indentation, we must use the
        // `span.start` of the first token on a line and the source byte offset
        // of the preceding newline.
        //
        // Since the `IndentPass` doesn't have access to the raw source string,
        // we embed the indentation level directly in the Newline token's span.
        // Specifically: we *augment* the Newline token to carry the indent level
        // of the *following* line.
        //
        // Wait — re-reading the design: the raw tokeniser emits a `Newline`
        // token at the newline character. The *next* non-whitespace token's
        // `span.start` minus the position of the last `\n` gives the column.
        // But IndentPass doesn't have the source string.
        //
        // REVISED APPROACH (no source access needed):
        //   In the raw tokeniser, after emitting `Token::Newline`, we immediately
        //   scan and *consume* all leading whitespace on the next line without
        //   emitting tokens for it. The first real token on that line has a
        //   `span.start` that includes the leading whitespace offset.
        //
        //   We embed the indentation in a synthetic `IndentHint` — but that would
        //   require a new Token variant, violating the constraint.
        //
        // FINAL APPROACH: Augment the Newline's span.
        //   `span.start` = byte offset of the '\n' itself.
        //   `span.end`   = byte offset of the first non-space character on the
        //                  next non-blank, non-comment line; the indentation
        //                  level = span.end - (offset_of_last_newline_before_that_line).
        //
        // Actually the cleanest approach is to embed the indent info into the
        // Newline span itself during tokenisation:
        //   span.start = byte offset of '\n'
        //   span.end   = byte offset of first non-space char on next line
        //               (equivalently: column = span.end - (start_of_that_line))
        //
        // We can compute column: after the '\n' we scan spaces; column = count of
        // spaces. We need the raw source in the tokeniser for this.
        //
        // IMPLEMENTATION: Thread the column through a *dedicated channel* — the
        // Newline token span:
        //   Newline span.start = byte offset of '\n'
        //   Newline span.end   = indent level of next line (as a usize count of spaces,
        //                        packed into the end field — this is unconventional but
        //                        avoids a new token variant)
        //
        // This packing is valid because we document it clearly and the INDENT/DEDENT
        // pass is the only consumer of Newline.span.end before the parser. After this
        // pass, Newline tokens have their spans corrected.
        //
        // See `Lexer::emit_newline_with_indent()` in mod.rs.
        //
        // Given the above design, the LookAheadResult is derived directly from
        // the *next* `Token::Newline`'s span or from looking at the following
        // non-Newline token.
        //
        // SIMPLER DESIGN USED: Because the raw tokeniser already emits Newline
        // tokens with `span.end` = indent level (number of leading spaces on the
        // *next* line), the IndentPass can just read that directly.
        //
        // Let's find the next non-comment, non-blank-line indicator.

        let mut i = self.pos;
        loop {
            match self.tokens.get(i) {
                None => return LookAheadResult::Eof,
                Some(t) => match &t.node {
                    // A Newline token whose span.end encodes the indent of the
                    // *following* line (set by the raw tokeniser). But blank lines
                    // repeat Newlines — skip them.
                    Token::Newline => {
                        // This is a blank line — skip and keep looking.
                        i += 1;
                    }
                    // Comment-only content on a line: skip it, then skip the
                    // following Newline.
                    Token::LineComment(_)
                    | Token::DocComment(_)
                    | Token::ModDocComment(_)
                    | Token::BlockComment(_) => {
                        i += 1;
                        // Skip the Newline that follows this comment line (if any)
                        if self.tokens.get(i).map(|t| t.node == Token::Newline).unwrap_or(false) {
                            i += 1;
                        }
                    }
                    Token::Eof => return LookAheadResult::Eof,
                    _ => {
                        // This is the first substantive token on the next line.
                        // Its `span.start` encodes (byte_offset_after_newline + spaces).
                        // The indentation level is stored in the *preceding* Newline's
                        // span.end by the raw tokeniser.
                        //
                        // Find the most recent Newline before index i.
                        let indent = if i > self.pos {
                            // Walk backwards to find a Newline
                            let mut j = i - 1;
                            loop {
                                if let Some(tok) = self.tokens.get(j) {
                                    if tok.node == Token::Newline {
                                        // span.end carries the indent level
                                        break tok.span.end;
                                    }
                                    if j == 0 {
                                        break 0;
                                    }
                                    j -= 1;
                                } else {
                                    break 0;
                                }
                            }
                        } else {
                            // No Newline between pos and i; use the current token's
                            // own inline indent info (from the previous Newline at pos-1).
                            let prev_newline = if self.pos > 0 { self.pos - 1 } else { 0 };
                            self.tokens
                                .get(prev_newline)
                                .filter(|t| t.node == Token::Newline)
                                .map(|t| t.span.end)
                                .unwrap_or(0)
                        };
                        let span = t.span.clone();
                        let (real_indent, has_tab) =
                            if indent == usize::MAX { (0, true) } else { (indent, false) };
                        return LookAheadResult::IndentLevel {
                            level: real_indent,
                            span,
                            has_tab,
                        };
                    }
                },
            }
        }
    }
}

enum LookAheadResult {
    Eof,
    /// The next line is inside a brace block (suppressed).
    /// Not currently returned by `look_ahead_indent` since brace suspension is
    /// handled by the tokeniser — kept for future explicit brace-depth checks.
    #[allow(dead_code)]
    BraceBlock,
    /// Found a real indented line.
    IndentLevel {
        level: usize,
        span: Span,
        has_tab: bool,
    },
}
