//! Lexer — UTF-8 source → token stream with synthesized INDENT/DEDENT tokens.
//!
//! Responsibilities (COMPILER_ARCHITECTURE.md §3):
//! - UTF-8 handling and validation
//! - Tokenisation of all terminals defined in SYNTAX.md §10
//! - INDENT/DEDENT synthesis via the offside-rule pass ([`indent`])
//! - Comment and doc-comment extraction
//! - String interpolation tokenisation (re-enters expression mode inside `{…}`)
//!
//! ## Newline span encoding
//!
//! The raw tokeniser embeds indentation metadata into each `Token::Newline`'s
//! span fields, so the `indent` pass can consume indentation without needing
//! access to the raw source string:
//!
//! - `Newline.span.start` — byte offset of the `\n` character.
//! - `Newline.span.end`   — number of *leading spaces* on the next
//!   non-blank, non-comment line.  `usize::MAX` is used as a sentinel to
//!   signal "tabs present in indentation" (error E0002).
//!
//! No other consumer of `Token::Newline` cares about `span.end` before the
//! indent pass rewrites it; the parser only sees the post-pass stream.
//!
//! The lexer MUST NOT perform any semantic reasoning. It produces a flat token
//! stream; all structural meaning is derived by the parser from that stream.

pub mod indent;
pub mod token;

#[cfg(test)]
mod tests;

use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};
pub use token::Token;

/// A token together with its source span.
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

