//! Recursive-descent grammar productions for the Noctivue parser.
//!
//! Implements the EBNF from SYNTAX.md §10.
//!
//! ## Three-rule parsing strategy (COMPILER_ARCHITECTURE.md §4)
//!
//! 1. **Reserved control-flow keyword** at head of statement → parse that construct.
//! 2. **Explicit declaration keyword** (`fn`, `struct`, `enum`, …) → parse directly.
//! 3. **Everything else** → bare `Identifier [(params)] [-> Type] :` → `BareDecl`.
//!
//! The parser NEVER peeks at identifier casing to make a decision.

use crate::ast::*;
use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};
use crate::lexer::{Spanned, Token};
use std::collections::HashSet;

// ─────────────────────────────────────────────────────────────────────────────
// Parser state
// ─────────────────────────────────────────────────────────────────────────────

pub struct Parser<'a> {
    tokens: &'a [Spanned<Token>],
    pos: usize,
    pub sink: &'a mut DiagnosticSink,
    known_types: HashSet<String>,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Spanned<Token>], sink: &'a mut DiagnosticSink, known_types: HashSet<String>) -> Self {
        Parser { tokens, pos: 0, sink, known_types }
    }

    // ── Token navigation ──────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .map(|s| &s.node)
            .unwrap_or(&Token::Eof)
    }

    fn peek_spanned(&self) -> &Spanned<Token> {
        static EOF: Spanned<Token> = Spanned { node: Token::Eof, span: Span { start: 0, end: 0 } };
        self.tokens.get(self.pos).unwrap_or(&EOF)
    }

    fn peek2(&self) -> &Token {
        self.tokens
            .get(self.pos + 1)
            .map(|s| &s.node)
            .unwrap_or(&Token::Eof)
    }

    fn peek3(&self) -> &Token {
        self.tokens
            .get(self.pos + 2)
            .map(|s| &s.node)
            .unwrap_or(&Token::Eof)
    }

    fn peek4(&self) -> &Token {
        self.tokens
            .get(self.pos + 3)
            .map(|s| &s.node)
            .unwrap_or(&Token::Eof)
    }

    fn current_span(&self) -> Span {
        self.peek_spanned().span.clone()
    }

    fn advance(&mut self) -> &Spanned<Token> {
        let s = self.tokens.get(self.pos).unwrap_or_else(|| {
            static EOF: Spanned<Token> =
                Spanned { node: Token::Eof, span: Span { start: 0, end: 0 } };
            &EOF
        });
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        s
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token) -> Option<Span> {
        if self.peek() == expected {
            let span = self.current_span();
            self.advance();
            Some(span)
        } else {
            let span = self.current_span();
            self.sink.emit(
                Diagnostic::error(format!(
                    "expected `{expected:?}`, found `{:?}`",
                    self.peek()
                ))
                .with_span(span.clone(), "here")
                .with_code("E0100"),
            );
            None
        }
    }

    /// Skip newlines (statement separators between items).
    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    fn skip_newlines_and_semis(&mut self) {
        while matches!(self.peek(), Token::Newline | Token::Semi) {
            self.advance();
        }
    }

    /// Skip comment tokens and blank lines that may appear between items.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Token::Newline
                | Token::Semi
                | Token::LineComment(_)
                | Token::BlockComment(_)
                | Token::DocComment(_)
                | Token::ModDocComment(_)
                | Token::Dedent  // stray dedents from malformed blocks
                => { self.advance(); }
                _ => break,
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Top-level program
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_program(&mut self) -> Program {
        let mut imports = Vec::new();
        let mut items = Vec::new();

        self.skip_trivia();

        // Collect import declarations first.
        while *self.peek() == Token::Import {
            if let Some(imp) = self.parse_import() {
                imports.push(imp);
            }
            self.skip_trivia();
        }

        // Then top-level items.
        while *self.peek() != Token::Eof {
            self.skip_trivia();
            if *self.peek() == Token::Eof {
                break;
            }
            // Imports are accepted between declarations as well as in the
            // conventional header position. This matters when independent
            // module files are linked into one source-backed diagnostic unit.
            if *self.peek() == Token::Import {
                if let Some(imp) = self.parse_import() {
                    imports.push(imp);
                }
                self.skip_trivia();
                continue;
            }
            if let Some(item) = self.parse_top_decl() {
                items.push(item);
            } else {
                // Error recovery: skip one token and try again.
                self.advance();
            }
        }

        Program { imports, items }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Import declarations
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_import(&mut self) -> Option<ImportDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Import)?;

        let mut path = vec![self.parse_ident()?];
        while *self.peek() == Token::ColonColon {
            self.advance();
            path.push(self.parse_ident()?);
        }

        let alias = if *self.peek() == Token::As {
            self.advance();
            Some(self.parse_ident()?)
        } else {
            None
        };

        let end = self.current_span().end;
        self.skip_newlines();
        Some(ImportDecl { path, alias, span: Span { start, end } })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Top-level declaration dispatch
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_top_decl(&mut self) -> Option<Item> {
        match self.peek() {
            Token::Export => {
                let _span = self.current_span();
                self.advance();
                let inner = self.parse_top_decl()?;
                Some(Item::Export(Box::new(inner)))
            }
            Token::Fn => Some(Item::Function(self.parse_fn_decl()?)),
            Token::Struct => Some(Item::Struct(self.parse_struct_decl()?)),
            Token::Enum => Some(Item::Enum(self.parse_enum_decl()?)),
            Token::Trait => Some(Item::Trait(self.parse_trait_decl()?)),
            Token::Impl => Some(Item::Impl(self.parse_impl_block()?)),
            Token::Const => Some(Item::Const(self.parse_const_decl()?)),
            Token::Mod => Some(Item::Mod(self.parse_mod_decl()?)),
            // Doc/mod-doc comments: attach to next item (skip for now).
            Token::DocComment(_) | Token::ModDocComment(_) => {
                self.advance();
                self.parse_top_decl()
            }
            // Async function shorthand: `async fn` or `async name(...)`
            Token::Async => {
                // Skip `async` for now — treated as a modifier, parse what follows.
                self.advance();
                self.parse_top_decl()
            }
            Token::Ident(_) => {
                // Could be a BareDecl or a call-expression statement at top level.
                self.parse_bare_decl_or_expr_item()
            }
            _ => {
                let span = self.current_span();
                self.sink.emit(
                    Diagnostic::error(format!(
                        "unexpected token `{:?}` at top level",
                        self.peek()
                    ))
                    .with_span(span, "here")
                    .with_code("E0101"),
                );
                None
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Bare declaration / expression-item disambiguation (Rule 3)
    // ─────────────────────────────────────────────────────────────────────────

    /// Parses a bare `Identifier [(params)] [-> Type] :` block into a `BareDecl`,
    /// or a standalone expression-statement if no colon-block follows.
    fn parse_bare_decl_or_expr_item(&mut self) -> Option<Item> {
        let start = self.current_span().start;
        let name = self.parse_ident()?;

        // Optional generic params: `<T, U>`
        // (skip for now — will re-add in a later pass)

        // Optional parameter list: `(params)`
        let params = if *self.peek() == Token::LParen {
            Some(self.parse_param_list()?)
        } else {
            None
        };

        // Optional return type: `-> Type`
        let return_ty = if *self.peek() == Token::Arrow {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        // Must be followed by `:` for a declaration.
        if *self.peek() != Token::Colon {
            // Not a declaration — this was a bare expression statement at top level.
            // Reconstruct as an expression item (unusual at top level, but valid for
            // components in some positions). Error-recover.
            let span = Span { start, end: self.current_span().end };
            self.sink.emit(
                Diagnostic::error(format!(
                    "expected `:` after `{name}` to begin a declaration block"
                ))
                .with_span(span, "here")
                .with_code("E0102"),
            );
            self.skip_to_next_top_level();
            return None;
        }

        let body = self.parse_colon_block()?;
        let end = body.span.end;

        Some(Item::BareDecl(BareDecl {
            name,
            params,
            return_ty,
            body,
            span: Span { start, end },
        }))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Explicit declaration parsers
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_fn_decl(&mut self) -> Option<FunctionDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Fn)?;
        let name = self.parse_ident()?;
        let generic_params = self.parse_generic_params_opt();
        let params = if *self.peek() == Token::LParen {
            self.parse_param_list()?
        } else {
            vec![]
        };
        let return_ty = if *self.peek() == Token::Arrow {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let body = self.parse_function_body()?;
        let end = self.current_span().start;
        Some(FunctionDecl {
            name,
            generic_params,
            params,
            return_ty,
            body,
            span: Span { start, end },
        })
    }

    fn parse_struct_decl(&mut self) -> Option<StructDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Struct)?;
        let name = self.parse_ident()?;
        let generic_params = self.parse_generic_params_opt();
        self.expect(&Token::Colon)?;
        let fields = self.parse_field_block()?;
        let end = self.current_span().start;
        Some(StructDecl { name, generic_params, fields, span: Span { start, end } })
    }

    fn parse_enum_decl(&mut self) -> Option<EnumDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Enum)?;
        let name = self.parse_ident()?;
        let generic_params = self.parse_generic_params_opt();
        self.expect(&Token::Colon)?;
        let variants = self.parse_enum_body()?;
        let end = self.current_span().start;
        Some(EnumDecl { name, generic_params, variants, span: Span { start, end } })
    }

    fn parse_trait_decl(&mut self) -> Option<TraitDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Trait)?;
        let name = self.parse_ident()?;
        let generic_params = self.parse_generic_params_opt();
        self.expect(&Token::Colon)?;
        let members = self.parse_trait_body()?;
        let end = self.current_span().start;
        Some(TraitDecl { name, generic_params, members, span: Span { start, end } })
    }

    fn parse_impl_block(&mut self) -> Option<ImplBlock> {
        let start = self.current_span().start;
        self.expect(&Token::Impl)?;
        let ty = self.parse_type_expr()?;
        let for_trait = if *self.peek() == Token::For {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        // Swap: in `impl Trait for Type`, ty is trait and for_trait is the implementing type.
        let (impl_ty, trait_ty) = if for_trait.is_some() {
            (for_trait.unwrap(), Some(ty))
        } else {
            (ty, None)
        };
        self.expect(&Token::Colon)?;
        let methods = self.parse_impl_body()?;
        let end = self.current_span().start;
        Some(ImplBlock { ty: impl_ty, for_trait: trait_ty, methods, span: Span { start, end } })
    }

    fn parse_const_decl(&mut self) -> Option<ConstDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Const)?;
        let name = self.parse_ident()?;
        self.expect(&Token::Colon)?;
        let ty = self.parse_type_expr()?;
        self.expect(&Token::Eq)?;
        let value = self.parse_expr()?;
        let end = self.current_span().start;
        self.skip_newlines();
        Some(ConstDecl { name, ty, value, span: Span { start, end } })
    }

    fn parse_mod_decl(&mut self) -> Option<ModDecl> {
        let start = self.current_span().start;
        self.expect(&Token::Mod)?;
        let name = self.parse_ident()?;
        self.expect(&Token::Colon)?;
        let items = self.parse_colon_block()?.stmts;
        let end = self.current_span().start;
        Some(ModDecl { name, items, span: Span { start, end } })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Blocks
    // ─────────────────────────────────────────────────────────────────────────

    /// Parse a `:` followed by either an indented block, a braced block, or an
    /// inline single-statement block.
    fn parse_colon_block(&mut self) -> Option<Block> {
        let colon_span = self.current_span();
        self.expect(&Token::Colon)?;

        match self.peek() {
            // Braced block: `: { ... }`
            Token::LBrace => {
                let start = self.current_span().start;
                self.advance(); // `{`
                let stmts = self.parse_stmt_list_braced();
                let end = self.current_span().end;
                self.expect(&Token::RBrace);
                Some(Block { stmts, span: Span { start, end } })
            }
            // Newline → INDENT block
            Token::Newline => {
                self.advance(); // consume newline
                self.parse_indented_block()
            }
            // Inline single statement (compact form): `: expr` or `: stmt`
            _ => {
                let start = colon_span.start;
                let stmts = self.parse_inline_stmt_list();
                let end = self.current_span().start;
                Some(Block { stmts, span: Span { start, end } })
            }
        }
    }

    /// Parse `INDENT { stmt } DEDENT`.
    fn parse_indented_block(&mut self) -> Option<Block> {
        let start = self.current_span().start;
        if *self.peek() != Token::Indent {
            // Empty body — valid (e.g. empty struct, Phase 0 decision: → struct).
            return Some(Block { stmts: vec![], span: Span { start, end: start } });
        }
        self.advance(); // INDENT
        let stmts = self.parse_stmt_list_indented();
        let end = self.current_span().start;
        // Consume DEDENT if present.
        if *self.peek() == Token::Dedent {
            self.advance();
        }
        Some(Block { stmts, span: Span { start, end } })
    }

    fn parse_stmt_list_indented(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines_and_semis();
            match self.peek() {
                Token::Dedent | Token::Eof => break,
                _ => {
                    if let Some(s) = self.parse_stmt() {
                        stmts.push(s);
                    } else {
                        // Error recovery: skip to next newline.
                        while !matches!(self.peek(), Token::Newline | Token::Dedent | Token::Eof) {
                            self.advance();
                        }
                    }
                }
            }
        }
        stmts
    }

    fn parse_stmt_list_braced(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines_and_semis();
            match self.peek() {
                Token::RBrace | Token::Eof => break,
                _ => {
                    if let Some(s) = self.parse_stmt() {
                        stmts.push(s);
                    } else {
                        while !matches!(self.peek(), Token::Semi | Token::Newline | Token::RBrace | Token::Eof) {
                            self.advance();
                        }
                    }
                }
            }
        }
        stmts
    }

    /// Parse a semicolon-separated inline list of statements on one line.
    fn parse_inline_stmt_list(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines_and_semis();
            match self.peek() {
                Token::Newline | Token::Eof | Token::Dedent => break,
                Token::Semi => { self.advance(); }
                _ => {
                    if let Some(s) = self.parse_stmt() {
                        stmts.push(s);
                    } else {
                        break;
                    }
                    if *self.peek() == Token::Semi {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }
        stmts
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Struct field block
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_field_block(&mut self) -> Option<Vec<FieldDecl>> {
        match self.peek() {
            Token::LBrace => {
                self.advance();
                let fields = self.parse_field_list(true);
                self.expect(&Token::RBrace);
                Some(fields)
            }
            Token::Newline => {
                self.advance();
                if *self.peek() == Token::Indent {
                    self.advance(); // INDENT
                    let fields = self.parse_field_list(false);
                    if *self.peek() == Token::Dedent {
                        self.advance();
                    }
                    Some(fields)
                } else {
                    Some(vec![])
                }
            }
            _ => Some(vec![]),
        }
    }

    fn parse_field_list(&mut self, braced: bool) -> Vec<FieldDecl> {
        let mut fields = Vec::new();
        loop {
            self.skip_newlines_and_semis();
            let end_tok = if braced { Token::RBrace } else { Token::Dedent };
            if *self.peek() == end_tok || *self.peek() == Token::Eof {
                break;
            }
            if let Token::Ident(_) = self.peek() {
                let start = self.current_span().start;
                let name = match self.parse_ident() {
                    Some(n) => n,
                    None => break,
                };
                if self.expect(&Token::Colon).is_none() {
                    break;
                }
                let ty = match self.parse_type_expr() {
                    Some(t) => t,
                    None => break,
                };
                let end = self.current_span().start;
                self.skip_newlines();
                fields.push(FieldDecl { name, ty, span: Span { start, end } });
            } else {
                break;
            }
        }
        fields
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Enum body
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_enum_body(&mut self) -> Option<Vec<EnumVariant>> {
        let mut variants = Vec::new();
        match self.peek() {
            Token::Newline => {
                self.advance();
                if *self.peek() != Token::Indent {
                    return Some(variants);
                }
                self.advance(); // INDENT
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), Token::Dedent | Token::Eof) {
                        break;
                    }
                    if let Some(v) = self.parse_enum_variant() {
                        variants.push(v);
                    } else {
                        break;
                    }
                }
                if *self.peek() == Token::Dedent {
                    self.advance();
                }
            }
            Token::LBrace => {
                self.advance();
                loop {
                    self.skip_newlines_and_semis();
                    if matches!(self.peek(), Token::RBrace | Token::Eof) {
                        break;
                    }
                    if let Some(v) = self.parse_enum_variant() {
                        variants.push(v);
                    } else {
                        break;
                    }
                }
                self.expect(&Token::RBrace);
            }
            _ => {}
        }
        Some(variants)
    }

    fn parse_enum_variant(&mut self) -> Option<EnumVariant> {
        let start = self.current_span().start;
        let name = self.parse_ident()?;
        let fields = if *self.peek() == Token::LParen {
            self.advance();
            let mut types = Vec::new();
            loop {
                self.skip_newlines();
                if matches!(self.peek(), Token::RParen | Token::Eof) {
                    break;
                }
                // Named field in variant: `name: Type` — skip name if present.
                if let Token::Ident(_) = self.peek() {
                    if *self.peek2() == Token::Colon {
                        self.advance(); // name
                        self.advance(); // colon
                    }
                }
                if let Some(ty) = self.parse_type_expr() {
                    types.push(ty);
                }
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(&Token::RParen);
            types
        } else {
            vec![]
        };
        let end = self.current_span().start;
        self.skip_newlines();
        Some(EnumVariant { name, fields, span: Span { start, end } })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Trait body
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_trait_body(&mut self) -> Option<Vec<FunctionSig>> {
        let mut members = Vec::new();
        match self.peek() {
            Token::Newline => {
                self.advance();
                if *self.peek() != Token::Indent {
                    return Some(members);
                }
                self.advance();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), Token::Dedent | Token::Eof) {
                        break;
                    }
                    if let Some(sig) = self.parse_fn_sig() {
                        members.push(sig);
                    } else {
                        break;
                    }
                }
                if *self.peek() == Token::Dedent {
                    self.advance();
                }
            }
            _ => {}
        }
        Some(members)
    }

    fn parse_fn_sig(&mut self) -> Option<FunctionSig> {
        let start = self.current_span().start;
        let has_fn = self.eat(&Token::Fn);
        let _ = has_fn;
        let name = self.parse_ident()?;
        let generic_params = self.parse_generic_params_opt();
        let params = if *self.peek() == Token::LParen {
            self.parse_param_list()?
        } else {
            vec![]
        };
        let return_ty = if *self.peek() == Token::Arrow {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let end = self.current_span().start;
        self.skip_newlines();
        Some(FunctionSig { name, generic_params, params, return_ty, span: Span { start, end } })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Impl body
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_impl_body(&mut self) -> Option<Vec<FunctionDecl>> {
        let mut methods = Vec::new();
        match self.peek() {
            Token::Newline => {
                self.advance();
                if *self.peek() != Token::Indent {
                    return Some(methods);
                }
                self.advance();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), Token::Dedent | Token::Eof) {
                        break;
                    }
                    // Skip doc comments
                    if matches!(self.peek(), Token::DocComment(_)) {
                        self.advance();
                        continue;
                    }
                    if let Some(f) = self.parse_fn_decl() {
                        methods.push(f);
                    } else {
                        break;
                    }
                }
                if *self.peek() == Token::Dedent {
                    self.advance();
                }
            }
            _ => {}
        }
        Some(methods)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Function body
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_function_body(&mut self) -> Option<FunctionBody> {
        let colon_span = self.current_span();
        self.expect(&Token::Colon)?;

        match self.peek() {
            // Braced body
            Token::LBrace => {
                let start = self.current_span().start;
                self.advance();
                let stmts = self.parse_stmt_list_braced();
                let end = self.current_span().end;
                self.expect(&Token::RBrace);
                Some(FunctionBody::Block(Block { stmts, span: Span { start, end } }))
            }
            // Newline → indented block
            Token::Newline => {
                self.advance();
                let block = self.parse_indented_block()?;
                Some(FunctionBody::Block(block))
            }
            // Single-statement body on same line (can be an expr or a control-flow stmt)
            _ => {
                let start = self.current_span().start;
                let mut stmts = self.parse_inline_stmt_list();
                let end = self.current_span().start;
                let _ = colon_span;
                if stmts.len() == 1 {
                    if let Stmt::Expr(_) = &stmts[0] {
                        if let Stmt::Expr(e) = stmts.remove(0) {
                            return Some(FunctionBody::Expr(e));
                        }
                    }
                }
                Some(FunctionBody::Block(Block { stmts, span: Span { start, end } }))
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Parameter lists
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_param_list(&mut self) -> Option<Vec<Param>> {
        self.expect(&Token::LParen)?;
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RParen | Token::Eof) {
                break;
            }
            if let Some(p) = self.parse_param() {
                params.push(p);
            } else {
                break;
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Some(params)
    }

    fn parse_param(&mut self) -> Option<Param> {
        let start = self.current_span().start;
        let name = self.parse_ident()?;
        self.expect(&Token::Colon)?;
        let ty = self.parse_type_expr()?;
        let default = if *self.peek() == Token::Eq {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let end = self.current_span().start;
        Some(Param { name, ty, default, span: Span { start, end } })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Generic parameters
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_generic_params_opt(&mut self) -> Vec<GenericParam> {
        if *self.peek() != Token::Lt {
            return vec![];
        }
        self.advance(); // `<`
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::Gt | Token::Eof) {
                break;
            }
            let start = self.current_span().start;
            let name = match self.parse_ident() {
                Some(n) => n,
                None => break,
            };
            let bounds = if *self.peek() == Token::Colon {
                self.advance();
                let mut bs = vec![];
                loop {
                    match self.parse_ident() {
                        Some(b) => bs.push(b),
                        None => break,
                    }
                    if *self.peek() == Token::Plus {
                        self.advance();
                    } else {
                        break;
                    }
                }
                bs
            } else {
                vec![]
            };
            let end = self.current_span().start;
            params.push(GenericParam { name, bounds, span: Span { start, end } });
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::Gt);
        params
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Statements
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Option<Stmt> {
        self.skip_newlines_and_semis();
        match self.peek() {
            Token::Let   => self.parse_let_stmt().map(Stmt::Let),
            Token::Var   => self.parse_var_stmt().map(Stmt::Var),
            Token::Return => self.parse_return_stmt().map(Stmt::Return),
            Token::Break  => self.parse_break_stmt().map(Stmt::Break),
            Token::Continue => {
                let span = self.current_span();
                self.advance();
                self.skip_newlines();
                Some(Stmt::Continue(span))
            }
            Token::If    => self.parse_if_stmt().map(Stmt::If),
            Token::While => self.parse_while_stmt().map(Stmt::While),
            Token::Loop  => self.parse_loop_stmt().map(Stmt::Loop),
            Token::For   => self.parse_for_stmt().map(Stmt::For),
            Token::Match => self.parse_match_stmt().map(Stmt::Match),
            Token::Fn    => self.parse_fn_decl().map(|f| Stmt::Function(f)),
            Token::Struct => self.parse_struct_decl().map(|s| Stmt::Struct(s)),
            Token::DocComment(_) | Token::LineComment(_) | Token::BlockComment(_) => {
                self.advance();
                self.parse_stmt()
            }
            Token::Ident(s) if s == "state" => {
                self.parse_state_stmt().map(Stmt::State)
            }
            Token::Ident(_) => {
                if *self.peek2() == Token::LParen {
                    self.parse_ident_paren_stmt()
                } else if *self.peek2() == Token::Colon {
                    if self.is_field_decl_ahead() {
                        self.parse_bare_field_stmt()
                    } else if self.is_bare_decl_ahead_no_parens() {
                        self.parse_bare_decl_stmt()
                    } else {
                        self.parse_decl_stmt()
                    }
                } else {
                    self.parse_expr_or_assign_stmt()
                }
            }
            _ => self.parse_expr_or_assign_stmt(),
        }
    }

    fn parse_ident_paren_stmt(&mut self) -> Option<Stmt> {
        let start = self.current_span().start;

        // Scan forward past balanced parens to see what follows.
        let mut j = self.pos + 1;
        let mut depth = 0u32;
        let mut empty_parens = false;
        let mut looks_like_params = false;
        while j < self.tokens.len() {
            match &self.tokens[j].node {
                Token::LParen => depth += 1,
                Token::RParen => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Token::Colon if depth == 0 => {
                    let first = self.tokens.get(self.pos + 2).map(|s| &s.node);
                    let second = self.tokens.get(self.pos + 3).map(|s| &s.node);
                    empty_parens = j == self.pos + 2;
                    looks_like_params = matches!(first, Some(Token::Ident(_)))
                        && matches!(second, Some(Token::Colon));
                    break;
                }
                Token::Newline | Token::Semi | Token::Dedent | Token::Eof if depth == 0 => break,
                _ => {}
            }
            j += 1;
        }

        if empty_parens || looks_like_params {
            let name = self.parse_ident()?;
            let generic_params = Vec::new();
            let params = if *self.peek() == Token::LParen {
                self.parse_param_list()?
            } else {
                Vec::new()
            };
            let return_ty = if *self.peek() == Token::Arrow {
                self.advance();
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            let body = self.parse_colon_block()?;
            let end = body.span.end;
            let func = FunctionDecl {
                name,
                generic_params,
                params,
                return_ty,
                body: FunctionBody::Block(body),
                span: Span { start, end },
            };
            return Some(Stmt::Function(func));
        }

        self.parse_expr_or_assign_stmt()
    }

    /// Returns true if the next tokens look like `Ident : TypeStart`
    /// where TypeStart is a known type name, a tuple type `(`, or a
    /// collection type `[T]` whose first inner token is a type-like ident
    /// or nested paren. Call expressions and literals are excluded.
    fn is_field_decl_ahead(&self) -> bool {
        if !matches!(self.peek(), Token::Ident(_)) {
            return false;
        }
        if *self.peek2() != Token::Colon {
            return false;
        }
        match self.peek3() {
            Token::Ident(ref name) => self.known_types.contains(name.as_str()),
            Token::LParen => true,
            Token::LBracket => matches!(self.peek4(), Token::Ident(_) | Token::LParen),
            _ => false,
        }
    }

    /// Returns true if the next tokens look like a bare declaration without
    /// preceding parens: `identifier: block`. This is only checked when the
    /// identifier is directly followed by `:` (no `(` in between).
    fn is_bare_decl_ahead_no_parens(&self) -> bool {
        if !matches!(self.peek(), Token::Ident(_)) {
            return false;
        }
        if *self.peek2() != Token::Colon {
            return false;
        }
        let next = self.peek3();
        matches!(
            next,
            Token::Newline | Token::Indent | Token::LBrace | Token::Dedent | Token::Eof
        )
    }

    fn parse_bare_field_stmt(&mut self) -> Option<Stmt> {
        let start = self.current_span().start;
        let name = self.parse_ident()?;
        self.expect(&Token::Colon)?;
        let ty = self.parse_type_expr()?;
        let end = self.current_span().start;
        self.skip_newlines();
        Some(Stmt::BareField(FieldDecl { name, ty, span: Span { start, end } }))
    }

    fn parse_bare_decl_stmt(&mut self) -> Option<Stmt> {
        let start = self.current_span().start;
        let name = self.parse_ident()?;
        let generic_params = Vec::new();
        let params = if *self.peek() == Token::LParen {
            self.parse_param_list()?
        } else {
            Vec::new()
        };
        let return_ty = if *self.peek() == Token::Arrow {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let body = self.parse_colon_block()?;
        let end = body.span.end;
        let func = FunctionDecl {
            name,
            generic_params,
            params,
            return_ty,
            body: FunctionBody::Block(body),
            span: Span { start, end },
        };
        Some(Stmt::Function(func))
    }

    /// Parse a bare declaration `name: expr` without a type annotation.
    fn parse_decl_stmt(&mut self) -> Option<Stmt> {
        let start = self.current_span().start;
        let name = self.parse_ident()?;
        self.expect(&Token::Colon)?;
        let value = self.parse_expr()?;
        let end = self.current_span().start;
        self.skip_newlines();
        Some(Stmt::Decl(DeclStmt { name, value, span: Span { start, end } }))
    }

    fn parse_let_stmt(&mut self) -> Option<LetStmt> {
        let start = self.current_span().start;
        self.expect(&Token::Let)?;
        let name = self.parse_ident()?;
        let ty = if *self.peek() == Token::Colon {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(&Token::Eq)?;
        let value = self.parse_expr()?;
        let end = self.current_span().start;
        self.skip_newlines();
        Some(LetStmt { name, ty, value, span: Span { start, end } })
    }

    fn parse_var_stmt(&mut self) -> Option<VarStmt> {
        let start = self.current_span().start;
        self.expect(&Token::Var)?;
        let name = self.parse_ident()?;
        let ty = if *self.peek() == Token::Colon {
            self.advance();
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(&Token::Eq)?;
        let value = self.parse_expr()?;
        let end = self.current_span().start;
        self.skip_newlines();
        Some(VarStmt { name, ty, value, span: Span { start, end } })
    }

    fn parse_state_stmt(&mut self) -> Option<StateStmt> {
        let start = self.current_span().start;
        self.advance(); // consume `state` ident
        let name = self.parse_ident()?;
        // Optional type annotation
        if *self.peek() == Token::Colon {
            self.advance();
            self.parse_type_expr(); // consume but discard for now
        }
        self.expect(&Token::Eq)?;
        let value = self.parse_expr()?;
        let end = self.current_span().start;
        self.skip_newlines();
        Some(StateStmt { name, value, span: Span { start, end } })
    }

    fn parse_return_stmt(&mut self) -> Option<ReturnStmt> {
        let start = self.current_span().start;
        self.advance(); // `return`
        let value = if !matches!(self.peek(), Token::Newline | Token::Semi | Token::Eof | Token::Dedent) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let end = self.current_span().start;
        self.skip_newlines();
        Some(ReturnStmt { value, span: Span { start, end } })
    }

    fn parse_break_stmt(&mut self) -> Option<BreakStmt> {
        let start = self.current_span().start;
        self.advance(); // `break`
        let value = if !matches!(self.peek(), Token::Newline | Token::Semi | Token::Eof | Token::Dedent) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let end = self.current_span().start;
        self.skip_newlines();
        Some(BreakStmt { value, span: Span { start, end } })
    }

    fn parse_if_stmt(&mut self) -> Option<IfStmt> {
        let start = self.current_span().start;
        self.expect(&Token::If)?;

        // Handle `if let Some(x) = expr` pattern — consume `let` but simplify
        // to expression for now.
        if *self.peek() == Token::Let {
            self.advance(); // `let`
            // parse the pattern loosely as ident or Type(ident)
            self.parse_pattern(); // discard — we just need to consume it
            self.expect(&Token::Eq);
        }

        let condition = self.parse_expr()?;
        let then_block = self.parse_colon_block()?;

        let mut else_if_clauses = Vec::new();
        let mut else_block = None;

        loop {
            self.skip_newlines();
            if *self.peek() == Token::Else {
                self.advance();
                if *self.peek() == Token::If {
                    self.advance(); // `if`
                    let cond = self.parse_expr()?;
                    let blk = self.parse_colon_block()?;
                    else_if_clauses.push((cond, blk));
                } else {
                    else_block = Some(self.parse_colon_block()?);
                    break;
                }
            } else {
                break;
            }
        }

        let end = self.current_span().start;
        Some(IfStmt { condition, then_block, else_if_clauses, else_block, span: Span { start, end } })
    }

    fn parse_while_stmt(&mut self) -> Option<WhileStmt> {
        let start = self.current_span().start;
        self.expect(&Token::While)?;
        let condition = self.parse_expr()?;
        let body = self.parse_colon_block()?;
        let end = self.current_span().start;
        Some(WhileStmt { condition, body, span: Span { start, end } })
    }

    fn parse_loop_stmt(&mut self) -> Option<LoopStmt> {
        let start = self.current_span().start;
        self.expect(&Token::Loop)?;
        let body = self.parse_colon_block()?;
        let end = self.current_span().start;
        Some(LoopStmt { body, span: Span { start, end } })
    }

    fn parse_for_stmt(&mut self) -> Option<ForStmt> {
        let start = self.current_span().start;
        self.expect(&Token::For)?;
        let binding = self.parse_ident()?;
        self.expect(&Token::In)?;
        let iterable = self.parse_expr()?;
        let body = self.parse_colon_block()?;
        let end = self.current_span().start;
        Some(ForStmt { binding, iterable, body, span: Span { start, end } })
    }

    fn parse_match_stmt(&mut self) -> Option<MatchStmt> {
        let start = self.current_span().start;
        self.expect(&Token::Match)?;
        let scrutinee = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        self.skip_newlines();
        if *self.peek() != Token::Indent {
            let span = self.current_span();
            self.sink.emit(
                Diagnostic::error("expected indented block after `match`")
                    .with_span(span, "here")
                    .with_code("E0103"),
            );
            return None;
        }
        self.advance(); // INDENT
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::Dedent | Token::Eof) {
                break;
            }
            if let Some(arm) = self.parse_match_arm() {
                arms.push(arm);
            } else {
                break;
            }
        }
        if *self.peek() == Token::Dedent {
            self.advance();
        }
        let end = self.current_span().start;
        Some(MatchStmt { scrutinee, arms, span: Span { start, end } })
    }

    fn parse_match_arm(&mut self) -> Option<MatchArm> {
        let start = self.current_span().start;
        let pattern = self.parse_pattern()?;
        let guard = if *self.peek() == Token::If {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        // Body is either `: expr` (inline) or a colon-block.
        let body = if *self.peek() == Token::Colon {
            self.advance();
            match self.peek() {
                Token::Newline => {
                    self.advance();
                    let block = self.parse_indented_block()?;
                    MatchBody::Block(block)
                }
                _ => {
                    let expr = self.parse_expr()?;
                    MatchBody::Expr(expr)
                }
            }
        } else {
            let span = self.current_span();
            self.sink.emit(
                Diagnostic::error("expected `:` after match arm pattern")
                    .with_span(span, "here")
                    .with_code("E0104"),
            );
            return None;
        };
        let end = self.current_span().start;
        self.skip_newlines();
        Some(MatchArm { pattern, guard, body, span: Span { start, end } })
    }

    /// Expression statement or assignment: `expr` or `lvalue op= expr`.
    fn parse_expr_or_assign_stmt(&mut self) -> Option<Stmt> {
        let expr = self.parse_expr()?;
        let op = match self.peek() {
            Token::Eq       => Some(AssignOp::Eq),
            Token::PlusEq   => Some(AssignOp::PlusEq),
            Token::MinusEq  => Some(AssignOp::MinusEq),
            Token::StarEq   => Some(AssignOp::StarEq),
            Token::SlashEq  => Some(AssignOp::SlashEq),
            Token::PercentEq => Some(AssignOp::PercentEq),
            _ => None,
        };
        if let Some(op) = op {
            let start = expr_span(&expr).start;
            self.advance();
            let value = self.parse_expr()?;
            let end = self.current_span().start;
            self.skip_newlines();
            return Some(Stmt::Assign(AssignStmt {
                target: expr,
                op,
                value,
                span: Span { start, end },
            }));
        }

        // Trailing block on a call expression: `call(args): block`.
        // This is distinct from control-flow `:` (if/match/for) which is
        // consumed by those parsers before we reach here.
        if *self.peek() == Token::Colon {
            if let Expr::Call(ref call) = expr {
                let block = self.parse_colon_block()?;
                let span = Span { start: expr_span(&expr).start, end: block.span.end };
                let expr = Expr::Call(CallExpr {
                    callee: call.callee.clone(),
                    args: call.args.clone(),
                    trailing_block: Some(block),
                    span,
                });
                self.skip_newlines();
                return Some(Stmt::Expr(expr));
            }
        }

        self.skip_newlines();
        Some(Stmt::Expr(expr))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Patterns
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.current_span().start;
        match self.peek().clone() {
            Token::Ident(ref s) if s == "_" => {
                let span = self.current_span();
                self.advance();
                Some(Pattern::Wildcard(span))
            }
            Token::Ident(_) => {
                let name = self.parse_ident()?;
                if *self.peek() == Token::LParen {
                    self.advance();
                    let mut sub = Vec::new();
                    loop {
                        self.skip_newlines();
                        if matches!(self.peek(), Token::RParen | Token::Eof) {
                            break;
                        }
                        if let Some(p) = self.parse_pattern() {
                            sub.push(p);
                        }
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                    self.expect(&Token::RParen);
                    let end = self.current_span().start;
                    Some(Pattern::Variant(name, sub, Span { start, end }))
                } else {
                    let end = self.current_span().start;
                    Some(Pattern::Ident(name, Span { start, end }))
                }
            }
            Token::IntLit(n) => {
                let n = n;
                let span = self.current_span();
                self.advance();
                Some(Pattern::Literal(Literal::Int(n), span))
            }
            Token::StringLit(ref s) => {
                let s = s.clone();
                let span = self.current_span();
                self.advance();
                Some(Pattern::Literal(Literal::String(s), span))
            }
            Token::BoolLit(b) => {
                let span = self.current_span();
                self.advance();
                Some(Pattern::Literal(Literal::Bool(b), span))
            }
            _ => {
                let span = self.current_span();
                self.sink.emit(
                    Diagnostic::error(format!("expected pattern, found `{:?}`", self.peek()))
                        .with_span(span, "here")
                        .with_code("E0105"),
                );
                None
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Expressions — precedence climbing (SYNTAX.md §9)
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assignment_expr()
    }

    fn parse_assignment_expr(&mut self) -> Option<Expr> {
        self.parse_coalesce_expr()
    }

    fn parse_coalesce_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_or_expr()?;
        while *self.peek() == Token::QuestionQuestion {
            let op_span = self.current_span();
            self.advance();
            let right = self.parse_or_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr {
                op: BinOp::Coalesce,
                left: Box::new(left),
                right: Box::new(right),
                span,
            });
            let _ = op_span;
        }
        Some(left)
    }

    fn parse_or_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_and_expr()?;
        while *self.peek() == Token::PipePipe {
            self.advance();
            let right = self.parse_and_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr { op: BinOp::Or, left: Box::new(left), right: Box::new(right), span });
        }
        Some(left)
    }

    fn parse_and_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_eq_expr()?;
        while *self.peek() == Token::AmpAmp {
            self.advance();
            let right = self.parse_eq_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr { op: BinOp::And, left: Box::new(left), right: Box::new(right), span });
        }
        Some(left)
    }

    fn parse_eq_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_cmp_expr()?;
        loop {
            let op = match self.peek() {
                Token::EqEq  => BinOp::Eq,
                Token::BangEq => BinOp::Ne,
                _ => break,
            };
            self.advance();
            let right = self.parse_cmp_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr { op, left: Box::new(left), right: Box::new(right), span });
        }
        Some(left)
    }

    fn parse_cmp_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_range_expr()?;
        loop {
            let op = match self.peek() {
                Token::Lt   => BinOp::Lt,
                Token::LtEq => BinOp::Le,
                Token::Gt   => BinOp::Gt,
                Token::GtEq => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_range_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr { op, left: Box::new(left), right: Box::new(right), span });
        }
        Some(left)
    }

    fn parse_range_expr(&mut self) -> Option<Expr> {
        let left = self.parse_add_expr()?;
        match self.peek() {
            Token::DotDot | Token::DotDotEq => {
                let inclusive = *self.peek() == Token::DotDotEq;
                self.advance();
                let right = self.parse_add_expr()?;
                let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
                Some(Expr::Range(RangeExpr { start: Box::new(left), end: Box::new(right), inclusive, span }))
            }
            _ => Some(left),
        }
    }

    fn parse_add_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_mul_expr()?;
        loop {
            let op = match self.peek() {
                Token::Plus  => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr { op, left: Box::new(left), right: Box::new(right), span });
        }
        Some(left)
    }

    fn parse_mul_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_unary_expr()?;
        loop {
            let op = match self.peek() {
                Token::Star    => BinOp::Mul,
                Token::Slash   => BinOp::Div,
                Token::Percent => BinOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary_expr()?;
            let span = Span { start: expr_span(&left).start, end: expr_span(&right).end };
            left = Expr::BinOp(BinOpExpr { op, left: Box::new(left), right: Box::new(right), span });
        }
        Some(left)
    }

    fn parse_unary_expr(&mut self) -> Option<Expr> {
        let start = self.current_span().start;
        match self.peek() {
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                let end = expr_span(&operand).end;
                Some(Expr::UnaryOp(UnaryOpExpr {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                    span: Span { start, end },
                }))
            }
            Token::Bang => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                let end = expr_span(&operand).end;
                Some(Expr::UnaryOp(UnaryOpExpr {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                    span: Span { start, end },
                }))
            }
            Token::Await => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                let end = expr_span(&operand).end;
                Some(Expr::UnaryOp(UnaryOpExpr {
                    op: UnaryOp::Await,
                    operand: Box::new(operand),
                    span: Span { start, end },
                }))
            }
            _ => self.parse_postfix_expr(),
        }
    }

    fn parse_postfix_expr(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek() {
                // Member access: `expr.field`
                Token::Dot => {
                    self.advance();
                    let field = self.parse_ident()?;
                    let span = Span {
                        start: expr_span(&expr).start,
                        end: self.current_span().start,
                    };
                    expr = Expr::Member(MemberExpr { object: Box::new(expr), field, span });
                }
                // Call: `expr(args)`
                Token::LParen => {
                    let start = expr_span(&expr).start;
                    let args = self.parse_arg_list()?;
                    // NOTE: trailing blocks (`call(args): block`) are NOT parsed
                    // here in expression context. They are handled at statement
                    // level by `parse_bare_decl_or_expr_item` and
                    // `parse_colon_block`. Parsing them here would be ambiguous
                    // with the colon that follows an `if`/`match`/`for` condition.
                    let end = self.current_span().start;
                    expr = Expr::Call(CallExpr {
                        callee: Box::new(expr),
                        args,
                        trailing_block: None,
                        span: Span { start, end },
                    });
                }
                // Index: `expr[idx]`
                Token::LBracket => {
                    let start = expr_span(&expr).start;
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket);
                    let end = self.current_span().start;
                    expr = Expr::Index(IndexExpr {
                        object: Box::new(expr),
                        index: Box::new(index),
                        span: Span { start, end },
                    });
                }
                // Try: `expr?`
                Token::Question => {
                    let start = expr_span(&expr).start;
                    let end = self.current_span().end;
                    self.advance();
                    expr = Expr::Try(TryExpr { expr: Box::new(expr), span: Span { start, end } });
                }
                // Path separator: `Name::Variant` → treat as a further member
                Token::ColonColon => {
                    self.advance();
                    let field = self.parse_ident()?;
                    let span = Span {
                        start: expr_span(&expr).start,
                        end: self.current_span().start,
                    };
                    expr = Expr::Member(MemberExpr { object: Box::new(expr), field, span });
                }
                // Single-argument call sugar: `ident: expr` → `ident(expr)`.
                // Only apply when the token after `:` starts an expression.
                // If `:` is followed by a block-starting token, it belongs to
                // control-flow syntax (`if`/`match`/`for`) and must not be
                // consumed here.
                Token::Colon => {
                    if let Expr::Ident(ref name, _) = expr {
                        if !matches!(
                            self.peek2(),
                            Token::Newline | Token::Indent | Token::LBrace | Token::Dedent | Token::Eof
                        ) {
                            self.advance(); // consume `:`
                            let arg = self.parse_expr()?;
                            let arg_span = expr_span(&arg).clone();
                            let span = Span {
                                start: expr_span(&expr).start,
                                end: arg_span.end,
                            };
                            expr = Expr::Call(CallExpr {
                                callee: Box::new(Expr::Ident(name.clone(), expr_span(&expr).clone())),
                                args: vec![Arg { label: None, value: arg, span: arg_span }],
                                trailing_block: None,
                                span,
                            });
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        Some(expr)
    }

    fn parse_arg_list(&mut self) -> Option<Vec<Arg>> {
        self.expect(&Token::LParen)?;
        let mut args = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RParen | Token::Eof) {
                break;
            }
            if let Some(a) = self.parse_arg() {
                args.push(a);
            } else {
                break;
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Some(args)
    }

    fn parse_arg(&mut self) -> Option<Arg> {
        let start = self.current_span().start;
        let label = if let Token::Ident(_) = self.peek() {
            if *self.peek2() == Token::Colon {
                let name = self.parse_ident()?;
                self.advance(); // colon
                Some(name)
            } else {
                None
            }
        } else {
            None
        };
        let value = self.parse_expr()?;
        let end = self.current_span().start;
        Some(Arg { label, value, span: Span { start, end } })
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        let _start = self.current_span().start;
        match self.peek().clone() {
            Token::IntLit(n) => {
                let span = self.current_span();
                self.advance();
                Some(Expr::Literal(Literal::Int(n), span))
            }
            Token::FloatLit(f) => {
                let span = self.current_span();
                self.advance();
                Some(Expr::Literal(Literal::Float(f), span))
            }
            Token::BoolLit(b) => {
                let span = self.current_span();
                self.advance();
                Some(Expr::Literal(Literal::Bool(b), span))
            }
            Token::CharLit(c) => {
                let span = self.current_span();
                self.advance();
                Some(Expr::Literal(Literal::Char(c), span))
            }
            Token::StringLit(ref s) => {
                let s = s.clone();
                let span = self.current_span();
                self.advance();
                Some(Expr::Literal(Literal::String(s), span))
            }
            // Interpolated string: InterpStart … InterpEnd
            Token::InterpStart(ref prefix) => {
                let prefix = prefix.clone();
                let span_start = self.current_span().start;
                self.advance();
                let mut parts = vec![InterpPart::Literal(prefix)];
                // Parse expressions until InterpEnd
                loop {
                    if let Some(expr) = self.parse_expr() {
                        parts.push(InterpPart::Expr(expr));
                    }
                    match self.peek().clone() {
                        Token::InterpMiddle(ref m) => {
                            let m = m.clone();
                            self.advance();
                            parts.push(InterpPart::Literal(m));
                        }
                        Token::InterpEnd(ref e) => {
                            let e = e.clone();
                            let span_end = self.current_span().end;
                            self.advance();
                            parts.push(InterpPart::Literal(e));
                            return Some(Expr::StringInterp(StringInterpExpr {
                                parts,
                                span: Span { start: span_start, end: span_end },
                            }));
                        }
                        _ => break,
                    }
                }
                let end = self.current_span().start;
                Some(Expr::StringInterp(StringInterpExpr { parts, span: Span { start: span_start, end } }))
            }
            Token::Ident(ref s) => {
                let s = s.clone();
                let span = self.current_span();
                self.advance();
                // Struct literal: `Name { field: val, ... }`
                // Only parse as struct literal when `{` follows immediately
                // (no newline between the ident and the brace).
                if *self.peek() == Token::LBrace {
                    let start = span.start;
                    self.advance(); // `{`
                    let mut fields = Vec::new();
                    loop {
                        self.skip_newlines_and_semis();
                        if matches!(self.peek(), Token::RBrace | Token::Eof) {
                            break;
                        }
                        // Check for spread: `...expr`
                        if matches!(self.peek(), Token::DotDotDot) {
                            self.advance(); // consume `...`
                            let expr = self.parse_expr()?;
                            fields.push(StructField::Spread(Box::new(expr)));
                            self.eat(&Token::Comma);
                            continue;
                        }
                        if let Token::Ident(_) = self.peek() {
                            if *self.peek2() == Token::Colon {
                                let fname = self.parse_ident()?;
                                self.advance(); // colon
                                let fval = self.parse_expr()?;
                                fields.push(StructField::Named(fname, fval));
                                self.eat(&Token::Comma);
                                continue;
                            }
                        }
                        break;
                    }
                    let end = self.current_span().end;
                    self.expect(&Token::RBrace);
                    return Some(Expr::StructLit(StructLitExpr {
                        name: s,
                        fields,
                        span: Span { start, end },
                    }));
                }
                Some(Expr::Ident(s, span))
            }
            Token::Type => {
                let span = self.current_span();
                self.advance();
                Some(Expr::Ident("type".to_string(), span))
            }
            Token::LParen => {
                self.advance();
                // Could be a parenthesised expression or a tuple.
                let expr = self.parse_expr()?;
                if self.eat(&Token::Comma) {
                    // Tuple
                    let mut elems = vec![expr];
                    loop {
                        self.skip_newlines();
                        if matches!(self.peek(), Token::RParen | Token::Eof) { break; }
                        if let Some(e) = self.parse_expr() { elems.push(e); }
                        if !self.eat(&Token::Comma) { break; }
                    }
                    self.expect(&Token::RParen);
                    // Represent as a call to a synthetic "tuple" — for now just return first element
                    // TODO: add Tuple variant to Expr in a later pass
                    return Some(elems.remove(0));
                }
                self.expect(&Token::RParen);
                Some(expr)
            }
            Token::Pipe => {
                // Closure: `|params| body`
                let start = self.current_span().start;
                self.advance(); // consume `|`
                let mut params = Vec::new();
                // Parse parameters: `|a, b|` or `||`
                if *self.peek() != Token::Pipe {
                    loop {
                        self.skip_newlines();
                        if let Token::Ident(ref name) = self.peek() {
                            params.push(name.clone());
                            self.advance();
                        } else {
                            break;
                        }
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Token::Pipe); // closing `|`
                let body = self.parse_expr()?;
                let end = self.current_span().end;
                Some(Expr::Closure(ClosureExpr {
                    params,
                    body: Box::new(body),
                    span: Span { start, end },
                }))
            }
            Token::DotDotDot => {
                // Spread expression: `...expr`
                let start = self.current_span().start;
                self.advance(); // consume `...`
                let expr = self.parse_expr()?;
                let end = self.current_span().end;
                Some(Expr::Spread(SpreadExpr {
                    expr: Box::new(expr),
                    span: Span { start, end },
                }))
            }
            Token::LBracket => {
                // List literal `[a, b, c]`
                self.advance();
                let start = self.current_span().start;
                let mut elements = Vec::new();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), Token::RBracket | Token::Eof) { break; }
                    if let Some(e) = self.parse_expr() { elements.push(e); }
                    if !self.eat(&Token::Comma) { break; }
                }
                let end = self.current_span().end;
                self.expect(&Token::RBracket);
                Some(Expr::ListLit(ListLitExpr { elements, span: Span { start, end } }))
            }
            _ => {
                let span = self.current_span();
                self.sink.emit(
                    Diagnostic::error(format!(
                        "expected expression, found `{:?}`",
                        self.peek()
                    ))
                    .with_span(span, "here")
                    .with_code("E0106"),
                );
                None
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Type expressions
    // ─────────────────────────────────────────────────────────────────────────

    pub fn parse_type_expr(&mut self) -> Option<TypeExpr> {
        let start = self.current_span().start;
        match self.peek().clone() {
            Token::LBracket => {
                // `[T]` — collection type
                self.advance();
                let inner = self.parse_type_expr()?;
                let end = self.current_span().end;
                self.expect(&Token::RBracket);
                Some(TypeExpr::Collection(Box::new(inner), Span { start, end }))
            }
            Token::LParen => {
                // Tuple type `(A, B)` or function type `(A, B) -> C`
                self.advance();
                let mut types = Vec::new();
                loop {
                    self.skip_newlines();
                    if matches!(self.peek(), Token::RParen | Token::Eof) { break; }
                    if let Some(t) = self.parse_type_expr() { types.push(t); }
                    if !self.eat(&Token::Comma) { break; }
                }
                self.expect(&Token::RParen);
                if *self.peek() == Token::Arrow {
                    self.advance();
                    let ret = self.parse_type_expr()?;
                    let end = self.current_span().start;
                    Some(TypeExpr::Function(types, Box::new(ret), Span { start, end }))
                } else {
                    let end = self.current_span().start;
                    Some(TypeExpr::Tuple(types, Span { start, end }))
                }
            }
            Token::Ident(ref name) => {
                let name = name.clone();
                self.advance();
                // Generic args: `Name<T, U>`
                let generic_args = if *self.peek() == Token::Lt {
                    self.advance();
                    let mut args = Vec::new();
                    loop {
                        self.skip_newlines();
                        if matches!(self.peek(), Token::Gt | Token::Eof) { break; }
                        if let Some(t) = self.parse_type_expr() { args.push(t); }
                        if !self.eat(&Token::Comma) { break; }
                    }
                    self.expect(&Token::Gt);
                    args
                } else {
                    vec![]
                };
                let end = self.current_span().start;
                Some(TypeExpr::Named(name, generic_args, Span { start, end }))
            }
            _ => {
                let span = self.current_span();
                self.sink.emit(
                    Diagnostic::error(format!("expected type, found `{:?}`", self.peek()))
                        .with_span(span, "here")
                        .with_code("E0107"),
                );
                None
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Identifier helper
    // ─────────────────────────────────────────────────────────────────────────

    fn parse_ident(&mut self) -> Option<String> {
        match self.peek().clone() {
            Token::Ident(s) => {
                self.advance();
                Some(s)
            }
            _ => {
                let span = self.current_span();
                self.sink.emit(
                    Diagnostic::error(format!(
                        "expected identifier, found `{:?}`",
                        self.peek()
                    ))
                    .with_span(span, "here")
                    .with_code("E0108"),
                );
                None
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Error recovery
    // ─────────────────────────────────────────────────────────────────────────

    /// Skip forward until we reach something that looks like the start of a new
    /// top-level declaration (Ident at column 0 or an explicit keyword).
    fn skip_to_next_top_level(&mut self) {
        loop {
            match self.peek() {
                Token::Eof => break,
                Token::Fn | Token::Struct | Token::Enum | Token::Trait
                | Token::Impl | Token::Const | Token::Export => break,
                Token::Newline => {
                    self.advance();
                    // After a newline, if the next token is NOT indented (no Indent token),
                    // it's at the top level.
                    if !matches!(self.peek(), Token::Indent) {
                        break;
                    }
                }
                _ => { self.advance(); }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Span extraction helper
// ─────────────────────────────────────────────────────────────────────────────

fn expr_span(expr: &Expr) -> &Span {
    match expr {
        Expr::Literal(_, s) => s,
        Expr::Ident(_, s) => s,
        Expr::Call(e) => &e.span,
        Expr::Member(e) => &e.span,
        Expr::Index(e) => &e.span,
        Expr::BinOp(e) => &e.span,
        Expr::UnaryOp(e) => &e.span,
        Expr::Try(e) => &e.span,
        Expr::Range(e) => &e.span,
        Expr::StringInterp(e) => &e.span,
        Expr::StructLit(e) => &e.span,
        Expr::ListLit(e) => &e.span,
        Expr::Closure(e) => &e.span,
        Expr::Spread(e) => &e.span,
    }
}
