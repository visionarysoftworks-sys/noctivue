//! High-level analysis API for LSP and tooling.
//!
//! This module provides a convenient `analyze_file` function that runs the full
//! compiler frontend (lex → parse → resolve → typecheck) and returns structured
//! results suitable for LSP features: diagnostics, hover info, go-to-definition,
//! and completions.

use crate::ast::{Item, Program};
use crate::diagnostics::{Diagnostic, DiagnosticSink, Severity, Span};
use crate::hir::Module;
use crate::lexer::lex;
use crate::parser::parse;
use crate::resolver::resolve;
use crate::typeck::typecheck;
use std::str::FromStr;

/// Result of analyzing a single source file.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// Source file path (for reference)
    pub file_path: String,
    /// Source text (for span-to-position conversion)
    pub source: String,
    /// All diagnostics from all pipeline stages
    pub diagnostics: Vec<Diagnostic>,
    /// The unresolved AST (from parser)
    pub ast: Program,
    /// The resolved AST (from resolver) — BareDecls classified
    pub resolved: Program,
    /// The typed HIR module (from typecheck)
    pub typed: Module,
}

/// Run the full compiler frontend on a source file.
///
/// This is the primary entry point for LSP server and other tooling.
/// It runs: lex → parse → resolve → typecheck, collecting diagnostics at each stage.
pub fn analyze_file(file_path: impl AsRef<str>, source: &str) -> AnalysisResult {
    let file_path = file_path.as_ref().to_string();
    let mut sink = DiagnosticSink::new();

    let tokens = lex(source, &mut sink);
    let ast = parse(&tokens, &mut sink);
    let resolved = resolve(ast.clone(), &mut sink);
    let typed = typecheck(resolved.clone(), &mut sink);

    AnalysisResult {
        file_path,
        source: source.to_string(),
        diagnostics: sink.take(),
        ast,
        resolved,
        typed,
    }
}

/// Convert a byte-offset [`Span`] to an LSP [`Position`] (0-based line/character).
pub fn span_to_position(source: &str, span: &Span) -> lsp_types::Position {
    let start_pos = byte_offset_to_position(source, span.start);
    lsp_types::Position::new(start_pos.line as u32, start_pos.character as u32)
}

/// Convert a byte-offset [`Span`] to an LSP [`Range`].
pub fn span_to_range(source: &str, span: &Span) -> lsp_types::Range {
    let start = byte_offset_to_position(source, span.start);
    let end = byte_offset_to_position(source, span.end);
    lsp_types::Range::new(
        lsp_types::Position::new(start.line as u32, start.character as u32),
        lsp_types::Position::new(end.line as u32, end.character as u32),
    )
}

/// Convert a compiler [`Diagnostic`] to an LSP [`Diagnostic`].
pub fn diagnostic_to_lsp(source: &str, diag: &Diagnostic) -> lsp_types::Diagnostic {
    let severity = match diag.severity {
        Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
        Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
        Severity::Note => lsp_types::DiagnosticSeverity::INFORMATION,
        Severity::Help => lsp_types::DiagnosticSeverity::HINT,
    };

    let range = if let Some(label) = diag.labels.first() {
        span_to_range(source, &label.span)
    } else {
        // Fallback: entire file
        lsp_types::Range::new(
            lsp_types::Position::new(0, 0),
            lsp_types::Position::new(0, 0),
        )
    };

    let code = diag.code.clone().map(lsp_types::NumberOrString::String);

    // Related information (additional labels beyond the first)
    let related: Option<Vec<lsp_types::DiagnosticRelatedInformation>> = if diag.labels.len() > 1 {
        Some(
            diag.labels
                .iter()
                .skip(1)
                .map(|label| {
                    lsp_types::DiagnosticRelatedInformation {
                        location: lsp_types::Location {
                            uri: lsp_types::Uri::from_str("file://dummy").unwrap(), // Will be overridden by client
                            range: span_to_range(source, &label.span),
                        },
                        message: label.message.clone(),
                    }
                })
                .collect(),
        )
    } else {
        None
    };

    let tags = None; // Could add Deprecated/Unnecessary tags if needed

    lsp_types::Diagnostic::new(
        range,
        Some(severity),
        code,
        Some("noctivue".to_string()),
        diag.message.clone(),
        related,
        tags,
    )
}

/// Internal: convert byte offset to (line, character) position.
/// Both line and character are 0-based.
fn byte_offset_to_position(source: &str, offset: usize) -> BytePosition {
    let mut line = 0;
    let mut line_start = 0;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    BytePosition {
        line,
        character: offset - line_start,
    }
}