/// Lex `source` into a stream of spanned tokens, running the offside-rule pass
/// to synthesise INDENT/DEDENT tokens.
///
/// Errors are emitted into `sink`; the returned `Vec` may still contain tokens
/// up to the first error for error-recovery purposes.
pub fn lex(source: &str, sink: &mut DiagnosticSink) -> Vec<Spanned<Token>> {
    let raw = tokenise(source, sink);
    indent::synthesise_indent_dedent(raw, sink)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal tokeniser
// ─────────────────────────────────────────────────────────────────────────────

/// Produce the raw token stream (no INDENT/DEDENT yet).
fn tokenise(source: &str, sink: &mut DiagnosticSink) -> Vec<Spanned<Token>> {
    let mut lexer = Lexer::new(source, sink);
    lexer.run();
    lexer.tokens
}

struct Lexer<'src, 'sink> {
    source: &'src str,
    /// Current byte offset into `source`.
    pos: usize,
    tokens: Vec<Spanned<Token>>,
    sink: &'sink mut DiagnosticSink,
    /// Nesting depth: while any is > 0, newlines do not emit Token::Newline
    /// (continuation context).
    paren_depth: u32,
    bracket_depth: u32,
    /// Brace depth: inside `{ }` newlines are suppressed AND the INDENT/DEDENT
    /// pass is suspended.  We track this separately from parens/brackets so the
    /// INDENT/DEDENT pass can mirror it.
    brace_depth: u32,
    /// Stack of brace depths at which interpolation expressions began.
    /// When the brace depth falls back to an entry on this stack, the closing
    /// `}` resumes string scanning rather than emitting `Token::RBrace`.
    interp_stack: Vec<u32>,
}

impl<'src, 'sink> Lexer<'src, 'sink> {
    fn new(source: &'src str, sink: &'sink mut DiagnosticSink) -> Self {
        Lexer {
            source,
            pos: 0,
            tokens: Vec::new(),
            sink,
            paren_depth: 0,
            bracket_depth: 0,
            brace_depth: 0,
            interp_stack: Vec::new(),
        }
    }

    fn in_continuation(&self) -> bool {
        self.paren_depth > 0 || self.bracket_depth > 0 || self.brace_depth > 0
    }

    fn run(&mut self) {
        while self.pos < self.source.len() {
            self.scan_one();
        }
        // Ensure the stream ends with a Newline so the INDENT/DEDENT pass can
        // close any remaining open blocks.
        let needs_newline = self
            .tokens
            .last()
            .map(|t| t.node != Token::Newline)
            .unwrap_or(true);
        if needs_newline {
            let end = self.source.len();
            // At EOF the next line has indent 0.
            self.push_newline(end, 0);
        }
        let end = self.source.len();
        self.push(Token::Eof, end, end);
    }

    fn push(&mut self, tok: Token, start: usize, end: usize) {
        self.tokens.push(Spanned { node: tok, span: Span { start, end } });
    }

    /// Push a `Token::Newline` with the indent level encoded in `span.end`.
    ///
    /// `indent_level` — number of leading spaces on the next non-blank,
    ///   non-comment line; `usize::MAX` signals tab-mixing (E0002).
    fn push_newline(&mut self, newline_byte_offset: usize, indent_level: usize) {
        self.tokens.push(Spanned {
            node: Token::Newline,
            span: Span { start: newline_byte_offset, end: indent_level },
        });
    }

    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut it = self.source[self.pos..].chars();
        it.next();
        it.next()
    }

    fn peek3(&self) -> Option<char> {
        let mut it = self.source[self.pos..].chars();
        it.next();
        it.next();
        it.next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn err_unrecognised(&mut self, ch: char, start: usize) {
        let end = start + ch.len_utf8();
        self.sink.emit(
            Diagnostic::error(format!("unrecognised character `{ch}`"))
                .with_span(Span { start, end }, "unexpected character")
                .with_code("E0001"),
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Indentation scanning helper
    // ─────────────────────────────────────────────────────────────────────────

    /// Starting at `pos` (which is the byte just after a `\n`), peek ahead to
    /// find the next non-blank, non-comment-only line and return its leading
    /// space count.
    ///
    /// Returns `usize::MAX` if a tab character appears in the indentation of
    /// that line (mixed tabs/spaces error).
    ///
    /// Does **not** advance `self.pos`.
    fn peek_next_indent(&self) -> usize {
        let src = &self.source[self.pos..];
        let mut i = 0;
        loop {
            // Skip the leading whitespace of the current candidate line.
            let _line_start = i;
            let mut spaces: usize = 0;
            let mut has_tab = false;
            while i < src.len() {
                match src.as_bytes()[i] {
                    b' ' => {
                        spaces += 1;
                        i += 1;
                    }
                    b'\t' => {
                        has_tab = true;
                        i += 1;
                    }
                    _ => break,
                }
            }

            // Check what follows the whitespace.
            if i >= src.len() {
                // EOF — indent 0 to close all blocks.
                return 0;
            }
            let next_byte = src.as_bytes()[i];
            match next_byte {
                b'\n' | b'\r' => {
                    // Blank line — skip and try the next line.
                    i += 1; // skip '\n' (handle '\r\n' too)
                    if next_byte == b'\r' && i < src.len() && src.as_bytes()[i] == b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'/' if i + 1 < src.len() && (src.as_bytes()[i + 1] == b'/' || src.as_bytes()[i + 1] == b'*') => {
                    // Comment-only line — skip to end of line and continue.
                    let is_block = src.as_bytes()[i + 1] == b'*';
                    if is_block {
                        // Skip block comment
                        i += 2;
                        loop {
                            if i + 1 >= src.len() { break; }
                            if src.as_bytes()[i] == b'*' && src.as_bytes()[i + 1] == b'/' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    } else {
                        // Skip line comment to end of line
                        while i < src.len() && src.as_bytes()[i] != b'\n' {
                            i += 1;
                        }
                    }
                    // Skip any trailing whitespace/newline
                    while i < src.len() && matches!(src.as_bytes()[i], b' ' | b'\t' | b'\r') {
                        i += 1;
                    }
                    if i < src.len() && src.as_bytes()[i] == b'\n' {
                        i += 1;
                    }
                    // The comment line does NOT affect indentation tracking.
                    // The indentation of this *comment* line is not the indent
                    // we care about — we want the next real line.
                    continue;
                }
                _ => {
                    // This is a real line.
                    if has_tab {
                        return usize::MAX; // tab mixing sentinel
                    }
                    return spaces;
                }
            }
        }
    }

    fn scan_one(&mut self) {
        let start = self.pos;
        let ch = match self.advance() {
            Some(c) => c,
            None => return,
        };

        match ch {
            // ── Horizontal whitespace (not indentation — indentation is in
            //    peek_next_indent()) ────────────────────────────────────────
            ' ' | '\t' | '\r' => {}

            // ── Newlines ─────────────────────────────────────────────────────
            '\n' => {
                if !self.in_continuation() {
                    // Avoid adjacent Newline tokens.
                    let already = self
                        .tokens
                        .last()
                        .map(|t| t.node == Token::Newline)
                        .unwrap_or(false);
                    if !already {
                        let indent = self.peek_next_indent();
                        self.push_newline(start, indent);
                    }
                }
            }

            // ── Comments ─────────────────────────────────────────────────────
            '/' if matches!(self.peek(), Some('/') | Some('*')) => {
                self.scan_comment(start);
            }

            // ── String literals ──────────────────────────────────────────────
            '"' => self.scan_string(start),

            // ── Character literals ───────────────────────────────────────────
            '\'' => self.scan_char(start),

            // ── Numeric literals ─────────────────────────────────────────────
            '0'..='9' => self.scan_number(ch, start),

            // ── Identifiers and keywords ─────────────────────────────────────
            c if is_ident_start(c) => self.scan_ident(start),

            // ── Operators and punctuation ────────────────────────────────────
            '+' => {
                if self.eat('=') {
                    self.push(Token::PlusEq, start, self.pos);
                } else {
                    self.push(Token::Plus, start, self.pos);
                }
            }
            '-' => {
                if self.eat('>') {
                    self.push(Token::Arrow, start, self.pos);
                } else if self.eat('=') {
                    self.push(Token::MinusEq, start, self.pos);
                } else {
                    self.push(Token::Minus, start, self.pos);
                }
            }
            '*' => {
                if self.eat('=') {
                    self.push(Token::StarEq, start, self.pos);
                } else {
                    self.push(Token::Star, start, self.pos);
                }
            }
            '/' => {
                // The `'/' if peek...` arm above handles `//` and `/*`.
                // Reaching here means a plain `/` (division) or `/=`.
                if self.eat('=') {
                    self.push(Token::SlashEq, start, self.pos);
                } else {
                    self.push(Token::Slash, start, self.pos);
                }
            }
            '%' => {
                if self.eat('=') {
                    self.push(Token::PercentEq, start, self.pos);
                } else {
                    self.push(Token::Percent, start, self.pos);
                }
            }
            '=' => {
                if self.eat('=') {
                    self.push(Token::EqEq, start, self.pos);
                } else {
                    self.push(Token::Eq, start, self.pos);
                }
            }
            '!' => {
                if self.eat('=') {
                    self.push(Token::BangEq, start, self.pos);
                } else {
                    self.push(Token::Bang, start, self.pos);
                }
            }
            '<' => {
                if self.eat('=') {
                    self.push(Token::LtEq, start, self.pos);
                } else {
                    self.push(Token::Lt, start, self.pos);
                }
            }
            '>' => {
                if self.eat('=') {
                    self.push(Token::GtEq, start, self.pos);
                } else {
                    self.push(Token::Gt, start, self.pos);
                }
            }
            '&' => {
                if self.eat('&') {
                    self.push(Token::AmpAmp, start, self.pos);
                } else {
                    self.err_unrecognised('&', start);
                }
            }
            '|' => {
                if self.eat('|') {
                    self.push(Token::PipePipe, start, self.pos);
                } else {
                    self.push(Token::Pipe, start, self.pos);
                }
            }
            '?' => {
                if self.eat('?') {
                    self.push(Token::QuestionQuestion, start, self.pos);
                } else {
                    self.push(Token::Question, start, self.pos);
                }
            }
            '.' => {
                if self.peek() == Some('.') {
                    self.advance();
                    if self.eat('=') {
                        self.push(Token::DotDotEq, start, self.pos);
                    } else if self.peek() == Some('.') {
                        self.advance();
                        self.push(Token::DotDotDot, start, self.pos);
                    } else {
                        self.push(Token::DotDot, start, self.pos);
                    }
                } else {
                    self.push(Token::Dot, start, self.pos);
                }
            }
            ':' => {
                if self.eat(':') {
                    self.push(Token::ColonColon, start, self.pos);
                } else {
                    self.push(Token::Colon, start, self.pos);
                }
            }
            ',' => self.push(Token::Comma, start, self.pos),
            ';' => self.push(Token::Semi, start, self.pos),
            '(' => {
                self.paren_depth += 1;
                self.push(Token::LParen, start, self.pos);
            }
            ')' => {
                self.paren_depth = self.paren_depth.saturating_sub(1);
                self.push(Token::RParen, start, self.pos);
            }
            '[' => {
                self.bracket_depth += 1;
                self.push(Token::LBracket, start, self.pos);
            }
            ']' => {
                self.bracket_depth = self.bracket_depth.saturating_sub(1);
                self.push(Token::RBracket, start, self.pos);
            }
            '{' => {
                self.brace_depth += 1;
                self.push(Token::LBrace, start, self.pos);
            }
            '}' => {
                // Check if this `}` closes a string interpolation expression.
                let cur = self.brace_depth;
                if self.interp_stack.last().copied() == Some(cur) {
                    self.interp_stack.pop();
                    self.brace_depth = self.brace_depth.saturating_sub(1);
                    // Resume scanning the interpolated string tail.
                    self.scan_string_interp_tail(start);
                } else {
                    self.brace_depth = self.brace_depth.saturating_sub(1);
                    self.push(Token::RBrace, start, self.pos);
                }
            }

            other => self.err_unrecognised(other, start),
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Comments
    // ─────────────────────────────────────────────────────────────────────────

    fn scan_comment(&mut self, start: usize) {
        match self.peek() {
            Some('/') => {
                self.advance(); // second '/'
                if self.peek() == Some('/') {
                    // `///` doc comment
                    self.advance();
                    let text = self.consume_line_content();
                    self.push(Token::DocComment(text), start, self.pos);
                } else if self.peek() == Some('!') {
                    // `//!` module doc comment
                    self.advance();
                    let text = self.consume_line_content();
                    self.push(Token::ModDocComment(text), start, self.pos);
                } else {
                    // plain `//` line comment
                    let text = self.consume_line_content();
                    self.push(Token::LineComment(text), start, self.pos);
                }
            }
            Some('*') => {
                self.advance(); // consume '*'
                let text = self.consume_block_comment();
                self.push(Token::BlockComment(text), start, self.pos);
            }
            _ => unreachable!(),
        }
    }

    fn consume_line_content(&mut self) -> String {
        // Skip optional single space after comment marker
        if self.peek() == Some(' ') {
            self.advance();
        }
        let s = self.pos;
        while self.peek().map(|c| c != '\n').unwrap_or(false) {
            self.advance();
        }
        self.source[s..self.pos].to_string()
    }

    fn consume_block_comment(&mut self) -> String {
        let s = self.pos;
        loop {
            match self.advance() {
                None => break,
                Some('*') if self.peek() == Some('/') => {
                    let body_end = self.pos - 1;
                    self.advance(); // '/'
                    return self.source[s..body_end].to_string();
                }
                _ => {}
            }
        }
        self.source[s..self.pos].to_string()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // String literals
    // ─────────────────────────────────────────────────────────────────────────

    fn scan_string(&mut self, start: usize) {
        // Check for `"""` multiline string
        if self.peek() == Some('"') && self.peek2() == Some('"') {
            self.advance();
            self.advance();
            self.scan_multiline_string(start);
        } else {
            self.scan_single_string(start);
        }
    }

    fn scan_multiline_string(&mut self, start: usize) {
        let mut text = String::new();
        loop {
            if self.peek() == Some('"')
                && self.peek2() == Some('"')
                && self.peek3() == Some('"')
            {
                self.advance();
                self.advance();
                self.advance();
                break;
            }
            match self.peek() {
                None => {
                    self.sink.emit(
                        Diagnostic::error("unterminated multiline string literal")
                            .with_span(Span { start, end: self.pos }, "starts here")
                            .with_code("E0001"),
                    );
                    break;
                }
                Some('\\') => {
                    self.advance();
                    if let Some(c) = self.scan_escape(start) {
                        text.push(c);
                    }
                }
                Some(c) => {
                    self.advance();
                    text.push(c);
                }
            }
        }
        self.push(Token::StringLit(text), start, self.pos);
    }

    /// Scan a `"…"` string starting after the opening `"` has been consumed.
    fn scan_single_string(&mut self, start: usize) {
        let mut text = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    self.sink.emit(
                        Diagnostic::error("unterminated string literal")
                            .with_span(Span { start, end: self.pos }, "starts here")
                            .with_code("E0001"),
                    );
                    self.push(Token::StringLit(text), start, self.pos);
                    return;
                }
                Some('"') => {
                    self.advance();
                    self.push(Token::StringLit(text), start, self.pos);
                    return;
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('{') => { self.advance(); text.push('{'); }
                        Some('}') => { self.advance(); text.push('}'); }
                        _ => {
                            if let Some(c) = self.scan_escape(start) {
                                text.push(c);
                            }
                        }
                    }
                }
                Some('{') => {
                    // Start of interpolation expression
                    self.advance(); // consume '{'
                    let end_pos = self.pos;
                    self.push(Token::InterpStart(text), start, end_pos);
                    self.brace_depth += 1;
                    self.interp_stack.push(self.brace_depth);
                    return;
                }
                Some(c) => {
                    self.advance();
                    text.push(c);
                }
            }
        }
    }

    /// Called when `}` closes a string interpolation; `close_start` is its offset.
    fn scan_string_interp_tail(&mut self, close_start: usize) {
        let mut text = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    self.sink.emit(
                        Diagnostic::error("unterminated interpolated string literal")
                            .with_span(
                                Span { start: close_start, end: self.pos },
                                "starts here",
                            )
                            .with_code("E0001"),
                    );
                    self.push(Token::InterpEnd(text), close_start, self.pos);
                    return;
                }
                Some('"') => {
                    self.advance();
                    self.push(Token::InterpEnd(text), close_start, self.pos);
                    return;
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('{') => { self.advance(); text.push('{'); }
                        Some('}') => { self.advance(); text.push('}'); }
                        _ => {
                            if let Some(c) = self.scan_escape(close_start) {
                                text.push(c);
                            }
                        }
                    }
                }
                Some('{') => {
                    self.advance();
                    let end_pos = self.pos;
                    self.push(Token::InterpMiddle(text), close_start, end_pos);
                    self.brace_depth += 1;
                    self.interp_stack.push(self.brace_depth);
                    return;
                }
                Some(c) => {
                    self.advance();
                    text.push(c);
                }
            }
        }
    }

    fn scan_escape(&mut self, ctx_start: usize) -> Option<char> {
        let esc_start = self.pos;
        match self.advance() {
            Some('n')  => Some('\n'),
            Some('t')  => Some('\t'),
            Some('r')  => Some('\r'),
            Some('\\') => Some('\\'),
            Some('\'') => Some('\''),
            Some('"')  => Some('"'),
            Some('0')  => Some('\0'),
            Some('u')  => {
                if !self.eat('{') {
                    let end = self.pos;
                    self.sink.emit(
                        Diagnostic::error("expected `{` after `\\u` in Unicode escape")
                            .with_span(Span { start: esc_start, end }, "here")
                            .with_code("E0001"),
                    );
                    return None;
                }
                let hex_s = self.pos;
                while self.peek().map(|c| c.is_ascii_hexdigit()).unwrap_or(false) {
                    self.advance();
                }
                let hex_e = self.pos;
                if !self.eat('}') {
                    let end = self.pos;
                    self.sink.emit(
                        Diagnostic::error("expected `}` to close Unicode escape")
                            .with_span(Span { start: esc_start, end }, "here")
                            .with_code("E0001"),
                    );
                    return None;
                }
                let hex = &self.source[hex_s..hex_e];
                match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => Some(c),
                    None => {
                        let end = self.pos;
                        self.sink.emit(
                            Diagnostic::error(format!(
                                "invalid Unicode codepoint `\\u{{{hex}}}`"
                            ))
                            .with_span(Span { start: esc_start, end }, "here")
                            .with_code("E0001"),
                        );
                        None
                    }
                }
            }
            Some(c) => {
                let end = self.pos;
                self.sink.emit(
                    Diagnostic::error(format!("unknown escape sequence `\\{c}`"))
                        .with_span(Span { start: ctx_start, end }, "here")
                        .with_code("E0001"),
                );
                None
            }
            None => {
                let end = self.pos;
                self.sink.emit(
                    Diagnostic::error("unexpected end of file in escape sequence")
                        .with_span(Span { start: ctx_start, end }, "here")
                        .with_code("E0001"),
                );
                None
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Character literals
    // ─────────────────────────────────────────────────────────────────────────

    fn scan_char(&mut self, start: usize) {
        let ch = match self.peek() {
            None => {
                self.sink.emit(
                    Diagnostic::error("unterminated character literal")
                        .with_span(Span { start, end: self.pos }, "here")
                        .with_code("E0001"),
                );
                return;
            }
            Some('\'') => {
                self.advance();
                self.sink.emit(
                    Diagnostic::error("empty character literal")
                        .with_span(Span { start, end: self.pos }, "here")
                        .with_code("E0001"),
                );
                return;
            }
            Some('\\') => {
                self.advance();
                match self.scan_escape(start) {
                    Some(c) => c,
                    None => {
                        while self.peek().map(|c| c != '\'').unwrap_or(false) {
                            self.advance();
                        }
                        self.eat('\'');
                        return;
                    }
                }
            }
            Some(c) => {
                self.advance();
                c
            }
        };
        if !self.eat('\'') {
            let end = self.pos;
            self.sink.emit(
                Diagnostic::error("unterminated character literal, expected closing `'`")
                    .with_span(Span { start, end }, "here")
                    .with_code("E0001"),
            );
            while self.peek().map(|c| c != '\'').unwrap_or(false) {
                self.advance();
            }
            self.eat('\'');
            return;
        }
        self.push(Token::CharLit(ch), start, self.pos);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Numeric literals
    // ─────────────────────────────────────────────────────────────────────────

    fn scan_number(&mut self, first: char, start: usize) {
        if first == '0' {
            match self.peek() {
                Some('x') | Some('X') => {
                    self.advance();
                    return self.scan_int_radix(start, 16, |c| c.is_ascii_hexdigit() || c == '_');
                }
                Some('o') | Some('O') => {
                    self.advance();
                    return self.scan_int_radix(start, 8, |c| {
                        matches!(c, '0'..='7' | '_')
                    });
                }
                Some('b') | Some('B') => {
                    self.advance();
                    return self.scan_int_radix(start, 2, |c| matches!(c, '0' | '1' | '_'));
                }
                _ => {}
            }
        }

        // Consume remaining decimal digits and underscores.
        while self.peek().map(|c| c.is_ascii_digit() || c == '_').unwrap_or(false) {
            self.advance();
        }

        // Float if followed by `.digit` or by `e`/`E`
        let is_float = (self.peek() == Some('.')
            && self.peek2().map(|c| c.is_ascii_digit()).unwrap_or(false))
            || matches!(self.peek(), Some('e') | Some('E'));

        if is_float {
            self.scan_float_tail(start);
        } else {
            let raw = &self.source[start..self.pos];
            let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
            match cleaned.parse::<i128>() {
                Ok(n) => self.push(Token::IntLit(n), start, self.pos),
                Err(_) => {
                    let end = self.pos;
                    self.sink.emit(
                        Diagnostic::error(format!("integer literal `{raw}` is out of range"))
                            .with_span(Span { start, end }, "here")
                            .with_code("E0001"),
                    );
                }
            }
        }
    }

    fn scan_int_radix(&mut self, start: usize, radix: u32, valid: impl Fn(char) -> bool) {
        let digits_start = self.pos;
        while self.peek().map(|c| valid(c)).unwrap_or(false) {
            self.advance();
        }
        let raw = &self.source[digits_start..self.pos];
        let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
        match i128::from_str_radix(&cleaned, radix) {
            Ok(n) => self.push(Token::IntLit(n), start, self.pos),
            Err(_) => {
                let end = self.pos;
                self.sink.emit(
                    Diagnostic::error(format!(
                        "invalid integer literal `{}`",
                        &self.source[start..end]
                    ))
                    .with_span(Span { start, end }, "here")
                    .with_code("E0001"),
                );
            }
        }
    }

    fn scan_float_tail(&mut self, start: usize) {
        if self.peek() == Some('.') && self.peek2().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.advance(); // '.'
            while self.peek().map(|c| c.is_ascii_digit() || c == '_').unwrap_or(false) {
                self.advance();
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.advance();
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.advance();
            }
            while self.peek().map(|c| c.is_ascii_digit() || c == '_').unwrap_or(false) {
                self.advance();
            }
        }
        let raw = &self.source[start..self.pos];
        let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
        match cleaned.parse::<f64>() {
            Ok(f) => self.push(Token::FloatLit(f), start, self.pos),
            Err(_) => {
                let end = self.pos;
                self.sink.emit(
                    Diagnostic::error(format!("invalid float literal `{raw}`"))
                        .with_span(Span { start, end }, "here")
                        .with_code("E0001"),
                );
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Identifiers and keywords
    // ─────────────────────────────────────────────────────────────────────────

    fn scan_ident(&mut self, start: usize) {
        while self.peek().map(is_ident_continue).unwrap_or(false) {
            self.advance();
        }
        let text = &self.source[start..self.pos];
        let tok = keyword_or_ident(text);
        self.push(tok, start, self.pos);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unicode identifier helpers (UAX #31)
// ─────────────────────────────────────────────────────────────────────────────

fn is_ident_start(c: char) -> bool {
    c == '_' || unicode_ident::is_xid_start(c)
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || unicode_ident::is_xid_continue(c)
}

// ─────────────────────────────────────────────────────────────────────────────
// Keyword lookup (LANGUAGE_SPEC.md §7)
// ─────────────────────────────────────────────────────────────────────────────

fn keyword_or_ident(s: &str) -> Token {
    match s {
        // §7.1 Core keywords — `true`/`false` produce BoolLit, not keyword tokens
        "as"       => Token::As,
        "async"    => Token::Async,
        "await"    => Token::Await,
        "break"    => Token::Break,
        "const"    => Token::Const,
        "continue" => Token::Continue,
        "else"     => Token::Else,
        "export"   => Token::Export,
        "false"    => Token::BoolLit(false),
        "for"      => Token::For,
        "if"       => Token::If,
        "import"   => Token::Import,
        "in"       => Token::In,
        "let"      => Token::Let,
        "loop"     => Token::Loop,
        "match"    => Token::Match,
        "mod"      => Token::Mod,
        "return"   => Token::Return,
        "true"     => Token::BoolLit(true),
        "use"      => Token::Use,
        "var"      => Token::Var,
        "while"    => Token::While,

        // §7.2 Type/system keywords
        "derive" => Token::Derive,
        "enum"   => Token::Enum,
        "fn"     => Token::Fn,
        "impl"   => Token::Impl,
        "struct" => Token::Struct,
        "trait"  => Token::Trait,
        "type"   => Token::Type,
        "unsafe" => Token::Unsafe,

        // §7.3 Memory/concurrency keywords
        "owned"   => Token::Owned,
        "borrow"  => Token::Borrow,
        "managed" => Token::Managed,
        "weak"    => Token::Weak,
        "unowned" => Token::Unowned,
        "task"    => Token::Task,

        // §7.4 Reserved-for-future-use keywords
        "actor"    => Token::Actor,
        "defer"    => Token::Defer,
        "extern"   => Token::Extern,
        "macro"    => Token::Macro,
        "native"   => Token::Native,
        "operator" => Token::Operator,
        "protocol" => Token::Protocol,
        "reflect"  => Token::Reflect,
        "spawn"    => Token::Spawn,
        "static"   => Token::Static,
        "where"    => Token::Where,
        "yield"    => Token::Yield,

        _ => Token::Ident(s.to_string()),
    }
}
