//! `noct fmt --v2` — AST printer (P-004 v2 scope).
//!
//! Prints the PARSED program (pre-resolve) in expanded canonical form.
//! Pre-resolve is load-bearing for D6: concise (`User:`,
//! `calculate(x: Int) -> Int:`) and explicit (`struct`/`fn`) spellings
//! are distinct AST nodes (`BareDecl` vs `Function`), so printing each
//! as written never rewrites the author's choice. Statement-level
//! functions print bare with two load-bearing exceptions (empty
//! parameter lists drop their parens — `name():` would reparse as a
//! trailing-block call — and single-expression bodies keep `fn` —
//! bare `name: expr` would reparse as `Decl`); both were caught by
//! the equivalence gate, not by reasoning.
//!
//! Canonical form (D1/D2/D4/D5/D6):
//! - 4-space indents (D1); LF; exactly one `\n` at EOF; at most one
//!   blank line, and only between top-level items (D2).
//! - One statement per line; `;` is never emitted except by the
//!   density heuristic (D3 — the parser already drops `;`, so removal
//!   is automatic).
//! - Density heuristic: runs of tiny single-line statements (plain
//!   `let`/`var`/`state`/`assign`/`decl`/simple-expression, no attached
//!   comments) join with `; ` while the joined line stays within the
//!   100-column cap (D5). Anything else stays expanded (D4).
//! - Calls/lists/struct literals that fit print single-line; past the
//!   cap they break (one element per line, trailing commas) — SYNTAX
//!   §7 requires both directions.
//! - Expressions gain one-space operator padding; literals print in
//!   reparse-stable spellings (ints decimal, floats via `{:?}` so `1.0`
//!   never becomes `1`, strings/char re-escaped with exactly the
//!   escapes the lexer reads, `{`/`}` as `\{`/`\}`).
//! - Operator nesting parenthesizes by SYNTAX §9 precedence (a
//!   same-or-looser right child, a strictly looser left child, and any
//!   non-atom in operand/base/callee position) so the output always
//!   reparses to the same tree. `Variant()` keeps its parens (dropping
//!   them would turn a variant test into a binding — the L-001 trap).
//!
//! Comments: comment-only lines above a node attach leading (greedily —
//! blank lines never orphan a comment; association may shift but no
//! byte is ever lost); a comment trailing a node's last line attaches
//! trailing; `//!` file docs hoist to the header; leftovers flush at
//! EOF. Inline block comments inside code lines are REFUSED loud — v2
//! only handles what it provably preserves.
//!
//! Gates (all blocking, like v1): AST-equivalence (reparse the output
//! and compare structurally, spans ignored — skipped only when the
//! INPUT has parse errors, where recovery trees can legally wobble;
//! no-new-errors still applies there), idempotency, no-new-errors.

use std::collections::{HashMap, HashSet};

use compiler::ast::*;
use compiler::diagnostics::DiagnosticSink;
use compiler::lexer::{Spanned, Token};

/// Loud refusal (v2 prints nothing on these paths).
#[derive(Debug)]
pub struct Fmt2Error {
    pub message: String,
}

const LINE_CAP: usize = 100;
const INDENT: &str = "    ";

/// Format with the v2 AST printer, running all three gates.
pub fn format_v2(source: &str) -> Result<String, Fmt2Error> {
    check_no_tab_indent(source)?;
    let (program, tokens, in_errors) = parse_keep_tokens(source);
    // v2 reasons about structure, so it requires parseable input
    // (v1's byte-level trivia mode stays available for the rest —
    // refusing here reports an input problem, never a formatter bug).
    if in_errors > 0 {
        return Err(Fmt2Error {
            message: format!(
                "cannot format: input has {in_errors} parse error(s); fix them or use v1 trivia mode"
            ),
        });
    }
    let out = print_with_tokens(&program, &tokens, source)?;
    // Gate 1: AST-equivalence (input is clean by the check above, so
    // recovery wobble cannot excuse a mismatch).
    let (program2, _, out_errors) = parse_keep_tokens(&out);
    if !ast_eq_program(&program, &program2) {
        return Err(Fmt2Error {
            message: "formatter bug (v2 output parses to a different AST); output not written"
                .to_string(),
        });
    }
    // Gate 2: idempotency.
    let (program2b, tokens2, _) = parse_keep_tokens(&out);
    let out2 = print_with_tokens(&program2b, &tokens2, &out)?;
    if out2 != out {
        return Err(Fmt2Error {
            message: "formatter bug (v2 output is not idempotent); output not written".to_string(),
        });
    }
    // Gate 3: no-new-errors.
    if out_errors > in_errors {
        return Err(Fmt2Error {
            message: format!(
                "refusing to write: v2 output has more errors ({out_errors}) than input ({in_errors})"
            ),
        });
    }
    Ok(out)
}