#[derive(Debug)]
struct BytePosition {
    line: usize,
    character: usize,
}

/// Find all definitions in the resolved AST for go-to-definition and completions.
pub fn collect_definitions(resolved: &Program) -> Vec<Definition> {
    let mut defs = Vec::new();
    for item in &resolved.items {
        match item {
            Item::Struct(s) => defs.push(Definition {
                name: s.name.clone(),
                kind: DefinitionKind::Struct,
                span: s.span.clone(),
                detail: format!("struct {}", s.name),
            }),
            Item::Function(f) => defs.push(Definition {
                name: f.name.clone(),
                kind: DefinitionKind::Function,
                span: f.span.clone(),
                detail: format!("fn {}(...)", f.name),
            }),
            Item::Task(t) => defs.push(Definition {
                name: t.name.clone(),
                kind: DefinitionKind::Function,
                span: t.span.clone(),
                detail: format!("task {}(...) -> handle", t.name),
            }),
            Item::Enum(e) => defs.push(Definition {
                name: e.name.clone(),
                kind: DefinitionKind::Enum,
                span: e.span.clone(),
                detail: format!("enum {}", e.name),
            }),
            Item::Const(c) => defs.push(Definition {
                name: c.name.clone(),
                kind: DefinitionKind::Const,
                span: c.span.clone(),
                detail: format!(
                    "const {}: {}",
                    c.name,
                    crate::ast::type_expr_to_string(&c.ty)
                ),
            }),
            Item::Trait(t) => defs.push(Definition {
                name: t.name.clone(),
                kind: DefinitionKind::Trait,
                span: t.span.clone(),
                detail: format!("trait {}", t.name),
            }),
            Item::BareDecl(d) => defs.push(Definition {
                name: d.name.clone(),
                kind: DefinitionKind::Component,
                span: d.span.clone(),
                detail: format!("component {}", d.name),
            }),
            Item::Mod(m) => defs.push(Definition {
                name: m.name.clone(),
                kind: DefinitionKind::Module,
                span: m.span.clone(),
                detail: format!("mod {}", m.name),
            }),
            Item::Export(_) => {}
            Item::Impl(_) => {}
        }
    }
    defs
}

/// A named definition with its location and kind.
#[derive(Debug, Clone)]
pub struct Definition {
    pub name: String,
    pub kind: DefinitionKind,
    pub span: Span,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Struct,
    Function,
    Enum,
    Const,
    Trait,
    Component,
    Module,
}

/// Find the definition at a given position (for go-to-definition).
pub fn find_definition_at(
    resolved: &Program,
    source: &str,
    position: lsp_types::Position,
) -> Option<Definition> {
    let offset = position_to_byte_offset(source, position);
    for item in &resolved.items {
        match item {
            Item::Struct(s) if span_contains(&s.span, offset) => {
                return Some(Definition {
                    name: s.name.clone(),
                    kind: DefinitionKind::Struct,
                    span: s.span.clone(),
                    detail: format!("struct {}", s.name),
                })
            }
            Item::Function(f) if span_contains(&f.span, offset) => {
                return Some(Definition {
                    name: f.name.clone(),
                    kind: DefinitionKind::Function,
                    span: f.span.clone(),
                    detail: format!("fn {}(...)", f.name),
                })
            }
            Item::Enum(e) if span_contains(&e.span, offset) => {
                return Some(Definition {
                    name: e.name.clone(),
                    kind: DefinitionKind::Enum,
                    span: e.span.clone(),
                    detail: format!("enum {}", e.name),
                })
            }
            Item::Const(c) if span_contains(&c.span, offset) => {
                return Some(Definition {
                    name: c.name.clone(),
                    kind: DefinitionKind::Const,
                    span: c.span.clone(),
                    detail: format!(
                        "const {}: {}",
                        c.name,
                        crate::ast::type_expr_to_string(&c.ty)
                    ),
                })
            }
            Item::Trait(t) if span_contains(&t.span, offset) => {
                return Some(Definition {
                    name: t.name.clone(),
                    kind: DefinitionKind::Trait,
                    span: t.span.clone(),
                    detail: format!("trait {}", t.name),
                })
            }
            Item::BareDecl(d) if span_contains(&d.span, offset) => {
                return Some(Definition {
                    name: d.name.clone(),
                    kind: DefinitionKind::Component,
                    span: d.span.clone(),
                    detail: format!("component {}", d.name),
                })
            }
            Item::Mod(m) if span_contains(&m.span, offset) => {
                return Some(Definition {
                    name: m.name.clone(),
                    kind: DefinitionKind::Module,
                    span: m.span.clone(),
                    detail: format!("mod {}", m.name),
                })
            }
            Item::Task(t) if span_contains(&t.span, offset) => {
                return Some(Definition {
                    name: t.name.clone(),
                    kind: DefinitionKind::Function,
                    span: t.span.clone(),
                    detail: format!("task {}(...)", t.name),
                })
            }
            _ => {}
        }
    }
    None
}

/// Check if a span contains a byte offset.
fn span_contains(span: &Span, offset: usize) -> bool {
    span.start <= offset && offset < span.end
}

/// Convert LSP Position to byte offset.
fn position_to_byte_offset(source: &str, position: lsp_types::Position) -> usize {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in source.char_indices() {
        if line == position.line as usize && col == position.character as usize {
            return i;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    source.len()
}

/// Get completions at a position.
pub fn get_completions(
    resolved: &Program,
    _source: &str,
    _position: lsp_types::Position,
) -> Vec<lsp_types::CompletionItem> {
    let mut items = Vec::new();
    let definitions = collect_definitions(resolved);
    for def in definitions {
        let kind = match def.kind {
            DefinitionKind::Struct => lsp_types::CompletionItemKind::STRUCT,
            DefinitionKind::Function => lsp_types::CompletionItemKind::FUNCTION,
            DefinitionKind::Enum => lsp_types::CompletionItemKind::ENUM,
            DefinitionKind::Const => lsp_types::CompletionItemKind::CONSTANT,
            DefinitionKind::Trait => lsp_types::CompletionItemKind::INTERFACE,
            DefinitionKind::Component => lsp_types::CompletionItemKind::CLASS,
            DefinitionKind::Module => lsp_types::CompletionItemKind::MODULE,
        };
        items.push(lsp_types::CompletionItem {
            label: def.name,
            kind: Some(kind),
            detail: Some(def.detail),
            documentation: None,
            ..Default::default()
        });
    }
    // Add keywords
    for kw in &[
        "fn", "struct", "enum", "const", "let", "var", "if", "else", "match", "while", "for",
        "loop", "return", "break", "continue", "import", "export", "mod", "use", "as", "trait",
        "impl", "unsafe", "async", "await",
    ] {
        items.push(lsp_types::CompletionItem {
            label: (*kw).to_string(),
            kind: Some(lsp_types::CompletionItemKind::KEYWORD),
            detail: Some("keyword".to_string()),
            ..Default::default()
        });
    }
    items
}

/// Get hover information at a position.
/// Resolves the specific identifier under the cursor only — there is
/// deliberately no "enclosing item" fallback, so hovering whitespace or an
/// unknown name shows nothing instead of a misleading parent item.
pub fn get_hover(
    resolved: &Program,
    typed: &Module,
    source: &str,
    position: lsp_types::Position,
    file_label: &str,
) -> Option<lsp_types::Hover> {
    let offset = position_to_byte_offset(source, position);
    get_hover_for_identifier(resolved, typed, source, offset, file_label)
}

/// The body of a hover card, rendered in a fixed section order mirroring
/// mainstream language servers (signature, type, declaring location, docs).
struct HoverInfo {
    /// Signature shown in a fenced `noctivue` code block.
    signature: String,
    /// Optional `Type: ...` line (functions, locals).
    ty: Option<String>,
    /// `Declared in ...` line (file name, or `std (builtin)`).
    declared_in: String,
    /// Optional `///` doc text (user items) or hand-written notes (builtins).
    docs: Option<String>,
}

fn render_hover(info: HoverInfo, range: Option<lsp_types::Range>) -> lsp_types::Hover {
    let mut md = format!("```noctivue\n{}\n```", info.signature);
    if let Some(ty) = info.ty {
        md.push_str(&format!("\n\nType: {ty}"));
    }
    md.push_str(&format!("\n\nDeclared in {}.", info.declared_in));
    if let Some(docs) = info.docs {
        md.push_str(&format!("\n\n{docs}"));
    }
    lsp_types::Hover {
        contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
            kind: lsp_types::MarkupKind::Markdown,
            value: md,
        }),
        range,
    }
}