/// D1: tabs in leading whitespace are a loud error, never silently
/// re-indented (same rule as v1 — v2 reprints all indentation, which
/// would otherwise launder tabs without a word). Span lines are
/// opaque; blank lines exempt (their whitespace dies as trailing).
fn check_no_tab_indent(source: &str) -> Result<(), Fmt2Error> {
    let mut in_triple = false;
    for (index, line) in source.replace("\r\n", "\n").split('\n').enumerate() {
        if in_triple {
            if line.matches("\"\"\"").count() % 2 == 1 {
                in_triple = false;
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        // A `"""` opener ends code for this line (same conservatism as v1).
        let code = match line.find("\"\"\"") {
            Some(i) => &line[..i],
            None => line,
        };
        let indent_len = code.len() - code.trim_start_matches([' ', '\t']).len();
        if code[..indent_len].contains('\t') {
            return Err(Fmt2Error {
                message: format!(
                    "line {}: tab in leading whitespace (use 4 spaces per level)",
                    index + 1
                ),
            });
        }
        if line.matches("\"\"\"").count() % 2 == 1 {
            in_triple = true;
        }
    }
    Ok(())
}

fn parse_keep_tokens(source: &str) -> (Program, Vec<Spanned<Token>>, usize) {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let errors = sink.diagnostics().iter().filter(|d| d.is_error()).count();
    (program, tokens, errors)
}

fn print_with_tokens(
    program: &Program,
    tokens: &[Spanned<Token>],
    source: &str,
) -> Result<String, Fmt2Error> {
    let mut cx = Cx::build(source, tokens)?;
    let mut out = String::new();
    // `//!` file docs first, then a blank line when more follows.
    let mut first = true;
    for c in cx.take_moddocs() {
        out.push_str(&c);
        out.push('\n');
        first = false;
    }
    if !program.imports.is_empty() {
        if !first {
            out.push('\n');
        }
        for import in &program.imports {
            // Imports carry no comments in practice (comment lines above
            // an import attach to it like any node).
            for line in cx.leading_for(import.span.start) {
                out.push_str(&indent_lines(&line, 0));
                out.push('\n');
            }
            out.push_str(&print_import(import));
            out.push('\n');
        }
        first = false;
    }
    for item in &program.items {
        if !first {
            out.push('\n');
        }
        first = false;
        for line in print_item(item, 0, &mut cx) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    // Orphaned trailing comments (after the last item) flush at EOF.
    for line in cx.flush_rest() {
        out.push_str(&line);
        out.push('\n');
    }
    if out.is_empty() {
        return Ok("\n".to_string());
    }
    Ok(out)
}

// ── Comment table ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentKind {
    Line,
    Doc,
    ModDoc,
    Block,
}

struct Comment {
    start_line: usize,
    end_line: usize,
    start_off: usize,
    kind: CommentKind,
    text: String,
}

struct Cx<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
    comments: Vec<Comment>,
    consumed: HashSet<usize>,
}

impl<'a> Cx<'a> {
    fn build(source: &'a str, tokens: &[Spanned<Token>]) -> Result<Self, Fmt2Error> {
        let mut line_starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let line_of = |off: usize| -> usize {
            match line_starts.binary_search(&off.min(source.len())) {
                Ok(l) => l + 1,
                Err(l) => l,
            }
        };
        // Lines containing real code (anything but comments and the
        // structural trivia tokens), with code-token spans per line
        // (for the inline-block-comment rule below).
        let mut code_spans: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        let mut comments = Vec::new();
        for tok in tokens {
            let is_trivia = matches!(
                tok.node,
                Token::Newline | Token::Indent | Token::Dedent | Token::Eof
            );
            let kinded: Option<(CommentKind, String)> = match &tok.node {
                Token::LineComment(t) => Some((CommentKind::Line, t.clone())),
                Token::DocComment(t) => Some((CommentKind::Doc, t.clone())),
                Token::ModDocComment(t) => Some((CommentKind::ModDoc, t.clone())),
                Token::BlockComment(t) => Some((CommentKind::Block, t.clone())),
                _ => None,
            };
            if let Some((kind, text)) = kinded {
                let start_line = line_of(tok.span.start);
                let end_line = line_of(tok.span.end.min(source.len()));
                comments.push(Comment {
                    start_line,
                    end_line,
                    start_off: tok.span.start,
                    kind,
                    text,
                });
            } else if !is_trivia {
                let (mut l0, mut l1) = (
                    line_of(tok.span.start),
                    line_of(tok.span.end.min(source.len())),
                );
                if l1 < l0 {
                    std::mem::swap(&mut l0, &mut l1);
                }
                for l in l0..=l1 {
                    code_spans.entry(l).or_default().push((tok.span.start, tok.span.end));
                }
            }
        }
        // Inline block comments inside code lines are refused:
        // attachment cannot place them without moving bytes.
        for c in &comments {
            if c.kind != CommentKind::Block {
                continue;
            }
            if block_comment_is_inline(c, &code_spans) {
                return Err(Fmt2Error {
                    message: format!(
                        "inline block comment on line {} (v2 keeps comments only on their own lines or trailing); split it out",
                        c.start_line
                    ),
                });
            }
        }
        Ok(Cx {
            source,
            line_starts,
            comments,
            consumed: HashSet::new(),
        })
    }

    fn line_of(&self, off: usize) -> usize {
        match self.line_starts.binary_search(&off.min(self.source.len())) {
            Ok(l) => l + 1,
            Err(l) => l,
        }
    }

    fn take_moddocs(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, c) in self.comments.iter().enumerate() {
            if c.kind == CommentKind::ModDoc && !self.consumed.contains(&i) {
                self.consumed.insert(i);
                out.push(render_comment(c, 0));
            }
        }
        out
    }

    /// Leading comments for a node starting at `node_start_off`:
    /// every unconsumed comment strictly above its first line.
    /// Greedy by design (blank lines never orphan a comment — only
    /// association can shift, never bytes), and overlap-free in
    /// practice: same-line trailing comments are consumed when their
    /// node prints, before any later sibling looks.
    fn leading_for(&mut self, node_start_off: usize) -> Vec<String> {
        let first_line = self.line_of(node_start_off);
        let mut picked: Vec<usize> = Vec::new();
        for (i, c) in self.comments.iter().enumerate() {
            if self.consumed.contains(&i) || c.kind == CommentKind::ModDoc {
                continue;
            }
            if c.end_line < first_line
                || (c.end_line == first_line && c.start_off < node_start_off)
            {
                picked.push(i);
            }
        }
        let mut out = Vec::new();
        for i in picked {
            self.consumed.insert(i);
            out.push(render_comment(&self.comments[i], 0));
        }
        out
    }

    /// Trailing comment: first unconsumed comment starting on
    /// `last_line` at/after `end_off`. Rendered `  // text`.
    fn trailing_for(&mut self, end_off: usize) -> Option<String> {
        let last_line = self.line_of(end_off);
        for (i, c) in self.comments.iter().enumerate() {
            if self.consumed.contains(&i) || c.kind == CommentKind::ModDoc {
                continue;
            }
            if c.start_line == last_line && c.start_off >= end_off {
                self.consumed.insert(i);
                return Some(format!("  {}", render_comment(c, 0)));
            }
        }
        None
    }

    /// Orphaned comment-only lines (after the last node): flush raw.
    fn flush_rest(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, c) in self.comments.iter().enumerate() {
            if self.consumed.contains(&i) || c.kind == CommentKind::ModDoc {
                continue;
            }
            self.consumed.insert(i);
            out.push(render_comment(c, 0));
        }
        out
    }
}

/// A block comment is placeable iff it owns its lines, or is a
/// single-line trailer (code only before it on its line). Anything
/// else (code after it, code on continuation lines) strands it.
fn block_comment_is_inline(c: &Comment, code_spans: &HashMap<usize, Vec<(usize, usize)>>) -> bool {
    if c.start_line == c.end_line {
        // Single line: refuse only when code starts at/after the comment.
        if let Some(spans) = code_spans.get(&c.start_line) {
            return spans.iter().any(|(s, _)| *s >= c.start_off);
        }
        return false;
    }
    // Multi-line: refuse on code anywhere in the span.
    (c.start_line..=c.end_line).any(|l| code_spans.contains_key(&l))
}

fn render_comment(c: &Comment, level: usize) -> String {
    let pad = INDENT.repeat(level);
    match c.kind {
        CommentKind::Line => {
            if c.text.is_empty() {
                format!("{pad}//")
            } else {
                format!("{pad}// {}", c.text)
            }
        }
        CommentKind::Doc => {
            if c.text.is_empty() {
                format!("{pad}///")
            } else {
                format!("{pad}/// {}", c.text)
            }
        }
        CommentKind::ModDoc => {
            if c.text.is_empty() {
                format!("{pad}//!")
            } else {
                format!("{pad}//! {}", c.text)
            }
        }
        CommentKind::Block => {
            // Body verbatim (may span lines — opaque like `"""`).
            let mut parts = c.text.split('\n').peekable();
            let mut out = String::new();
            if let Some(first) = parts.next() {
                out.push_str(&pad);
                out.push_str("/*");
                out.push_str(first);
                if parts.peek().is_none() {
                    out.push_str("*/");
                    return out;
                }
                for part in parts {
                    out.push('\n');
                    out.push_str(part);
                }
                out.push_str("*/");
            }
            out
        }
    }
}

fn indent_lines(text: &str, level: usize) -> String {
    // (Leading-comment lines from `leading_for` arrive unindented;
    // multi-line block bodies stay raw past the first line.)
    let pad = INDENT.repeat(level);
    let mut lines = text.split('\n');
    let mut out = String::new();
    if let Some(first) = lines.next() {
        out.push_str(&pad);
        out.push_str(first);
        for rest in lines {
            out.push('\n');
            out.push_str(rest);
        }
    }
    out
}

// ── Items ───────────────────────────────────────────────────────────────────

/// Print one top-level item: leading comments, then node lines, then
/// an optional same-line trailing comment on the node's last line.
fn print_item(item: &Item, level: usize, cx: &mut Cx) -> Vec<String> {
    let first_off = item_span(item).start;
    let mut lines: Vec<String> = cx
        .leading_for(first_off)
        .into_iter()
        .map(|l| indent_lines(&l, level))
        .collect();
    let mut body = match item {
        Item::BareDecl(b) => {
            let mut head = format!("{}{}", b.name, print_params_opt(&b.params));
            if let Some(t) = &b.return_ty {
                head.push_str(&format!(" -> {}", print_type(t)));
            }
            head.push(':');
            let mut v = vec![indent_lines(&head, level)];
            v.extend(print_block(&b.body, level + 1, cx));
            v
        }
        Item::Function(f) => print_function_decl(f, level, cx, true),
        Item::Struct(s) => {
            let mut v = vec![indent_lines(
                &format!("struct {}{}:", s.name, print_generics(&s.generic_params)),
                level,
            )];
            for f in &s.fields {
                v.push(indent_lines(&format!("{}: {}", f.name, print_type(&f.ty)), level + 1));
            }
            v
        }
        Item::Enum(e) => {
            let mut v = vec![indent_lines(
                &format!("enum {}{}:", e.name, print_generics(&e.generic_params)),
                level,
            )];
            for v2 in &e.variants {
                v.push(indent_lines(&print_variant(v2), level + 1));
            }
            v
        }
        Item::Trait(t) => {
            let mut v = vec![indent_lines(
                &format!("trait {}{}:", t.name, print_generics(&t.generic_params)),
                level,
            )];
            for m in &t.members {
                v.push(indent_lines(&print_sig(m), level + 1));
            }
            v
        }
        Item::Impl(ib) => {
            let head = match &ib.for_trait {
                Some(tr) => format!("impl {} for {}:", print_type(tr), print_type(&ib.ty)),
                None => format!("impl {}:", print_type(&ib.ty)),
            };
            let mut v = vec![indent_lines(&head, level)];
            for m in &ib.methods {
                v.extend(print_function_decl(m, level + 1, cx, true));
            }
            v
        }
        Item::Const(c) => vec![indent_lines(
            &format!("const {}: {} = {}", c.name, print_type(&c.ty), print_expr(&c.value)),
            level,
        )],
        Item::Mod(m) => {
            let mut v = vec![indent_lines(&format!("mod {}:", m.name), level)];
            v.extend(print_stmts(&m.items, level + 1, cx));
            v
        }
        Item::Export(inner) => {
            // Print the inner item at this level (preserving its
            // relative indentation), then prefix `export ` to its
            // first non-comment line. Leading comments stay above,
            // untouched.
            let mut v = print_item(inner, level, cx);
            for line in v.iter_mut() {
                let trimmed = line.trim_start().to_string();
                if trimmed.is_empty()
                    || trimmed.starts_with("//")
                    || trimmed.starts_with("/*")
                {
                    continue;
                }
                let indent_len = line.len() - trimmed.len();
                *line = format!("{}export {}", &line[..indent_len], trimmed);
                break;
            }
            v
        }
    };
    if let Some(trail) = cx.trailing_for(item_span(item).end) {
        if let Some(last) = body.last_mut() {
            last.push_str(&trail);
        }
    }
    lines.append(&mut body);
    lines
}

fn item_span(item: &Item) -> compiler::diagnostics::Span {
    match item {
        Item::BareDecl(b) => b.span.clone(),
        Item::Function(f) => f.span.clone(),
        Item::Struct(s) => s.span.clone(),
        Item::Enum(e) => e.span.clone(),
        Item::Trait(t) => t.span.clone(),
        Item::Impl(ib) => ib.span.clone(),
        Item::Const(c) => c.span.clone(),
        Item::Mod(m) => m.span.clone(),
        Item::Export(inner) => item_span(inner),
    }
}