/// Built-in functions: signature plus behavioral docs verified against the
/// tree-walking interpreter (`interp::eval_builtin`), not aspirational text.
fn builtin_hover(name: &str) -> Option<HoverInfo> {
    let (signature, ty, docs) = match name {
        "print" => (
            "fn print(value: String)",
            "(String) -> ()",
            "Writes a value to standard output **without** a trailing newline.\n\nUse `println` for line-oriented output. String interpolation (`\"{expr}\"`) is evaluated before printing.",
        ),
        "println" => (
            "fn println(value: String)",
            "(String) -> ()",
            "Writes a value to standard output followed by a trailing newline.\n\nThis is the line-oriented counterpart to `print`.",
        ),
        "to_string" => (
            "fn to_string(value: String) -> String",
            "(String) -> String",
            "Converts any value to its string representation.",
        ),
        "to_int" => (
            "fn to_int(value: String) -> Option<Int>",
            "(String) -> Option<Int>",
            "Parses a string as an integer (leading/trailing whitespace is trimmed).\n\nReturns `None` when the string is not a valid integer. Non-string numeric inputs are converted directly (`Float` truncates toward zero).",
        ),
        "to_float" => (
            "fn to_float(value: String) -> Option<Float>",
            "(String) -> Option<Float>",
            "Parses a string as a float (leading/trailing whitespace is trimmed).\n\nReturns `None` when the string is not a valid float. Non-string numeric inputs are converted directly.",
        ),
        "panic" => (
            "fn panic(message: String)",
            "(String) -> ()",
            "Aborts the program immediately with the given message.\n\nA panic unwinds to the interpreter boundary and surfaces as an uncaught error, never as a `Result::Err`.",
        ),
        "assert" => (
            "fn assert(condition: Bool, message: String)",
            "(Bool, String) -> ()",
            "Checks a condition; does nothing when it is `true`.\n\nWhen the condition is `false`, aborts with `message` (default: `\"assertion failed\"`). A non-`Bool` condition is itself a panic.",
        ),
        _ => return None,
    };
    Some(HoverInfo {
        signature: signature.to_string(),
        ty: Some(ty.to_string()),
        declared_in: "std (builtin)".to_string(),
        docs: Some(docs.to_string()),
    })
}

/// Collect the consecutive `///` doc-comment lines immediately above the
/// line containing `item_start` (mirrors the `///`-for-declarations rule in
/// STYLE_GUIDE.md §5). A blank line between the docs and the item ends the
/// block; the `///` prefix and one following space are stripped.
///
/// `pub` deliberately: `noct doc` renders through this (one rule, two
/// consumers with hover).
pub fn doc_comment_for(source: &str, item_start: usize) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut item_line = 0;
    let mut seen = 0;
    for (i, line) in lines.iter().enumerate() {
        let line_start = seen;
        seen += line.len() + 1; // +1 for the stripped '\n'
        if line_start <= item_start && item_start < seen {
            item_line = i;
            break;
        }
    }
    let mut docs: Vec<String> = Vec::new();
    let mut i = item_line;
    while i > 0 {
        i -= 1;
        let trimmed = lines[i].trim_start();
        match trimmed.strip_prefix("///") {
            Some(rest) => docs.push(rest.strip_prefix(' ').unwrap_or(rest).to_string()),
            None => break,
        }
    }
    if docs.is_empty() {
        return None;
    }
    docs.reverse();
    Some(docs.join("\n"))
}

/// Hover cards for keywords, literals, and prelude constructors.
///
/// Every keyword the language defines MUST have an entry here: a usage
/// template as the signature plus a one-to-two-line role summary. Adding a
/// new keyword without its hover entry is an incomplete change (see
/// TOOLCHAIN.md §6.1). Reserved words share one explicit "reserved" note.
fn keyword_hover(name: &str) -> Option<HoverInfo> {
    // (signature, docs)
    let (signature, docs): (&str, &str) = match name {
        // ── Declarations ──
        "fn" => ("fn name(params) -> Ret:",
            "Declares a function. Parameters are comma-separated `name: Type` pairs; the return type follows `->`."),
        "struct" => ("struct Name:\n    field: Type",
            "Declares a record type with named fields. Values are built with struct literals (`Name { field: value }`)."),
        "enum" => ("enum Name:\n    Variant(payload)",
            "Declares an enumerated type. `match` over an enum must be exhaustive."),
        "trait" => ("trait Name:\n    fn method(...) -> ...",
            "Declares an interface of required methods, implemented with `impl Trait for Type`."),
        "impl" => ("impl [Trait for] Type:",
            "Implements methods for a type, or implements a trait for a type."),
        "type" => ("type Name = ExistingType",
            "Declares a type alias: a new name for an existing type."),
        "const" => ("const NAME: Type = value",
            "Declares an immutable compile-time constant. Names use SCREAMING_CASE by convention."),
        "mod" => ("mod name:",
            "Declares a module. Items are module-private unless marked `export`."),
        "export" => ("export ...",
            "Re-exports an item so importing modules can use it."),
        "import" => ("import path::Item",
            "Imports items from another module. `import path::Item` binds the item; `import path` binds the module namespace."),
        "use" => ("use ...",
            "Brings an imported path into scope."),
        // ── Bindings & control flow ──
        "let" => ("let name[: Type] = value",
            "Declares an immutable local binding. The type is inferred when omitted."),
        "var" => ("var name[: Type] = value",
            "Declares a mutable local binding. Prefer `let` unless mutation is needed."),
        "if" => ("if condition:\n    ...\nelse:\n    ...",
            "Conditional branch. The condition must be `Bool`."),
        "else" => ("else:",
            "Fallback branch of an `if`, or of an `else if` chain."),
        "match" => ("match scrutinee:\n    Pattern:\n        ...",
            "Exhaustive pattern match over enums, `Option`, `Result`, and literals. A missing variant is a compile error; `_` is the catch-all."),
        "while" => ("while condition:",
            "Loops while the condition holds (`Bool`)."),
        "loop" => ("loop:",
            "Infinite loop. Exits via `break`, skips iterations via `continue`."),
        "for" => ("for binding in iterable:",
            "Iterates over a collection or range, binding each element in turn."),
        "in" => ("for binding in iterable",
            "Names the iteration source in a `for` loop."),
        "return" => ("return [expr]",
            "Returns a value from the current function (must match the declared return type)."),
        "break" => ("break",
            "Exits the innermost loop immediately."),
        "continue" => ("continue",
            "Skips to the next iteration of the innermost loop."),
        "as" => ("expr as Type",
            "Converts a value between compatible types."),
        // ── Async ──
        "async" => ("async fn name(...) -> ...:",
            "Marks a function as asynchronous. Callers drive it with `await`."),
        "await" => ("await expr",
            "Suspends until an async operation completes, yielding its value."),
        "task" => ("task ...",
            "Introduces a unit of concurrent work under structured concurrency."),
        // ── Memory modes ──
        "owned" => ("owned",
            "Ownership annotation (native mode): this binding owns its value. Enforcement lands with M2 native compilation."),
        "borrow" => ("borrow",
            "Borrow annotation (native mode): uses a value without taking ownership. Enforcement lands with M2 native compilation."),
        "managed" => ("managed ...",
            "Managed-memory (ARC) mode annotation. The exact declaration-site syntax is still Proposed (MEMORY_MODEL.md)."),
        "weak" => ("weak",
            "Non-owning reference: does not keep the referent alive. Access yields `Option<T>`."),
        "unowned" => ("unowned",
            "Non-owning reference: does not keep the referent alive. Access assumes the referent is still valid."),
        // ── Unsafe / FFI ──
        "unsafe" => ("unsafe ...:",
            "Marks a block where unsafe operations (FFI calls, raw pointers) are permitted. Safety invariants become the author's responsibility."),
        _ => return None,
    };
    Some(HoverInfo {
        signature: signature.to_string(),
        ty: None,
        declared_in: "keyword".to_string(),
        docs: Some(docs.to_string()),
    })
}

/// Reserved words: lexed as keywords but not part of the language surface.
fn reserved_hover(name: &str) -> Option<HoverInfo> {
    const RESERVED: &[&str] = &[
        "actor", "defer", "extern", "macro", "native", "operator", "protocol", "reflect", "spawn",
        "static", "where", "yield",
    ];
    if !RESERVED.contains(&name) {
        return None;
    }
    Some(HoverInfo {
        signature: name.to_string(),
        ty: None,
        declared_in: "keyword (reserved)".to_string(),
        docs: Some(
            "Reserved word: recognized by the lexer but not part of the language. Reserved for future use — do not use as an identifier.".to_string(),
        ),
    })
}

/// Prelude literals and constructors.
fn prelude_hover(name: &str) -> Option<HoverInfo> {
    let (signature, ty, docs): (&str, &str, &str) = match name {
        "true" => ("true", "Bool", "The `Bool` constant for truth."),
        "false" => ("false", "Bool", "The `Bool` constant for falsity."),
        "None" => (
            "None",
            "Option<T> (empty)",
            "The empty `Option`: a value explicitly absent. Compare with `Some(v)`.",
        ),
        "Some" => (
            "Some(value: T) -> Option<T>",
            "(T) -> Option<T>",
            "Wraps a value as a present `Option`. Unwrap with `match` or `?`.",
        ),
        "Ok" => (
            "Ok(value: T) -> Result<T, E>",
            "(T) -> Result<T, Unknown>",
            "Wraps a success value as a `Result`. Unwrap with `match` or `?`.",
        ),
        "Err" => (
            "Err(error: E) -> Result<T, E>",
            "(E) -> Result<Unknown, E>",
            "Wraps a failure value as a `Result`. Unwrap with `match` or `?`.",
        ),
        _ => return None,
    };
    Some(HoverInfo {
        signature: signature.to_string(),
        ty: Some(ty.to_string()),
        declared_in: "prelude".to_string(),
        docs: Some(docs.to_string()),
    })
}