fn print_generics(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|g| {
            if g.bounds.is_empty() {
                g.name.clone()
            } else {
                format!("{}: {}", g.name, g.bounds.join(" + "))
            }
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn print_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| {
            let mut s = format!("{}: {}", p.name, print_type(&p.ty));
            if let Some(d) = &p.default {
                s.push_str(&format!(" = {}", print_expr(d)));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_params_opt(params: &Option<Vec<Param>>) -> String {
    match params {
        None => String::new(),
        Some(ps) => format!("({})", print_params(ps)),
    }
}

fn print_sig(sig: &FunctionSig) -> String {
    let mut s = format!("fn {}({})", sig.name, print_params(&sig.params));
    if let Some(t) = &sig.return_ty {
        s.push_str(&format!(" -> {}", print_type(t)));
    }
    s
}

fn print_variant(v: &EnumVariant) -> String {
    if v.fields.is_empty() {
        // Empty parens are LOAD-BEARING: bare `Red` would reparse as a
        // binding (the L-001 trap), never a variant test.
        format!("{}()", v.name)
    } else {
        let parts: Vec<String> = v.fields.iter().map(print_type).collect();
        format!("{}({})", v.name, parts.join(", "))
    }
}

/// Function declarations. `with_fn` selects the explicit `fn`
/// spelling (top-level items, impl methods). Statement-level
/// functions print bare — with two load-bearing exceptions, both
/// caught by the equivalence gate during development:
/// - empty parameter lists print WITHOUT parens in bare position:
///   `name():` at statement level reparses as a trailing-block call
///   while `name:` stays a bare function;
/// - single-expression bodies keep `fn` (bare `name: expr` would
///   reparse as a `Decl` statement).
fn print_function_decl(f: &FunctionDecl, level: usize, cx: &mut Cx, with_fn: bool) -> Vec<String> {
    let expr_body = matches!(f.body, FunctionBody::Expr(_));
    let kw = if with_fn || expr_body { "fn " } else { "" };
    let parens = if !with_fn && !expr_body && f.params.is_empty() {
        String::new()
    } else {
        format!("({})", print_params(&f.params))
    };
    let mut head = format!("{kw}{}{}{parens}", f.name, print_generics(&f.generic_params));
    if let Some(t) = &f.return_ty {
        head.push_str(&format!(" -> {}", print_type(t)));
    }
    match &f.body {
        // Single-expression bodies stay inline (`f(x): x`) — block vs
        // inline is a real AST distinction, not a density choice.
        FunctionBody::Expr(e) => vec![indent_lines(&format!("{head}: {}", print_expr(e)), level)],
        FunctionBody::Block(b) => {
            let mut v = vec![indent_lines(&format!("{head}:"), level)];
            v.extend(print_block(b, level + 1, cx));
            v
        }
    }
}

// ── Blocks, statements, density ───────────────────────────────────────────────

/// One printed statement: its own lines plus join metadata. Joining
/// only ever merges whole single lines, so comments (which live on
/// their own lines) can never be orphaned — commented statements
/// simply opt out.
struct StmtLines {
    lines: Vec<String>,
    joinable: bool,
}

fn print_block(block: &Block, level: usize, cx: &mut Cx) -> Vec<String> {
    print_stmts(&block.stmts, level, cx)
}

fn print_stmts(stmts: &[Stmt], level: usize, cx: &mut Cx) -> Vec<String> {
    let mut rendered: Vec<StmtLines> = Vec::new();
    for stmt in stmts {
        rendered.push(print_stmt(stmt, level, cx));
    }
    // Density heuristic (D3/D4): join runs of tiny single-line
    // statements with `; ` while the joined line fits the cap.
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rendered.len() {
        if !rendered[i].joinable || rendered[i].lines.len() != 1 {
            out.extend(rendered[i].lines.drain(..));
            i += 1;
            continue;
        }
        let mut joined = rendered[i].lines[0].clone();
        let mut j = i + 1;
        while j < rendered.len() && rendered[j].joinable && rendered[j].lines.len() == 1 {
            let candidate = format!("{}; {}", joined, rendered[j].lines[0].trim_start());
            if candidate.len() > LINE_CAP {
                break;
            }
            joined = candidate;
            j += 1;
        }
        // A "run" of one is just the statement itself (no `;` emitted
        // unless the heuristic actually joined something).
        out.push(joined);
        i = j.max(i + 1);
    }
    out
}

fn print_stmt(stmt: &Stmt, level: usize, cx: &mut Cx) -> StmtLines {
    let pad = INDENT.repeat(level);
    let leading: Vec<String> = cx
        .leading_for(stmt_span(stmt).start)
        .into_iter()
        .map(|l| indent_lines(&l, level))
        .collect();
    let has_leading = !leading.is_empty();
    let mut lines = leading;
    // joinable starts true for the tiny kinds; structural statements
    // and anything spanning lines opt out below.
    let mut joinable = matches!(
        stmt,
        Stmt::Let(_) | Stmt::Var(_) | Stmt::State(_) | Stmt::Assign(_) | Stmt::Decl(_) | Stmt::Expr(_)
    ) && !has_leading;
    match stmt {
        Stmt::Let(l) => lines.push(format!(
            "{pad}let {}{} = {}",
            l.name,
            l.ty.as_ref().map(|t| format!(": {}", print_type(t))).unwrap_or_default(),
            print_expr(&l.value)
        )),
        Stmt::Var(v) => lines.push(format!(
            "{pad}var {}{} = {}",
            v.name,
            v.ty.as_ref().map(|t| format!(": {}", print_type(t))).unwrap_or_default(),
            print_expr(&v.value)
        )),
        // The parser discards `state` annotations (StateStmt has no
        // field for them) — printed without, AST-equal by construction.
        Stmt::State(s) => lines.push(format!("{pad}state {} = {}", s.name, print_expr(&s.value))),
        Stmt::Assign(a) => lines.push(format!(
            "{pad}{} {} {}",
            print_expr(&a.target),
            assign_op(a.op.clone()),
            print_expr(&a.value)
        )),
        Stmt::Expr(e) => {
            let expr_lines = print_expr_stmt(e, level, cx);
            if expr_lines.len() != 1 {
                joinable = false;
            }
            lines.extend(expr_lines);
        }
        Stmt::Return(r) => lines.push(match &r.value {
            Some(v) => format!("{pad}return {}", print_expr(v)),
            None => format!("{pad}return"),
        }),
        Stmt::Break(b) => lines.push(match &b.value {
            Some(v) => format!("{pad}break {}", print_expr(v)),
            None => format!("{pad}break"),
        }),
        Stmt::Continue(_) => lines.push(format!("{pad}continue")),
        Stmt::If(i) => {
            joinable = false;
            lines.push(format!("{pad}if {}:", print_expr(&i.condition)));
            lines.extend(print_block(&i.then_block, level + 1, cx));
            for (cond, blk) in &i.else_if_clauses {
                lines.push(format!("{pad}else if {}:", print_expr(cond)));
                lines.extend(print_block(blk, level + 1, cx));
            }
            if let Some(b) = &i.else_block {
                lines.push(format!("{pad}else:"));
                lines.extend(print_block(b, level + 1, cx));
            }
        }
        Stmt::While(w) => {
            joinable = false;
            lines.push(format!("{pad}while {}:", print_expr(&w.condition)));
            lines.extend(print_block(&w.body, level + 1, cx));
        }
        Stmt::Loop(l) => {
            joinable = false;
            lines.push(format!("{pad}loop:"));
            lines.extend(print_block(&l.body, level + 1, cx));
        }
        Stmt::For(f) => {
            joinable = false;
            lines.push(format!("{pad}for {} in {}:", f.binding, print_expr(&f.iterable)));
            lines.extend(print_block(&f.body, level + 1, cx));
        }
        Stmt::Match(m) => {
            joinable = false;
            lines.push(format!("{pad}match {}:", print_expr(&m.scrutinee)));
            for arm in &m.arms {
                lines.extend(print_arm(arm, level + 1, cx));
            }
        }
        Stmt::Function(f) => {
            joinable = false;
            for line in print_function_decl(f, level, cx, false) {
                lines.push(line);
            }
        }
        Stmt::Struct(s) => {
            joinable = false;
            lines.push(format!("{pad}struct {}:", s.name));
            let fpad = INDENT.repeat(level + 1);
            for f in &s.fields {
                lines.push(format!("{fpad}{}: {}", f.name, print_type(&f.ty)));
            }
        }
        Stmt::BareField(f) => lines.push(format!("{pad}{}: {}", f.name, print_type(&f.ty))),
        Stmt::Decl(d) => lines.push(format!("{pad}{}: {}", d.name, print_expr(&d.value))),
    }
    // Same-line trailing comment belongs to this statement (and opts
    // it out of joining — the comment must stay put).
    if let Some(trail) = cx.trailing_for(stmt_span(stmt).end) {
        if let Some(last) = lines.last_mut() {
            last.push_str(&trail);
        }
        joinable = false;
    }
    StmtLines { lines, joinable }
}

fn stmt_span(stmt: &Stmt) -> compiler::diagnostics::Span {
    match stmt {
        Stmt::Let(l) => l.span.clone(),
        Stmt::Var(v) => v.span.clone(),
        Stmt::State(s) => s.span.clone(),
        Stmt::Assign(a) => a.span.clone(),
        Stmt::Expr(e) => expr_span(e),
        Stmt::Return(r) => r.span.clone(),
        Stmt::Break(b) => b.span.clone(),
        Stmt::Continue(s) => s.clone(),
        Stmt::If(i) => i.span.clone(),
        Stmt::While(w) => w.span.clone(),
        Stmt::Loop(l) => l.span.clone(),
        Stmt::For(f) => f.span.clone(),
        Stmt::Match(m) => m.span.clone(),
        Stmt::Function(f) => f.span.clone(),
        Stmt::Struct(s) => s.span.clone(),
        Stmt::BareField(f) => f.span.clone(),
        Stmt::Decl(d) => d.span.clone(),
    }
}

fn expr_span(e: &Expr) -> compiler::diagnostics::Span {
    match e {
        Expr::Literal(_, s) | Expr::Ident(_, s) => s.clone(),
        Expr::Call(c) => c.span.clone(),
        Expr::Member(m) => m.span.clone(),
        Expr::Index(i) => i.span.clone(),
        Expr::BinOp(b) => b.span.clone(),
        Expr::UnaryOp(u) => u.span.clone(),
        Expr::Try(t) => t.span.clone(),
        Expr::Range(r) => r.span.clone(),
        Expr::StringInterp(s) => s.span.clone(),
        Expr::StructLit(s) => s.span.clone(),
        Expr::ListLit(l) => l.span.clone(),
        Expr::Closure(c) => c.span.clone(),
        Expr::Spread(s) => s.span.clone(),
    }
}

fn assign_op(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Eq => "=",
        AssignOp::PlusEq => "+=",
        AssignOp::MinusEq => "-=",
        AssignOp::StarEq => "*=",
        AssignOp::SlashEq => "/=",
        AssignOp::PercentEq => "%=",
    }
}

/// Match arms at `level` (already the arm indent): `pat[ if g]:`
/// plus block lines, or `pat[ if g]: expr` inline. Block-vs-inline
/// is AST-real (MatchBody), never a density choice.
fn print_arm(arm: &MatchArm, level: usize, cx: &mut Cx) -> Vec<String> {
    let pad = INDENT.repeat(level);
    let mut head = print_pattern(&arm.pattern);
    if let Some(g) = &arm.guard {
        head.push_str(&format!(" if {}", print_expr(g)));
    }
    let mut lines: Vec<String> = cx
        .leading_for(arm.span.start)
        .into_iter()
        .map(|l| indent_lines(&l, level))
        .collect();
    match &arm.body {
        MatchBody::Expr(e) => lines.push(format!("{pad}{head}: {}", print_expr(e))),
        MatchBody::Block(b) => {
            lines.push(format!("{pad}{head}:"));
            lines.extend(print_block(b, level + 1, cx));
        }
    }
    if let Some(trail) = cx.trailing_for(arm.span.end) {
        if let Some(last) = lines.last_mut() {
            last.push_str(&trail);
        }
    }
    lines
}

fn print_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(_) => "_".to_string(),
        Pattern::Ident(name, _) => name.clone(),
        Pattern::Literal(lit, _) => print_literal(lit),
        // Empty parens are LOAD-BEARING: bare `Red` would reparse as a
        // binding (the L-001 trap), never a variant test.
        Pattern::Variant(name, subs, _) => {
            if subs.is_empty() {
                format!("{name}()")
            } else {
                let parts: Vec<String> = subs.iter().map(print_pattern).collect();
                format!("{name}({})", parts.join(", "))
            }
        }
    }
}

// ── Expressions ───────────────────────────────────────────────────────────────

/// An expression statement: a trailing-block call prints its head +
/// block; everything else is one line. Lines arrive fully indented.
fn print_expr_stmt(e: &Expr, level: usize, cx: &mut Cx) -> Vec<String> {
    let pad = INDENT.repeat(level);
    match e {
        Expr::Call(c) if c.trailing_block.is_some() => {
            let mut v = vec![format!("{pad}{}:", print_call_head(c, level))];
            v.extend(print_block(c.trailing_block.as_ref().unwrap(), level + 1, cx));
            v
        }
        _ => vec![format!("{pad}{}", print_expr(e))],
    }
}

/// Precedence classes (SYNTAX §9, loosest last). Used only to decide
/// parentheses — never to change meaning.
fn bin_prec(op: &BinOp) -> u8 {
    match op {
        BinOp::Coalesce => 10,
        BinOp::Or => 9,
        BinOp::And => 8,
        BinOp::Eq | BinOp::Ne => 7,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 6,
        BinOp::Range | BinOp::RangeInclusive => 5,
        BinOp::Add | BinOp::Sub => 4,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 3,
    }
}

fn bin_op_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::Range => "..",
        BinOp::RangeInclusive => "..=",
        BinOp::Coalesce => "??",
    }
}

/// Self-delimited in almost every position: literals, idents,
/// member/index chains, plain calls, struct/list literals,
/// interpolation. Everything else parenthesizes in operand, base,
/// callee, and range-end positions.
fn is_atom(e: &Expr) -> bool {
    match e {
        Expr::Literal(_, _)
        | Expr::Ident(_, _)
        | Expr::Member(_)
        | Expr::Index(_)
        | Expr::StructLit(_)
        | Expr::ListLit(_)
        | Expr::StringInterp(_) => true,
        Expr::Call(c) => c.trailing_block.is_none(),
        _ => false,
    }
}

/// Render one side of a binary operator: parenthesize a strictly
/// looser child, or a same-looseness child on the right (all
/// operators are left-associative); ranges and closures always
/// parenthesize (non-associativity and greedy-body hazards);
/// unary/`try` bind tightest and never need them as children.
/// Trailing-block calls cannot occur here per the grammar — meeting
/// one panics loud via `print_expr`, never prints garbage.
fn print_bin_child(e: &Expr, parent_prec: u8, is_right: bool) -> String {
    let need = match e {
        Expr::BinOp(inner) => {
            let p = bin_prec(&inner.op);
            p > parent_prec || (is_right && p == parent_prec)
        }
        Expr::Range(_) | Expr::Closure(_) => true,
        _ if is_atom(e) => false,
        Expr::UnaryOp(_) | Expr::Try(_) => false,
        _ => true,
    };
    if need {
        format!("({})", print_expr(e))
    } else {
        print_expr(e)
    }
}

/// Operand positions outside binary operators (unary operand, `?`
/// operand, range ends, member/index bases, callees): atoms print
/// bare, as does a nested `try` (postfix binds tightest, so
/// `await x?` needs no parens); everything else — including nested
/// unary (`-(-x)`, never `- -x`) — parenthesizes.
fn print_operand(e: &Expr) -> String {
    match e {
        _ if is_atom(e) => print_expr(e),
        Expr::Try(_) => print_expr(e),
        _ => format!("({})", print_expr(e)),
    }
}