/// File label used in `Declared in ...` lines (basename, not full path).
/// Currently exercised by the `analysis::tests` hover tests; the LSP server
/// derives its own labels from document URIs.
#[allow(dead_code)]
fn file_label_of(file_path: &str) -> String {
    file_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_path)
        .to_string()
}

/// Find hover info for a specific identifier at the given byte offset.
/// Lookup order: top-level definitions, then locals, then builtins —
/// a user-defined item always shadows a builtin of the same name.
fn get_hover_for_identifier(
    resolved: &Program,
    typed: &Module,
    source: &str,
    offset: usize,
    file_label: &str,
) -> Option<lsp_types::Hover> {
    // Extract the identifier at the cursor position
    let ident = extract_identifier_at(source, offset)?;

    // Look up the identifier in top-level definitions
    for item in &resolved.items {
        let (info, span) = match item {
            Item::Struct(s) if s.name == ident => {
                let fields: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| format!("  {}: {}", f.name, crate::ast::type_expr_to_string(&f.ty)))
                    .collect();
                (
                    HoverInfo {
                        signature: format!("struct {} {{\n{}\n}}", s.name, fields.join(",\n")),
                        ty: None,
                        declared_in: file_label.to_string(),
                        docs: doc_comment_for(source, s.span.start),
                    },
                    s.span.clone(),
                )
            }
            Item::Function(f) if f.name == ident => {
                let params: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, crate::ast::type_expr_to_string(&p.ty)))
                    .collect();
                let ret = f
                    .return_ty
                    .as_ref()
                    .map(|t| format!(" -> {}", crate::ast::type_expr_to_string(t)))
                    .unwrap_or_default();
                (
                    HoverInfo {
                        signature: format!("fn {}({}){}", f.name, params.join(", "), ret),
                        ty: fn_type_of(typed, &f.name),
                        declared_in: file_label.to_string(),
                        docs: doc_comment_for(source, f.span.start),
                    },
                    f.span.clone(),
                )
            }
            Item::Enum(e) if e.name == ident => {
                let variants: Vec<String> =
                    e.variants.iter().map(|v| format!("  {}", v.name)).collect();
                (
                    HoverInfo {
                        signature: format!("enum {} {{\n{}\n}}", e.name, variants.join(",\n")),
                        ty: None,
                        declared_in: file_label.to_string(),
                        docs: doc_comment_for(source, e.span.start),
                    },
                    e.span.clone(),
                )
            }
            Item::Const(c) if c.name == ident => (
                HoverInfo {
                    signature: format!(
                        "const {}: {}",
                        c.name,
                        crate::ast::type_expr_to_string(&c.ty)
                    ),
                    ty: None,
                    declared_in: file_label.to_string(),
                    docs: doc_comment_for(source, c.span.start),
                },
                c.span.clone(),
            ),
            Item::Trait(t) if t.name == ident => {
                let methods: Vec<String> = t
                    .members
                    .iter()
                    .map(|m| format!("  fn {}", m.name))
                    .collect();
                (
                    HoverInfo {
                        signature: format!("trait {} {{\n{}\n}}", t.name, methods.join(",\n")),
                        ty: None,
                        declared_in: file_label.to_string(),
                        docs: doc_comment_for(source, t.span.start),
                    },
                    t.span.clone(),
                )
            }
            _ => continue,
        };
        return Some(render_hover(info, Some(span_to_range(source, &span))));
    }

    // Check local variables/parameters in the typed HIR
    if let Some(local_hover) = get_hover_for_local(typed, &ident, file_label) {
        return Some(local_hover);
    }

    // Built-in functions (user-defined items take precedence, checked above)
    if let Some(info) = builtin_hover(&ident) {
        return Some(render_hover(info, None));
    }

    // Keywords, reserved words, and prelude items come last: they cannot be
    // shadowed by user code, so order relative to the tables above is moot,
    // but user items must win wherever shadowing is possible.
    if let Some(info) = keyword_hover(&ident) {
        return Some(render_hover(info, None));
    }
    if let Some(info) = reserved_hover(&ident) {
        return Some(render_hover(info, None));
    }
    if let Some(info) = prelude_hover(&ident) {
        return Some(render_hover(info, None));
    }

    None
}