fn print_expr(e: &Expr) -> String {
    match e {
        Expr::Literal(lit, _) => print_literal(lit),
        Expr::Ident(name, _) => name.clone(),
        Expr::Call(c) => {
            // Trailing-block calls only exist as direct statements
            // (the grammar never produces them in value position);
            // meeting one here is refused loud rather than printed
            // into an unparseable line.
            if c.trailing_block.is_some() {
                panic!("v2 cannot place a trailing-block call in value position");
            }
            print_call_head(c, 0)
        }
        Expr::Member(m) => {
            let base = if is_atom(&m.object) {
                print_expr(&m.object)
            } else {
                print_operand(&m.object)
            };
            format!("{base}.{}", m.field)
        }
        Expr::Index(i) => {
            let base = if is_atom(&i.object) {
                print_expr(&i.object)
            } else {
                print_operand(&i.object)
            };
            format!("{base}[{}]", print_expr(&i.index))
        }
        Expr::BinOp(b) => {
            let op = bin_op_str(&b.op);
            let prec = bin_prec(&b.op);
            let left = print_bin_child(&b.left, prec, false);
            let right = print_bin_child(&b.right, prec, true);
            format!("{left} {op} {right}")
        }
        Expr::UnaryOp(u) => {
            let op = match &u.op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
                UnaryOp::Await => "await ",
            };
            format!("{op}{}", print_operand(&u.operand))
        }
        Expr::Try(t) => format!("{}?", print_operand(&t.expr)),
        Expr::Range(r) => {
            let op = if r.inclusive { "..=" } else { ".." };
            format!("{}{op}{}", print_operand(&r.start), print_operand(&r.end))
        }
        Expr::StringInterp(s) => {
            let mut out = String::from("\"");
            for part in &s.parts {
                match part {
                    InterpPart::Literal(text) => out.push_str(&escape_double(text, true)),
                    InterpPart::Expr(e) => {
                        out.push('{');
                        out.push_str(&print_expr(e));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        Expr::StructLit(s) => print_struct_lit(s),
        Expr::ListLit(l) => {
            let elems: Vec<String> = l.elements.iter().map(print_expr).collect();
            print_broken("[", "]", &elems, ",")
        }
        Expr::Closure(c) => {
            let params = c.params.join(", ");
            format!("|{params}| {}", print_expr(&c.body))
        }
        Expr::Spread(s) => format!("...{}", print_expr(&s.expr)),
    }
}

// ── Calls, struct literals, breaking ──────────────────────────────────────────

/// Call head without trailing block (`f(a, label: b)`). Breaks past
/// the cap (SYNTAX §7 works both directions); `level` feeds only the
/// width estimate (nested calls approximate — deterministic, so the
/// idempotency gate still holds exactly).
fn print_call_head(c: &CallExpr, level: usize) -> String {
    let callee = if is_atom(&c.callee) {
        print_expr(&c.callee)
    } else {
        print_operand(&c.callee)
    };
    let args: Vec<String> = c
        .args
        .iter()
        .map(|a| match &a.label {
            Some(l) => format!("{l}: {}", print_expr(&a.value)),
            None => print_expr(&a.value),
        })
        .collect();
    let one_line = format!("{callee}({})", args.join(", "));
    if level * INDENT.len() + one_line.len() <= LINE_CAP {
        return one_line;
    }
    // Broken form: one argument per line, trailing comma (accepted by
    // the argument-list grammar — the equivalence gate proves it).
    let pad = INDENT.repeat(level + 1);
    let mut out = format!("{callee}(\n");
    for a in &args {
        out.push_str(&pad);
        out.push_str(a);
        out.push_str(",\n");
    }
    out.push_str(&INDENT.repeat(level));
    out.push(')');
    out
}

fn print_struct_lit(s: &StructLitExpr) -> String {
    if s.fields.is_empty() {
        return format!("{} {{}}", s.name);
    }
    let fields: Vec<String> = s
        .fields
        .iter()
        .map(|f| match f {
            StructField::Named(name, v) => format!("{name}: {}", print_expr(v)),
            StructField::Spread(e) => format!("...{}", print_expr(e)),
        })
        .collect();
    let one_line = format!("{} {{ {} }}", s.name, fields.join(", "));
    if one_line.len() <= LINE_CAP {
        return one_line;
    }
    let mut out = format!("{} {{\n", s.name);
    for f in &fields {
        out.push_str(INDENT);
        out.push_str(f);
        out.push_str(",\n");
    }
    out.push('}');
    out
}

/// `[`/`]`-style breaking shared by list literals: single line when
/// it fits, one element per line with trailing commas otherwise.
fn print_broken(open: &str, close: &str, elems: &[String], sep: &str) -> String {
    if elems.is_empty() {
        return format!("{open}{close}");
    }
    let one_line = format!("{open}{}{close}", elems.join(&format!("{sep} ")));
    if one_line.len() <= LINE_CAP {
        return one_line;
    }
    let mut out = format!("{open}\n");
    for e in elems {
        out.push_str(INDENT);
        out.push_str(e);
        out.push_str(sep);
        out.push('\n');
    }
    out.push_str(close);
    out
}

fn print_import(import: &ImportDecl) -> String {
    match &import.alias {
        Some(a) => format!("import {} as {a}", import.path.join("::")),
        None => format!("import {}", import.path.join("::")),
    }
}

fn print_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(name, args, _) => {
            if args.is_empty() {
                name.clone()
            } else {
                let parts: Vec<String> = args.iter().map(print_type).collect();
                format!("{name}<{}>", parts.join(", "))
            }
        }
        TypeExpr::Tuple(elems, _) => {
            let parts: Vec<String> = elems.iter().map(print_type).collect();
            format!("({})", parts.join(", "))
        }
        TypeExpr::Collection(inner, _) => format!("[{}]", print_type(inner)),
        TypeExpr::Function(params, ret, _) => {
            let parts: Vec<String> = params.iter().map(print_type).collect();
            format!("({}) -> {}", parts.join(", "), print_type(ret))
        }
    }
}

// ── Literals (reparse-stable spellings) ───────────────────────────────────────

fn print_literal(lit: &Literal) -> String {
    match lit {
        // Decimal always (hex/octal/binary spellings are values, and
        // the canonical form is decimal).
        Literal::Int(n) => n.to_string(),
        // Debug guarantees a reparseable float (`1.0` never `1`).
        Literal::Float(f) => format!("{f:?}"),
        Literal::Bool(b) => b.to_string(),
        Literal::Char(c) => format!("'{}'", escape_char(*c)),
        Literal::String(s) => format!("\"{}\"", escape_double(s, false)),
    }
}

/// Escape for `"` strings; `interp` additionally escapes braces
/// (`\{`/`\}` — the lexer's brace escapes, NOT doubled braces).
fn escape_double(s: &str, interp: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\0' => out.push_str("\\0"),
            '{' if interp => out.push_str("\\{"),
            '}' if interp => out.push_str("\\}"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn escape_char(c: char) -> String {
    match c {
        '\n' => "\\n".to_string(),
        '\t' => "\\t".to_string(),
        '\r' => "\\r".to_string(),
        '\\' => "\\\\".to_string(),
        '\'' => "\\'".to_string(),
        '\0' => "\\0".to_string(),
        c if (c as u32) < 0x20 => format!("\\u{{{:x}}}", c as u32),
        c => c.to_string(),
    }
}

// ── AST equivalence (spans ignored) ───────────────────────────────────────────
// The v2 gate: the output must reparse to the same tree. Spans always
// differ (bytes moved), so every node compares semantic content only.
// Floats compare with `==` (no NaN literal exists in the surface
// syntax, so the NaN caveat cannot trigger from real parses).

fn ast_eq_program(a: &Program, b: &Program) -> bool {
    a.imports.len() == b.imports.len()
        && a.imports.iter().zip(&b.imports).all(|(x, y)| {
            x.path == y.path && x.alias == y.alias
        })
        && a.items.len() == b.items.len()
        && a.items.iter().zip(&b.items).all(|(x, y)| ast_eq_item(x, y))
}

fn ast_eq_item(a: &Item, b: &Item) -> bool {
    match (a, b) {
        (Item::BareDecl(x), Item::BareDecl(y)) => {
            x.name == y.name
                && ast_eq_params_opt(&x.params, &y.params)
                && ast_eq_type_opt(&x.return_ty, &y.return_ty)
                && ast_eq_block(&x.body, &y.body)
        }
        (Item::Function(x), Item::Function(y)) => ast_eq_function(x, y),
        (Item::Struct(x), Item::Struct(y)) => {
            x.name == y.name
                && ast_eq_generics(&x.generic_params, &y.generic_params)
                && x.fields.len() == y.fields.len()
                && x.fields.iter().zip(&y.fields).all(|(f, g)| {
                    f.name == g.name && ast_eq_type(&f.ty, &g.ty)
                })
        }
        (Item::Enum(x), Item::Enum(y)) => {
            x.name == y.name
                && ast_eq_generics(&x.generic_params, &y.generic_params)
                && x.variants.len() == y.variants.len()
                && x.variants.iter().zip(&y.variants).all(|(v, w)| {
                    v.name == w.name
                        && v.fields.len() == w.fields.len()
                        && v.fields.iter().zip(&w.fields).all(|(s, t)| ast_eq_type(s, t))
                })
        }
        (Item::Trait(x), Item::Trait(y)) => {
            x.name == y.name
                && ast_eq_generics(&x.generic_params, &y.generic_params)
                && x.members.len() == y.members.len()
                && x.members.iter().zip(&y.members).all(|(m, n)| ast_eq_sig(m, n))
        }
        (Item::Impl(x), Item::Impl(y)) => {
            ast_eq_type(&x.ty, &y.ty)
                && ast_eq_type_opt(&x.for_trait, &y.for_trait)
                && x.methods.len() == y.methods.len()
                && x.methods.iter().zip(&y.methods).all(|(m, n)| ast_eq_function(m, n))
        }
        (Item::Const(x), Item::Const(y)) => {
            x.name == y.name && ast_eq_type(&x.ty, &y.ty) && ast_eq_expr(&x.value, &y.value)
        }
        (Item::Mod(x), Item::Mod(y)) => {
            x.name == y.name
                && x.items.len() == y.items.len()
                && x.items.iter().zip(&y.items).all(|(m, n)| ast_eq_stmt(m, n))
        }
        (Item::Export(x), Item::Export(y)) => ast_eq_item(x, y),
        _ => false,
    }
}

fn ast_eq_function(a: &FunctionDecl, b: &FunctionDecl) -> bool {
    a.name == b.name
        && ast_eq_generics(&a.generic_params, &b.generic_params)
        && ast_eq_params(&a.params, &b.params)
        && ast_eq_type_opt(&a.return_ty, &b.return_ty)
        && match (&a.body, &b.body) {
            (FunctionBody::Block(x), FunctionBody::Block(y)) => ast_eq_block(x, y),
            (FunctionBody::Expr(x), FunctionBody::Expr(y)) => ast_eq_expr(x, y),
            _ => false,
        }
}

fn ast_eq_sig(a: &FunctionSig, b: &FunctionSig) -> bool {
    a.name == b.name
        && ast_eq_generics(&a.generic_params, &b.generic_params)
        && ast_eq_params(&a.params, &b.params)
        && ast_eq_type_opt(&a.return_ty, &b.return_ty)
}

fn ast_eq_generics(a: &[GenericParam], b: &[GenericParam]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| x.name == y.name && x.bounds == y.bounds)
}

fn ast_eq_params(a: &[Param], b: &[Param]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.name == y.name
                && ast_eq_type(&x.ty, &y.ty)
                && match (&x.default, &y.default) {
                    (None, None) => true,
                    (Some(p), Some(q)) => ast_eq_expr(p, q),
                    _ => false,
                }
        })
}

fn ast_eq_params_opt(a: &Option<Vec<Param>>, b: &Option<Vec<Param>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => ast_eq_params(x, y),
        _ => false,
    }
}

fn ast_eq_block(a: &Block, b: &Block) -> bool {
    a.stmts.len() == b.stmts.len() && a.stmts.iter().zip(&b.stmts).all(|(x, y)| ast_eq_stmt(x, y))
}

fn ast_eq_stmt(a: &Stmt, b: &Stmt) -> bool {
    match (a, b) {
        (Stmt::Let(x), Stmt::Let(y)) => {
            x.name == y.name
                && ast_eq_type_opt(&x.ty, &y.ty)
                && ast_eq_expr(&x.value, &y.value)
        }
        (Stmt::Var(x), Stmt::Var(y)) => {
            x.name == y.name
                && ast_eq_type_opt(&x.ty, &y.ty)
                && ast_eq_expr(&x.value, &y.value)
        }
        (Stmt::State(x), Stmt::State(y)) => x.name == y.name && ast_eq_expr(&x.value, &y.value),
        (Stmt::Assign(x), Stmt::Assign(y)) => {
            x.op == y.op && ast_eq_expr(&x.target, &y.target) && ast_eq_expr(&x.value, &y.value)
        }
        (Stmt::Expr(x), Stmt::Expr(y)) => ast_eq_expr(x, y),
        (Stmt::Return(x), Stmt::Return(y)) => match (&x.value, &y.value) {
            (None, None) => true,
            (Some(p), Some(q)) => ast_eq_expr(p, q),
            _ => false,
        },
        (Stmt::Break(x), Stmt::Break(y)) => match (&x.value, &y.value) {
            (None, None) => true,
            (Some(p), Some(q)) => ast_eq_expr(p, q),
            _ => false,
        },
        (Stmt::Continue(_), Stmt::Continue(_)) => true,
        (Stmt::If(x), Stmt::If(y)) => {
            ast_eq_expr(&x.condition, &y.condition)
                && ast_eq_block(&x.then_block, &y.then_block)
                && x.else_if_clauses.len() == y.else_if_clauses.len()
                && x.else_if_clauses
                    .iter()
                    .zip(&y.else_if_clauses)
                    .all(|((c1, b1), (c2, b2))| ast_eq_expr(c1, c2) && ast_eq_block(b1, b2))
                && match (&x.else_block, &y.else_block) {
                    (None, None) => true,
                    (Some(p), Some(q)) => ast_eq_block(p, q),
                    _ => false,
                }
        }
        (Stmt::While(x), Stmt::While(y)) => {
            ast_eq_expr(&x.condition, &y.condition) && ast_eq_block(&x.body, &y.body)
        }
        (Stmt::Loop(x), Stmt::Loop(y)) => ast_eq_block(&x.body, &y.body),
        (Stmt::For(x), Stmt::For(y)) => {
            x.binding == y.binding
                && ast_eq_expr(&x.iterable, &y.iterable)
                && ast_eq_block(&x.body, &y.body)
        }
        (Stmt::Match(x), Stmt::Match(y)) => {
            ast_eq_expr(&x.scrutinee, &y.scrutinee)
                && x.arms.len() == y.arms.len()
                && x.arms.iter().zip(&y.arms).all(|(p, q)| {
                    ast_eq_pattern(&p.pattern, &q.pattern)
                        && match (&p.guard, &q.guard) {
                            (None, None) => true,
                            (Some(u), Some(v)) => ast_eq_expr(u, v),
                            _ => false,
                        }
                        && match (&p.body, &q.body) {
                            (MatchBody::Block(u), MatchBody::Block(v)) => ast_eq_block(u, v),
                            (MatchBody::Expr(u), MatchBody::Expr(v)) => ast_eq_expr(u, v),
                            _ => false,
                        }
                })
        }
        (Stmt::Function(x), Stmt::Function(y)) => ast_eq_function(x, y),
        (Stmt::Struct(x), Stmt::Struct(y)) => ast_eq_item(&Item::Struct(x.clone()), &Item::Struct(y.clone())),
        (Stmt::BareField(x), Stmt::BareField(y)) => {
            x.name == y.name && ast_eq_type(&x.ty, &y.ty)
        }
        (Stmt::Decl(x), Stmt::Decl(y)) => x.name == y.name && ast_eq_expr(&x.value, &y.value),
        _ => false,
    }
}