/// `Type: ...` line for a top-level function, from its HIR signature.
fn fn_type_of(typed: &Module, name: &str) -> Option<String> {
    typed.functions.iter().find(|f| f.name == name).map(|f| {
        let params: Vec<String> = f.params.iter().map(|(_, ty)| ty.to_string()).collect();
        format!("({}) -> {}", params.join(", "), f.return_ty)
    })
}

/// Get hover for local variables/parameters from typed HIR (simplified - no span)
fn get_hover_for_local(typed: &Module, ident: &str, file_label: &str) -> Option<lsp_types::Hover> {
    for func in &typed.functions {
        for stmt in &func.body {
            if let Some(local_hover) = find_local_in_stmt(stmt, ident, file_label) {
                return Some(local_hover);
            }
        }
    }
    None
}

/// Extract identifier at a byte offset (word-boundary expansion).
/// Works on byte offsets via `char_indices` so non-ASCII source (e.g. the
/// `café`/`привет` identifiers from LANGUAGE_SPEC.md §1) can't desync the
/// slicing.
fn extract_identifier_at(source: &str, offset: usize) -> Option<String> {
    if offset > source.len() {
        return None;
    }

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    // Byte index of the char containing `offset` (or the char just before it
    // when the cursor sits exactly on a boundary).
    let mut start = source.len();
    let mut end = source.len();
    let mut found = false;
    for (i, ch) in source.char_indices() {
        let next = i + ch.len_utf8();
        if i <= offset && offset < next {
            if !is_word_char(ch) {
                return None;
            }
            start = i;
            end = next;
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }

    while start > 0 {
        let prev_start = source[..start]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        let prev: char = source[prev_start..].chars().next().unwrap_or(' ');
        if !is_word_char(prev) {
            break;
        }
        start = prev_start;
    }
    while end < source.len() {
        let ch: char = source[end..].chars().next().unwrap_or(' ');
        if !is_word_char(ch) {
            break;
        }
        end += ch.len_utf8();
    }

    if start < end {
        Some(source[start..end].to_string())
    } else {
        None
    }
}

/// Recursively search for local variable/parameter in statements
fn find_local_in_stmt(
    stmt: &crate::hir::items::TypedStmt,
    ident: &str,
    file_label: &str,
) -> Option<lsp_types::Hover> {
    use crate::hir::items::TypedStmtKind;

    match &stmt.kind {
        TypedStmtKind::Let { name, ty, .. }
        | TypedStmtKind::Var { name, ty, .. }
        | TypedStmtKind::Decl { name, ty, .. }
            if name == ident =>
        {
            return Some(render_hover(
                HoverInfo {
                    signature: format!("let {name}: {ty}"),
                    ty: Some(ty.to_string()),
                    declared_in: format!("{file_label} (local binding)"),
                    docs: None,
                },
                None,
            ));
        }
        _ => {}
    }

    // Check nested statements
    match &stmt.kind {
        TypedStmtKind::If {
            then_body,
            else_body,
            ..
        } => {
            for s in then_body {
                if let Some(h) = find_local_in_stmt(s, ident, file_label) {
                    return Some(h);
                }
            }
            if let Some(eb) = else_body {
                for s in eb {
                    if let Some(h) = find_local_in_stmt(s, ident, file_label) {
                        return Some(h);
                    }
                }
            }
        }
        TypedStmtKind::While { body, .. }
        | TypedStmtKind::Loop { body, .. }
        | TypedStmtKind::For { body, .. } => {
            for s in body {
                if let Some(h) = find_local_in_stmt(s, ident, file_label) {
                    return Some(h);
                }
            }
        }
        TypedStmtKind::Match { arms, .. } => {
            for arm in arms {
                for s in &arm.body {
                    if let Some(h) = find_local_in_stmt(s, ident, file_label) {
                        return Some(h);
                    }
                }
            }
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = "fn greet(name: String) -> String:\n    name\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n\nmain():\n    print(greet(\"World\"))\n    let user = greet(\"Bob\")\n    print(\"User 99: {user}\")\n";

    fn hover_text_at(source: &str, line: u32, character: u32) -> Option<String> {
        let a = analyze_file("test.nv", source);
        let label = file_label_of(&a.file_path);
        let h = get_hover(
            &a.resolved,
            &a.typed,
            source,
            lsp_types::Position::new(line, character),
            &label,
        )?;
        match h.contents {
            lsp_types::HoverContents::Markup(m) => Some(m.value),
            _ => None,
        }
    }

    #[test]
    fn hover_call_site_shows_callee_signature() {
        // `greet` call on line 7: `    print(greet("World"))` — col of `greet` is 10
        let text = hover_text_at(DEMO, 7, 11).expect("expected hover on greet call");
        assert!(
            text.contains("fn greet(name: String) -> String"),
            "got: {text}"
        );
    }

    #[test]
    fn hover_builtin_print_shows_signature_not_main() {
        // `print` call on line 7, col 4
        let text = hover_text_at(DEMO, 7, 5).expect("expected hover on print");
        assert!(!text.contains("fn main"), "got fallback main: {text}");
        assert!(text.contains("print"), "got: {text}");
    }

    #[test]
    fn hover_whitespace_shows_nothing() {
        // Leading spaces of the print line: no identifier under cursor
        assert!(hover_text_at(DEMO, 7, 0).is_none());
    }

    #[test]
    fn hover_local_binding_shows_type() {
        // `user` use on line 9: `    print("User 99: {user}")` — col of `user` is 22
        let text = hover_text_at(DEMO, 9, 23).expect("expected hover on local user");
        assert!(
            text.contains("user") && text.contains("String"),
            "got: {text}"
        );
    }

    #[test]
    fn hover_definition_name_shows_signature() {
        // `add` definition on line 3, col 3
        let text = hover_text_at(DEMO, 3, 4).expect("expected hover on add def");
        assert!(
            text.contains("fn add(a: Int, b: Int) -> Int"),
            "got: {text}"
        );
    }

    #[test]
    fn hover_shows_type_and_declared_in() {
        let text = hover_text_at(DEMO, 7, 11).expect("expected hover on greet call");
        assert!(text.contains("Type: (String) -> String"), "got: {text}");
        assert!(text.contains("Declared in test.nv."), "got: {text}");
    }

    #[test]
    fn hover_builtin_shows_full_card() {
        let text = hover_text_at(DEMO, 7, 5).expect("expected hover on print");
        assert!(text.contains("fn print(value: String)"), "got: {text}");
        assert!(text.contains("Type: (String) -> ()"), "got: {text}");
        assert!(text.contains("Declared in std (builtin)."), "got: {text}");
        assert!(text.contains("without"), "got: {text}");
    }

    const DEMO_DOCS: &str = "/// Greets a person by name.\n///\n/// Returns the greeting string.\nfn greet(name: String) -> String:\n    name\n\nmain():\n    print(greet(\"World\"))\n";

    #[test]
    fn hover_shows_doc_comments() {
        // `greet` definition name on line 3
        let text = hover_text_at(DEMO_DOCS, 3, 4).expect("expected hover on greet def");
        assert!(text.contains("Greets a person by name."), "got: {text}");
        assert!(text.contains("Returns the greeting string."), "got: {text}");
        // ...and at the call site on line 7 too
        let text = hover_text_at(DEMO_DOCS, 7, 11).expect("expected hover on greet call");
        assert!(text.contains("Greets a person by name."), "got: {text}");
    }

    #[test]
    fn hover_unknown_identifier_shows_nothing() {
        // `World` string content is not an identifier with a definition
        assert!(hover_text_at(DEMO, 7, 17).is_none());
    }

    #[test]
    fn hover_keyword_shows_template() {
        // `fn` keyword on line 0
        let text = hover_text_at(DEMO, 0, 1).expect("expected hover on fn keyword");
        assert!(text.contains("fn name(params) -> Ret:"), "got: {text}");
        assert!(text.contains("Declared in keyword."), "got: {text}");
        // `let` keyword on line 8
        let text = hover_text_at(DEMO, 8, 5).expect("expected hover on let keyword");
        assert!(text.contains("let name[: Type] = value"), "got: {text}");
    }

    #[test]
    fn hover_reserved_word_shows_reserved_note() {
        let a = analyze_file("test.nv", "actor:\n    x = 1\n");
        let label = file_label_of(&a.file_path);
        let h = get_hover(
            &a.resolved,
            &a.typed,
            "actor:\n    x = 1\n",
            lsp_types::Position::new(0, 2),
            &label,
        );
        let text = match h.expect("expected hover on reserved word").contents {
            lsp_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(text.contains("Reserved word"), "got: {text}");
        assert!(
            text.contains("Declared in keyword (reserved)."),
            "got: {text}"
        );
    }

    #[test]
    fn hover_prelude_literals() {
        let src = "main():\n    x = None\n    y = true\n";
        let text = hover_text_at(src, 1, 9).expect("expected hover on None");
        assert!(text.contains("empty `Option`"), "got: {text}");
        assert!(text.contains("Declared in prelude."), "got: {text}");
        let text = hover_text_at(src, 2, 9).expect("expected hover on true");
        assert!(text.contains("Type: Bool"), "got: {text}");
    }
}