fn ast_eq_pattern(a: &Pattern, b: &Pattern) -> bool {
    match (a, b) {
        (Pattern::Wildcard(_), Pattern::Wildcard(_)) => true,
        (Pattern::Ident(x, _), Pattern::Ident(y, _)) => x == y,
        (Pattern::Literal(x, _), Pattern::Literal(y, _)) => ast_eq_literal(x, y),
        (Pattern::Variant(x, xs, _), Pattern::Variant(y, ys, _)) => {
            x == y && xs.len() == ys.len() && xs.iter().zip(ys).all(|(p, q)| ast_eq_pattern(p, q))
        }
        _ => false,
    }
}

fn ast_eq_literal(a: &Literal, b: &Literal) -> bool {
    match (a, b) {
        (Literal::Int(x), Literal::Int(y)) => x == y,
        (Literal::Float(x), Literal::Float(y)) => x == y,
        (Literal::Bool(x), Literal::Bool(y)) => x == y,
        (Literal::Char(x), Literal::Char(y)) => x == y,
        (Literal::String(x), Literal::String(y)) => x == y,
        _ => false,
    }
}

fn ast_eq_type(a: &TypeExpr, b: &TypeExpr) -> bool {
    match (a, b) {
        (TypeExpr::Named(x, xs, _), TypeExpr::Named(y, ys, _)) => {
            x == y && xs.len() == ys.len() && xs.iter().zip(ys).all(|(s, t)| ast_eq_type(s, t))
        }
        (TypeExpr::Tuple(x, _), TypeExpr::Tuple(y, _)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(s, t)| ast_eq_type(s, t))
        }
        (TypeExpr::Collection(x, _), TypeExpr::Collection(y, _)) => ast_eq_type(x, y),
        (TypeExpr::Function(xp, xr, _), TypeExpr::Function(yp, yr, _)) => {
            xp.len() == yp.len()
                && xp.iter().zip(yp).all(|(s, t)| ast_eq_type(s, t))
                && ast_eq_type(xr, yr)
        }
        _ => false,
    }
}

fn ast_eq_type_opt(a: &Option<TypeExpr>, b: &Option<TypeExpr>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => ast_eq_type(x, y),
        _ => false,
    }
}

fn ast_eq_expr(a: &Expr, b: &Expr) -> bool {    match (a, b) {
        (Expr::Literal(x, _), Expr::Literal(y, _)) => ast_eq_literal(x, y),
        (Expr::Ident(x, _), Expr::Ident(y, _)) => x == y,
        (Expr::Call(x), Expr::Call(y)) => {
            ast_eq_expr(&x.callee, &y.callee)
                && x.args.len() == y.args.len()
                && x.args.iter().zip(&y.args).all(|(p, q)| {
                    p.label == q.label && ast_eq_expr(&p.value, &q.value)
                })
                && match (&x.trailing_block, &y.trailing_block) {
                    (None, None) => true,
                    (Some(p), Some(q)) => ast_eq_block(p, q),
                    _ => false,
                }
        }
        (Expr::Member(x), Expr::Member(y)) => {
            x.field == y.field && ast_eq_expr(&x.object, &y.object)
        }
        (Expr::Index(x), Expr::Index(y)) => {
            ast_eq_expr(&x.object, &y.object) && ast_eq_expr(&x.index, &y.index)
        }
        (Expr::BinOp(x), Expr::BinOp(y)) => {
            x.op == y.op && ast_eq_expr(&x.left, &y.left) && ast_eq_expr(&x.right, &y.right)
        }
        (Expr::UnaryOp(x), Expr::UnaryOp(y)) => {
            x.op == y.op && ast_eq_expr(&x.operand, &y.operand)
        }
        (Expr::Try(x), Expr::Try(y)) => ast_eq_expr(&x.expr, &y.expr),
        (Expr::Range(x), Expr::Range(y)) => {
            x.inclusive == y.inclusive
                && ast_eq_expr(&x.start, &y.start)
                && ast_eq_expr(&x.end, &y.end)
        }
        (Expr::StringInterp(x), Expr::StringInterp(y)) => {
            x.parts.len() == y.parts.len()
                && x.parts.iter().zip(&y.parts).all(|(p, q)| match (p, q) {
                    (InterpPart::Literal(s), InterpPart::Literal(t)) => s == t,
                    (InterpPart::Expr(s), InterpPart::Expr(t)) => ast_eq_expr(s, t),
                    _ => false,
                })
        }
        (Expr::StructLit(x), Expr::StructLit(y)) => {
            x.name == y.name
                && x.fields.len() == y.fields.len()
                && x.fields.iter().zip(&y.fields).all(|(p, q)| match (p, q) {
                    (StructField::Named(pn, pv), StructField::Named(qn, qv)) => {
                        pn == qn && ast_eq_expr(pv, qv)
                    }
                    (StructField::Spread(p), StructField::Spread(q)) => ast_eq_expr(p, q),
                    _ => false,
                })
        }
        (Expr::ListLit(x), Expr::ListLit(y)) => {
            x.elements.len() == y.elements.len()
                && x.elements.iter().zip(&y.elements).all(|(p, q)| ast_eq_expr(p, q))
        }
        (Expr::Closure(x), Expr::Closure(y)) => {
            x.params == y.params && ast_eq_expr(&x.body, &y.body)
        }
        (Expr::Spread(x), Expr::Spread(y)) => ast_eq_expr(&x.expr, &y.expr),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gates hold on a representative snippet (the CLI suite covers
    /// the corpus + goldens end to end).
    #[test]
    fn v2_gates_hold_on_match_sample() {
        let src = "fn f(o: Option<Int>) -> Int:\n    match o:\n        Some(v):\n            v\n        None:\n            0\n";
        let out = format_v2(src).expect("format");
        assert_eq!(out, src);
    }

    #[test]
    fn v2_refuses_tab_indent() {
        let err = format_v2("main():\n\tprintln(\"hi\")\n").unwrap_err();
        assert!(err.message.contains("tab"), "{}", err.message);
    }

    #[test]
    fn debug_dump() {
        for src in [
            "fn f() -> String:\n    \"hi\"\n",
            "export fn f() -> String:\n    \"hi\"\n",
        ] {
            let (program, tokens, _) = parse_keep_tokens(src);
            let out = print_with_tokens(&program, &tokens, src).expect("print");
            println!("=== IN ===\n{src}=== OUT ===\n{out}=== END ===");
        }
    }
}
