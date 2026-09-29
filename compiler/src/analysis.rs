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
use crate::modules::ModuleGraph;
use crate::parser::parse;
use crate::resolver::resolve;
use crate::typeck::typecheck;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
///
/// Import-aware layer: the pipeline above is single-file, so flat
/// uses of linked modules surface here as `E0201 unknown identifier`
/// while `noct run` resolves them. When `file_path` names a real file
/// on disk, its module graph is consulted: E0201s for names the graph
/// exports are suppressed, and this file's graph diagnostics
/// (E0100–E0109/W0101, per-file spans) are surfaced. Hover,
/// go-to-definition, and completions stay single-file (a documented
/// limitation, not a silent gap).
pub fn analyze_file(file_path: impl AsRef<str>, source: &str) -> AnalysisResult {
    analyze_file_with_roots(file_path, source, &[])
}

/// [`analyze_file`] with explicit package roots for the module graph.
///
/// The graph lookup needs the same resolution roots the toolchain uses —
/// the content store, a committed `vendor/` tree, and manifest `path:`
/// dependencies. Without them the graph cannot resolve a declared
/// dependency, which produced two false results in the editor at once: an
/// `E0101 cannot resolve imported module` for an import that builds fine,
/// and a flood of `E0201 unknown identifier` squiggles, because the
/// `provided` set that suppresses those was built from a graph that never
/// loaded the dependency. `noct diagnostics` had the identical bug and the
/// identical fix; an editor that reports a clean build as broken is worse
/// than no editor at all.
///
/// `store_root` is deliberately not supplied by the compiler: the store
/// layout lives in the toolchain. A caller that knows it (the CLI) passes it
/// in; the LSP passes what it can derive from the manifest.
pub fn analyze_file_with_roots(
    file_path: impl AsRef<str>,
    source: &str,
    package_roots: &[std::path::PathBuf],
) -> AnalysisResult {
    let file_path = file_path.as_ref().to_string();
    let mut sink = DiagnosticSink::new();

    let tokens = lex(source, &mut sink);
    let ast = parse(&tokens, &mut sink);
    let resolved = resolve(ast.clone(), &mut sink);
    let typed = typecheck(resolved.clone(), &mut sink);

    let mut diagnostics = sink.take();
    apply_module_graph_diagnostics(&file_path, &ast, &mut diagnostics, package_roots);

    AnalysisResult {
        file_path,
        source: source.to_string(),
        diagnostics,
        ast,
        resolved,
        typed,
    }
}

/// Best-effort import awareness for single-file analysis (see
/// [`analyze_file`]). Never fails: any unresolvable path simply keeps
/// the single-file diagnostics unchanged.
fn apply_module_graph_diagnostics(
    file_path: &str,
    program: &Program,
    diagnostics: &mut Vec<Diagnostic>,
    package_roots: &[std::path::PathBuf],
) {
    let Some(disk) = uri_to_fs_path(file_path) else {
        return;
    };
    if !disk.is_file() {
        return;
    }
    let Ok(graph) = ModuleGraph::load_with(std::slice::from_ref(&disk), package_roots) else {
        return;
    };
    let canonical = disk.canonicalize().unwrap_or(disk);
    // Names the linked program provides: E0201s for these are an
    // artifact of single-file analysis, not real errors.
    let mut provided: HashSet<String> = graph
        .files
        .iter()
        .flat_map(|f| graph.exported_names(&f.path))
        .collect();
    // Alias (or module) names of imports that resolve: `v.foo()`
    // flags the alias object `v` in single-file analysis, but the
    // linker rewrites it to the flattened `foo()` before typeck —
    // so suppress the alias exactly when its import resolves.
    // Unresolved imports keep both their E0101 and the follow-on
    // E0201s (genuinely broken, must stay visible).
    if let Some(importer) = graph.files.iter().find(|f| f.path == canonical) {
        for import in &program.imports {
            if graph.resolve_import_decl(importer, import).is_some() {
                let name = import
                    .alias
                    .clone()
                    .or_else(|| import.path.last().cloned())
                    .unwrap_or_default();
                if !name.is_empty() {
                    provided.insert(name);
                }
            }
        }
    }
    if !provided.is_empty() {
        diagnostics.retain(|d| {
            if d.code.as_deref() != Some("E0201") {
                return true;
            }
            match ident_in_message(&d.message) {
                Some(name) => !provided.contains(name),
                None => true,
            }
        });
    }
    // This file's own graph diagnostics (deduped against identical
    // single-file ones, e.g. parse errors reported by both).
    let seen: HashSet<(Option<String>, String, usize, usize)> = diagnostics
        .iter()
        .map(|d| {
            (
                d.code.clone(),
                d.message.clone(),
                d.labels.first().map(|l| l.span.start).unwrap_or(0),
                d.labels.first().map(|l| l.span.end).unwrap_or(0),
            )
        })
        .collect();
    for d in graph.diagnostics_for(&canonical) {
        let key = (
            d.code.clone(),
            d.message.clone(),
            d.labels.first().map(|l| l.span.start).unwrap_or(0),
            d.labels.first().map(|l| l.span.end).unwrap_or(0),
        );
        if !seen.contains(&key) {
            diagnostics.push(d);
        }
    }
}

/// Extract the `` `name` `` from `unknown identifier \`name\``.
fn ident_in_message(message: &str) -> Option<&str> {
    let start = message.find('`')?;
    let rest = &message[start + 1..];
    let end = rest.find('`')?;
    Some(&rest[..end])
}

/// Map an LSP URI (or plain path) to a filesystem path.
/// Handles `file://` prefixes and `%XX` escapes; returns `None`
/// when the input cannot name a file.
pub fn uri_to_fs_path(uri: &str) -> Option<PathBuf> {
    let s = uri.strip_prefix("file://").unwrap_or(uri);
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    let mut it = s.as_bytes().iter();
    while let Some(&b) = it.next() {
        if b == b'%' {
            let hi = *it.next()?;
            let lo = *it.next()?;
            let hex = |c: u8| (c as char).to_digit(16);
            bytes.push(((hex(hi)? << 4) | hex(lo)?) as u8);
        } else {
            bytes.push(b);
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    // Windows `file:///C:/...` carries one leading slash too many;
    // Unix `/home/...` must keep its own.
    let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
    let path = if trimmed.len() >= 2
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && &trimmed[1..2] == ":"
    {
        PathBuf::from(trimmed)
    } else {
        PathBuf::from(&decoded)
    };
    Some(path)
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
/// Convert a compiler [`Diagnostic`] to an LSP [`Diagnostic`] for `uri`.
///
/// Secondary labels become `relatedInformation` pointing at the same
/// document (compiler labels are same-file — never `file://dummy`).
pub fn diagnostic_to_lsp(
    source: &str,
    uri: &lsp_types::Uri,
    diag: &Diagnostic,
) -> lsp_types::Diagnostic {
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
                            uri: uri.clone(),
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
/// Both line and character are 0-based. Character counts LSP UTF-16 code
/// units (spec §3.17), not bytes or scalar values: `é`/CJK are 1 unit,
/// emoji are 2. Byte/char counting desyncs every hover, goto, and
/// diagnostic on non-ASCII lines (LANGUAGE_SPEC.md §1 identifiers).
fn byte_offset_to_position(source: &str, offset: usize) -> BytePosition {
    let offset = offset.min(source.len());
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
    let character: usize = source[line_start..offset].chars().map(|c| c.len_utf16()).sum();
    BytePosition { line, character }
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
            // Pre-expansion only (the resolver consumes derives);
            // reachable via parse-only tooling.
            Item::Derive(d) => defs.push(Definition {
                name: d.target.clone(),
                kind: DefinitionKind::Function,
                span: d.span.clone(),
                detail: format!("derive {} for {}", d.trait_name, d.target),
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
            // `export` is visibility, not a definition, and neither are the
            // re-export forms: they forward names that are themselves
            // declared (and collected) in this program, so counting them
            // would double every public symbol in go-to-definition and
            // completions.
            Item::Export(_)
            | Item::ReExport(_)
            | Item::ExportAll(_)
            | Item::ExportAllExcept(_) => {}
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
    /// `let` / `var` / bare `name: value` local (noctivue-analyzer P0).
    Local,
    /// Function / task / local-fn parameter (noctivue-analyzer P0).
    Param,
    /// `for x in ...` binding (noctivue-analyzer P0).
    ForBinding,
    /// Match-pattern binding (`Some(v)`, `Ident`) (noctivue-analyzer P0).
    PatternBinding,
    /// `import path [as alias]` (noctivue-analyzer P0).
    Import,
    /// Struct field `Parent.field` (noctivue-analyzer members).
    Field,
    /// Enum variant `Parent::Variant` (bare in scope, noctivue-analyzer).
    Variant,
    /// Trait / impl method (noctivue-analyzer members).
    Method,
}

/// Name-only span for a definition: the identifier occurrence inside the
/// outer item/statement span (noctivue-analyzer P0).
///
/// The parser records whole-item spans (`fn` → end of body, `let` → end of
/// value). Editors must highlight/jump to the *name*, not the block —
/// TypeScript parity. Word-boundary search keeps `m` from matching inside
/// `mod` and works without parser surgery; falls back to the outer span
/// when the name cannot be located (synthesized derive output, recovery).
pub fn name_span_in(source: &str, outer: &Span, name: &str) -> Span {
    if name.is_empty() {
        return outer.clone();
    }
    let start = outer.start.min(source.len());
    let end = outer.end.min(source.len()).max(start);
    if start >= end {
        return outer.clone();
    }
    let hay = &source[start..end];
    let mut search_from = 0usize;
    while let Some(rel) = hay[search_from..].find(name) {
        let abs_start = start + search_from + rel;
        let abs_end = abs_start + name.len();
        let before_ok = if abs_start == 0 {
            true
        } else {
            !is_ident_char_at(source, abs_start - 1, false)
        };
        let after_ok = !is_ident_char_at(source, abs_end, true);
        if before_ok && after_ok {
            return Span {
                start: abs_start,
                end: abs_end,
            };
        }
        search_from += rel + 1;
        if search_from >= hay.len() {
            break;
        }
    }
    outer.clone()
}

fn is_ident_char_at(source: &str, byte_idx: usize, forward: bool) -> bool {
    if forward {
        if let Some(ch) = source.get(byte_idx..).and_then(|s| s.chars().next()) {
            return ch.is_alphanumeric() || ch == '_';
        }
        return false;
    }
    // Backward: find the char ending at byte_idx+1.
    if byte_idx >= source.len() {
        return false;
    }
    let mut last: Option<char> = None;
    for (i, ch) in source.char_indices() {
        if i > byte_idx {
            break;
        }
        if i + ch.len_utf8() - 1 == byte_idx || i == byte_idx {
            last = Some(ch);
        }
    }
    // Simpler fallback: byte-level check covers ASCII idents; non-ASCII
    // idents are handled by the forward path + find() alignment.
    match last {
        Some(ch) => ch.is_alphanumeric() || ch == '_',
        None => {
            let b = source.as_bytes().get(byte_idx).copied().unwrap_or(b' ');
            b.is_ascii_alphanumeric() || b == b'_'
        }
    }
}

/// Precise goto target for an already-resolved [`Definition`].
/// Returns the name-only span; falls back to the full span.
pub fn definition_name_span(source: &str, def: &Definition) -> Span {
    // Locals/params/patterns/fields/variants/methods/imports already
    // store name-only spans.
    match def.kind {
        DefinitionKind::Local
        | DefinitionKind::Param
        | DefinitionKind::ForBinding
        | DefinitionKind::PatternBinding
        | DefinitionKind::Import
        | DefinitionKind::Field
        | DefinitionKind::Variant
        | DefinitionKind::Method => def.span.clone(),
        _ => name_span_in(source, &def.span, &def.name),
    }
}

/// Filesystem path → `file://` URI (forward slashes; `C:\` → `file:///C:/`).
pub fn fs_path_to_uri(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    // UNC / already-absolute-Unix stay as-is; drive letters need the
    // extra slash (`file:///C:/...`).
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        s = format!("/{s}");
    } else if !s.starts_with('/') {
        s = format!("/{s}");
    }
    format!("file://{s}")
}

/// Identifier under the cursor as a string (`None` on whitespace/unknown).
pub fn identifier_at(source: &str, position: lsp_types::Position) -> Option<String> {
    let offset = position_to_byte_offset(source, position);
    extract_identifier_at(source, offset)
}

/// Parse + resolve a source text into a [`Program`] for cross-file target
/// lookup. Diagnostics are discarded: the target file parsed (or it didn't
/// — `None` on lex failure is not useful here since `lex` never fails hard;
/// `parse` may produce an empty program, which simply yields no item).
pub fn parse_resolved(source: &str) -> Program {
    let mut sink = DiagnosticSink::new();
    let tokens = lex(source, &mut sink);
    let ast = parse(&tokens, &mut sink);
    resolve(ast, &mut sink)
}

/// Top-level item lookup by name in an already-parsed program.
/// Returns `(kind, full_span, detail)`; the caller narrows with `name_span_in`.
pub fn find_item_span(program: &Program, name: &str) -> Option<(DefinitionKind, Span, String)> {
    for item in &program.items {
        // `export` is visibility, not identity: an exported item IS the
        // item for lookup purposes. Without this every cross-file lookup
        // (goto and hover alike) went blind on exactly the items a
        // dependency exists to provide.
        let item = match item {
            Item::Export(inner) => inner.as_ref(),
            other => other,
        };
        match item {
            Item::Struct(s) if s.name == name => {
                return Some((
                    DefinitionKind::Struct,
                    s.span.clone(),
                    format!("struct {}", s.name),
                ))
            }
            Item::Function(f) if f.name == name => {
                return Some((
                    DefinitionKind::Function,
                    f.span.clone(),
                    format!("fn {}(...)", f.name),
                ))
            }
            Item::Task(t) if t.name == name => {
                return Some((
                    DefinitionKind::Function,
                    t.span.clone(),
                    format!("task {}(...)", t.name),
                ))
            }
            Item::Enum(e) if e.name == name => {
                return Some((
                    DefinitionKind::Enum,
                    e.span.clone(),
                    format!("enum {}", e.name),
                ))
            }
            Item::Const(c) if c.name == name => {
                return Some((
                    DefinitionKind::Const,
                    c.span.clone(),
                    format!(
                        "const {}: {}",
                        c.name,
                        crate::ast::type_expr_to_string(&c.ty)
                    ),
                ))
            }
            Item::Trait(t) if t.name == name => {
                return Some((
                    DefinitionKind::Trait,
                    t.span.clone(),
                    format!("trait {}", t.name),
                ))
            }
            Item::Mod(m) if m.name == name => {
                return Some((
                    DefinitionKind::Module,
                    m.span.clone(),
                    format!("mod {}", m.name),
                ))
            }
            Item::BareDecl(d) if d.name == name => {
                return Some((
                    DefinitionKind::Component,
                    d.span.clone(),
                    format!("component {}", d.name),
                ))
            }
            _ => {}
        }
        // Bare variants live beside top-level items for cross-file flat uses.
        if let Item::Enum(e) = item {
            if e.variants.iter().any(|v| v.name == name) {
                return Some((
                    DefinitionKind::Variant,
                    e.span.clone(),
                    format!("variant {}::{name}", e.name),
                ));
            }
        }
    }
    None
}

/// Deepest HIR expression type at `position` (for go-to-type-definition).
/// Walks the full HIR; the tightest containing expression wins so
/// `user.name` reports the field type, not the struct.
pub fn find_type_at(
    typed: &Module,
    source: &str,
    position: lsp_types::Position,
) -> Option<crate::hir::types::Ty> {
    use crate::hir::items::{TypedExpr, TypedStmt, TypedStmtKind};
    let offset = position_to_byte_offset(source, position);
    fn in_expr(expr: &TypedExpr, offset: usize, best: &mut Option<(usize, crate::hir::types::Ty)>) {
        if !span_contains(&expr.span, offset) {
            return;
        }
        let deeper = best.as_ref().is_none_or(|(s, _)| expr.span.start >= *s);
        if deeper {
            *best = Some((expr.span.start, expr.ty.clone()));
        }
        match &expr.kind {
            crate::hir::items::TypedExprKind::Call { callee, args } => {
                in_expr(callee, offset, best);
                for a in args {
                    in_expr(a, offset, best);
                }
            }
            crate::hir::items::TypedExprKind::BinOp { left, right, .. } => {
                in_expr(left, offset, best);
                in_expr(right, offset, best);
            }
            crate::hir::items::TypedExprKind::UnaryOp { operand, .. } => {
                in_expr(operand, offset, best)
            }
            crate::hir::items::TypedExprKind::Member { object, .. } => {
                in_expr(object, offset, best)
            }
            crate::hir::items::TypedExprKind::Try(inner) => in_expr(inner, offset, best),
            crate::hir::items::TypedExprKind::Coalesce { left, right } => {
                in_expr(left, offset, best);
                in_expr(right, offset, best);
            }
            crate::hir::items::TypedExprKind::StringInterp(parts) => {
                for p in parts {
                    if let crate::hir::items::TypedInterpPart::Expr(e) = p {
                        in_expr(e, offset, best);
                    }
                }
            }
            crate::hir::items::TypedExprKind::Tuple(es)
            | crate::hir::items::TypedExprKind::List(es) => {
                for e in es {
                    in_expr(e, offset, best);
                }
            }
            crate::hir::items::TypedExprKind::Index { object, index } => {
                in_expr(object, offset, best);
                in_expr(index, offset, best);
            }
            crate::hir::items::TypedExprKind::Range { start, end, .. } => {
                in_expr(start, offset, best);
                in_expr(end, offset, best);
            }
            crate::hir::items::TypedExprKind::IfExpr {
                condition,
                then_expr,
                else_expr,
            } => {
                in_expr(condition, offset, best);
                in_expr(then_expr, offset, best);
                if let Some(e) = else_expr {
                    in_expr(e, offset, best);
                }
            }
            crate::hir::items::TypedExprKind::MatchExpr { scrutinee, arms } => {
                in_expr(scrutinee, offset, best);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        in_expr(g, offset, best);
                    }
                    for s in &arm.body {
                        in_stmt(s, offset, best);
                    }
                }
            }
            crate::hir::items::TypedExprKind::StructLit { fields, .. } => {
                for (_, e) in fields {
                    in_expr(e, offset, best);
                }
            }
            crate::hir::items::TypedExprKind::Spread(inner) => in_expr(inner, offset, best),
            crate::hir::items::TypedExprKind::Closure { body, .. } => {
                in_expr(body, offset, best)
            }
            crate::hir::items::TypedExprKind::Some(inner)
            | crate::hir::items::TypedExprKind::Ok(inner)
            | crate::hir::items::TypedExprKind::Err(inner) => in_expr(inner, offset, best),
            _ => {}
        }
    }
    fn in_stmt(stmt: &TypedStmt, offset: usize, best: &mut Option<(usize, crate::hir::types::Ty)>) {
        match &stmt.kind {
            TypedStmtKind::Let { value, .. }
            | TypedStmtKind::Var { value, .. }
            | TypedStmtKind::Decl { value, .. } => in_expr(value, offset, best),
            TypedStmtKind::Expr(e) => in_expr(e, offset, best),
            TypedStmtKind::Return(e) => {
                if let Some(e) = e {
                    in_expr(e, offset, best);
                }
            }
            TypedStmtKind::Assign { target, value } => {
                in_expr(target, offset, best);
                in_expr(value, offset, best);
            }
            TypedStmtKind::If {
                condition,
                then_body,
                else_body,
            } => {
                in_expr(condition, offset, best);
                for s in then_body {
                    in_stmt(s, offset, best);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        in_stmt(s, offset, best);
                    }
                }
            }
            TypedStmtKind::While { body, .. } | TypedStmtKind::Loop { body } => {
                for s in body {
                    in_stmt(s, offset, best);
                }
            }
            TypedStmtKind::For { iterable, body, .. } => {
                in_expr(iterable, offset, best);
                for s in body {
                    in_stmt(s, offset, best);
                }
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                in_expr(scrutinee, offset, best);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        in_expr(g, offset, best);
                    }
                    for s in &arm.body {
                        in_stmt(s, offset, best);
                    }
                }
            }
            TypedStmtKind::Break(e) => {
                if let Some(e) = e {
                    in_expr(e, offset, best);
                }
            }
            TypedStmtKind::Continue => {}
        }
    }
    let mut best = None;
    for func in &typed.functions {
        for stmt in &func.body {
            in_stmt(stmt, offset, &mut best);
        }
    }
    best.map(|(_, ty)| ty)
}

/// Nominal type name for go-to-type-definition (`Option<User>` → `User`).
/// `Unknown`/`Error`/fn-types yield `None` rather than a wrong jump.
pub fn nominal_name_of(ty: &crate::hir::types::Ty) -> Option<String> {
    use crate::hir::types::Ty;
    match ty {
        Ty::Named(name, _) => Some(name.clone()),
        Ty::Option(inner) | Ty::List(inner) => nominal_name_of(inner),
        Ty::Result(ok, _) => nominal_name_of(ok),
        Ty::Tuple(_) | Ty::Fn(_, _) | Ty::Unit | Ty::Unknown | Ty::Error => None,
        Ty::Int
        | Ty::UInt
        | Ty::Float
        | Ty::Bool
        | Ty::Char
        | Ty::String => None,
    }
}

/// Trait context for go-to-implementation (noctivue-analyzer).
/// Returns `(trait_name, method_or_none)`:
/// - cursor on a trait member def → `(Trait, Some(method))`;
/// - cursor on a trait name → `(Trait, None)` (all implementors);
/// - cursor on a bare method name owned by exactly one trait → that trait.
pub fn find_trait_context(
    resolved: &Program,
    source: &str,
    position: lsp_types::Position,
    ident: &str,
) -> Option<(String, Option<String>)> {
    let offset = position_to_byte_offset(source, position);
    for item in &resolved.items {
        match item {
            Item::Trait(t) => {
                if t.name == ident {
                    return Some((t.name.clone(), None));
                }
                for m in &t.members {
                    if m.name == ident && span_contains(&m.span, offset) {
                        return Some((t.name.clone(), Some(m.name.clone())));
                    }
                }
                // Cursor elsewhere inside the trait block on a member name.
                if span_contains(&t.span, offset)
                    && t.members.iter().any(|m| m.name == ident)
                {
                    return Some((t.name.clone(), Some(ident.to_string())));
                }
            }
            Item::Impl(b) => {
                // Cursor on an impl method → its trait (if any).
                if let Some(crate::ast::TypeExpr::Named(trait_name, _, _)) = &b.for_trait {
                    for m in &b.methods {
                        if m.name == ident && span_contains(&m.span, offset) {
                            return Some((trait_name.clone(), Some(m.name.clone())));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Bare method name owned by exactly one trait (method use site).
    let mut owners = Vec::new();
    for item in &resolved.items {
        if let Item::Trait(t) = item {
            if t.members.iter().any(|m| m.name == ident) {
                owners.push(t.name.clone());
            }
        }
    }
    if owners.len() == 1 {
        return Some((owners.remove(0), Some(ident.to_string())));
    }
    None
}

/// Name spans of impl methods for `trait_name` in `program`.
/// `method=None` collects every method of every matching impl block.
pub fn collect_impl_spans(
    program: &Program,
    source: &str,
    trait_name: &str,
    method: Option<&str>,
) -> Vec<Span> {
    let mut out = Vec::new();
    for item in &program.items {
        if let Item::Impl(b) = item {
            let matches = match &b.for_trait {
                Some(crate::ast::TypeExpr::Named(name, _, _)) => name == trait_name,
                _ => false,
            };
            if !matches {
                continue;
            }
            for m in &b.methods {
                if method.is_none_or(|w| w == m.name) {
                    out.push(name_span_in(source, &m.span, &m.name));
                }
            }
        }
    }
    out
}

/// Cross-file target for `ident` used in the file at `current_path`.
///
/// Loads the module graph (`vendor/` + manifest `path:` roots + stdlib via
/// the graph defaults) and returns `(providing_file, item_name)`:
/// - an import bound to `ident` resolves through `resolve_import_decl`
///   (`import ui::widget` used as `widget` → `ui/widget.nv`, item `widget`);
/// - otherwise the first graph file (other than the current one) exporting
///   `ident` wins (flat `helper()` from a `path:` dep).
/// Same-file hits are NOT reported here — single-file goto owns those.
/// Hover for a name that lives in ANOTHER file: an import alias, a
/// qualified module member, or a flat use of a dependency's export.
///
/// Single-file hover (`get_hover`) returns `None` for these by
/// construction — the definition is not in `resolved` — so without this
/// every imported name hovered to nothing. The card names the defining
/// file explicitly (`Declared in widget.nv`), because "fn text(...)"
/// without a location is indistinguishable from a local, and the whole
/// point of hovering an import is to learn where it comes from. Doc
/// comments are read from the TARGET file, not the current one.
///
/// Returns `None` when the name is local (single-file hover owns those),
/// unresolvable, or unreadable — this is a fallback, and a fallback that
/// guesses is worse than silence.
pub fn hover_cross_file_at(
    current_uri: &str,
    program: &Program,
    source: &str,
    position: lsp_types::Position,
    package_roots: &[PathBuf],
) -> Option<lsp_types::Hover> {
    let disk = uri_to_fs_path(current_uri)?;
    let offset = position_to_byte_offset(source, position);
    let ident = extract_identifier_at(source, offset)?;
    // Local first: a shadowing local owns the name, and this function
    // must not steal it. (Single-file hover already answered, but this
    // is also called where it has not, so check again rather than assume
    // the caller did.)
    if find_local_definition_at(program, source, offset, &ident).is_some() {
        return None;
    }
    let (target_path, item_name) = find_cross_file_target(&disk, program, &ident, package_roots)?;
    let name = item_name?;
    let target_source = std::fs::read_to_string(&target_path).ok()?;
    let mut sink = DiagnosticSink::new();
    let target_tokens = lex(&target_source, &mut sink);
    let target_program = parse(&target_tokens, &mut sink);
    let (kind, span, detail) = find_item_span(&target_program, &name)?;
    let _ = kind;
    let docs = doc_comment_for(&target_source, span.start);
    // Name the defining file with its parent directory, not just the
    // file name: dependencies conventionally expose `lib/main.nv`, so a
    // bare "main.nv" is ambiguous exactly when it matters most — when
    // the current file is ALSO called `main.nv`.
    let file_label = target_path
        .parent()
        .and_then(|d| d.file_name())
        .map(|d| {
            format!(
                "{}/{}",
                d.to_string_lossy(),
                target_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| target_path.display().to_string());
    let range = identifier_range_at(source, offset, &ident);
    Some(render_hover(
        HoverInfo {
            signature: detail,
            ty: None,
            declared_in: format!("{file_label} (imported)"),
            docs,
        },
        range,
    ))
}

/// The LSP range of the identifier under the cursor: its byte span in the
/// CURRENT file, so the editor underlines the use, not the definition.
fn identifier_range_at(
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<lsp_types::Range> {
    let start = source[..offset.min(source.len())]
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map(|i| i + 1)
        .unwrap_or(0);
    // `ident` was extracted at this offset, so it must start here; a
    // mismatch means the text moved under us, and no range beats a wrong
    // one.
    if !source[start..].starts_with(ident) {
        return None;
    }
    let end = start + ident.len();
    let s = byte_offset_to_position(source, start);
    let e = byte_offset_to_position(source, end);
    Some(lsp_types::Range {
        start: lsp_types::Position::new(s.line as u32, s.character as u32),
        end: lsp_types::Position::new(e.line as u32, e.character as u32),
    })
}

pub fn find_cross_file_target(
    current_path: &Path,
    program: &Program,
    ident: &str,
    package_roots: &[PathBuf],
) -> Option<(PathBuf, Option<String>)> {
    let graph =
        ModuleGraph::load_with(&[current_path.to_path_buf()], package_roots).ok()?;
    let canonical = current_path.canonicalize().unwrap_or_else(|_| current_path.to_path_buf());
    let importer = graph.files.iter().find(|f| f.path == canonical)?;
    // 1. Import decl bound to this ident.
    for import in &program.imports {
        let bound = import
            .alias
            .clone()
            .or_else(|| import.path.last().cloned())
            .unwrap_or_default();
        if bound == ident {
            if let Some((target, item)) = graph.resolve_import_decl(importer, import) {
                return Some((target, item));
            }
        }
    }
    // 2. Flat use of an export from another file.
    let std_root = crate::modules::stdlib_root();
    let mut providers: Vec<(PathBuf, bool)> = Vec::new();
    for f in &graph.files {
        if f.path == canonical {
            continue;
        }
        if graph.exported_names(&f.path).iter().any(|n| n == ident) {
            // Prefer real project files over stdlib for hover noise;
            // goto still jumps when stdlib is the only provider.
            let is_stdlib = std_root
                .as_ref()
                .is_some_and(|r| f.path.starts_with(r));
            providers.push((f.path.clone(), is_stdlib));
        }
    }
    providers.sort_by_key(|(_, std)| *std);
    providers.into_iter().next().map(|(p, _)| (p, Some(ident.to_string())))
}

/// Find the definition for the identifier under the cursor
/// (usage → definition, noctivue-analyzer).
///
/// Resolution order: member-field / struct-literal label (syntactically
/// qualified, beat shadowing) → scope-aware locals/params → bare enum
/// variants → field/variant/method *definitions* at the cursor → top-level
/// items → import aliases. Unknown/whitespace → `None`.
///
/// `typed` is required for member access (`user.name` needs `user: User`);
/// without it member uses degrade to `None` rather than a wrong guess.
pub fn find_definition_at(
    resolved: &Program,
    typed: &Module,
    source: &str,
    position: lsp_types::Position,
) -> Option<Definition> {
    let offset = position_to_byte_offset(source, position);
    let ident = extract_identifier_at(source, offset)?;
    // 1. Member-field use (`user.name`, `response.body`, `t.id`).
    if let Some(def) = find_member_definition_at(resolved, typed, source, offset, &ident) {
        return Some(def);
    }
    // 2. Struct-literal label (`User { name: ... }` — label, not value).
    if let Some(def) = find_struct_label_definition_at(resolved, source, offset, &ident) {
        return Some(def);
    }
    // 3. Locals / params / pattern bindings (shadow top-level).
    if let Some(local) = find_local_definition_at(resolved, source, offset, &ident) {
        return Some(local);
    }
    // 4. Bare enum variants (`North` — in scope without prefix, m0_demo).
    if let Some(def) = find_variant_definition(resolved, source, &ident) {
        return Some(def);
    }
    // 5. Field / variant / method definition at the cursor.
    if let Some(def) = find_member_def_at_cursor(resolved, source, offset, &ident) {
        return Some(def);
    }
    // 2. Top-level items by name (usage → def, not just cursor-in-def).
    for item in &resolved.items {
        let (name, kind, span, detail) = match item {
            Item::Struct(s) if s.name == ident => (
                s.name.clone(),
                DefinitionKind::Struct,
                s.span.clone(),
                format!("struct {}", s.name),
            ),
            Item::Function(f) if f.name == ident => (
                f.name.clone(),
                DefinitionKind::Function,
                f.span.clone(),
                format!("fn {}(...)", f.name),
            ),
            Item::Enum(e) if e.name == ident => (
                e.name.clone(),
                DefinitionKind::Enum,
                e.span.clone(),
                format!("enum {}", e.name),
            ),
            Item::Const(c) if c.name == ident => (
                c.name.clone(),
                DefinitionKind::Const,
                c.span.clone(),
                format!(
                    "const {}: {}",
                    c.name,
                    crate::ast::type_expr_to_string(&c.ty)
                ),
            ),
            Item::Trait(t) if t.name == ident => (
                t.name.clone(),
                DefinitionKind::Trait,
                t.span.clone(),
                format!("trait {}", t.name),
            ),
            Item::BareDecl(d) if d.name == ident => (
                d.name.clone(),
                DefinitionKind::Component,
                d.span.clone(),
                format!("component {}", d.name),
            ),
            Item::Mod(m) if m.name == ident => (
                m.name.clone(),
                DefinitionKind::Module,
                m.span.clone(),
                format!("mod {}", m.name),
            ),
            Item::Task(t) if t.name == ident => (
                t.name.clone(),
                DefinitionKind::Function,
                t.span.clone(),
                format!("task {}(...)", t.name),
            ),
            _ => continue,
        };
        let name_span = name_span_in(source, &span, &name);
        return Some(Definition {
            name,
            kind,
            span: name_span,
            detail,
        });
    }
    // 3. Import aliases / module paths (`import dep`, `import a::b as c`).
    for import in &resolved.imports {
        let bound = import
            .alias
            .clone()
            .or_else(|| import.path.last().cloned())
            .unwrap_or_default();
        if bound == ident {
            let name_span = name_span_in(source, &import.span, &ident);
            return Some(Definition {
                name: bound.clone(),
                kind: DefinitionKind::Import,
                span: name_span,
                detail: format!("import {}", import.path.join("::")),
            });
        }
    }
    None
}

/// Struct-field declaration lookup: `(parent, FieldDecl)`.
fn find_struct_field_decl<'a>(
    resolved: &'a Program,
    struct_name: &str,
    field_name: &str,
) -> Option<(&'a str, &'a crate::ast::FieldDecl)> {
    for item in &resolved.items {
        let (name, fields) = match item {
            Item::Struct(s) if s.name == struct_name => (s.name.as_str(), &s.fields),
            _ => continue,
        };
        if let Some(f) = fields.iter().find(|f| f.name == field_name) {
            return Some((name, f));
        }
    }
    None
}

/// Bare-variant lookup across all enums: `(enum_name, variant)`.
fn find_variant_definition(
    resolved: &Program,
    source: &str,
    ident: &str,
) -> Option<Definition> {
    for item in &resolved.items {
        if let Item::Enum(e) = item {
            if let Some(v) = e.variants.iter().find(|v| v.name == ident) {
                let payload = if v.fields.is_empty() {
                    String::new()
                } else {
                    let parts: Vec<String> =
                        v.fields.iter().map(crate::ast::type_expr_to_string).collect();
                    format!("({})", parts.join(", "))
                };
                return Some(Definition {
                    name: v.name.clone(),
                    kind: DefinitionKind::Variant,
                    span: name_span_in(source, &v.span, &v.name),
                    detail: format!("variant {}::{}{}", e.name, v.name, payload),
                });
            }
        }
    }
    None
}

/// Field / variant / trait-method / impl-method *definition* under the
/// cursor (e.g. `name` in `name: String` inside `struct User:`).
fn find_member_def_at_cursor(
    resolved: &Program,
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<Definition> {
    for item in &resolved.items {
        match item {
            Item::Struct(s) => {
                for f in &s.fields {
                    let ns = name_span_in(source, &f.span, &f.name);
                    if f.name == ident && span_contains(&ns, offset) {
                        return Some(Definition {
                            name: f.name.clone(),
                            kind: DefinitionKind::Field,
                            span: ns,
                            detail: format!(
                                "field {}.{}: {}",
                                s.name,
                                f.name,
                                crate::ast::type_expr_to_string(&f.ty)
                            ),
                        });
                    }
                }
            }
            Item::Enum(e) => {
                for v in &e.variants {
                    let ns = name_span_in(source, &v.span, &v.name);
                    if v.name == ident && span_contains(&ns, offset) {
                        return Some(Definition {
                            name: v.name.clone(),
                            kind: DefinitionKind::Variant,
                            span: ns,
                            detail: format!("variant {}::{}", e.name, v.name),
                        });
                    }
                }
            }
            Item::Trait(t) => {
                for m in &t.members {
                    if m.name == ident && span_contains(&m.span, offset) {
                        return Some(Definition {
                            name: m.name.clone(),
                            kind: DefinitionKind::Method,
                            span: name_span_in(source, &m.span, &m.name),
                            detail: format!("method {}.{}", t.name, m.name),
                        });
                    }
                }
            }
            Item::Impl(b) => {
                for m in &b.methods {
                    if m.name == ident && span_contains(&m.span, offset) {
                        return Some(Definition {
                            name: m.name.clone(),
                            kind: DefinitionKind::Method,
                            span: name_span_in(source, &m.span, &m.name),
                            detail: format!("method {}", m.name),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Member-field use (`user.name`): resolve the object type via HIR, then
/// jump to the struct's `FieldDecl`. Cursor must sit on the *field* part
/// (after the dot), never the object.
fn find_member_definition_at(
    resolved: &Program,
    typed: &Module,
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<Definition> {
    let (obj_ty, field, _member_span) = find_hir_member_at(typed, source, offset, ident)?;
    let struct_name = match &obj_ty {
        crate::hir::types::Ty::Named(name, _) => name.clone(),
        _ => return None,
    };
    let (parent, decl) = find_struct_field_decl(resolved, &struct_name, &field)?;
    Some(Definition {
        name: decl.name.clone(),
        kind: DefinitionKind::Field,
        span: name_span_in(source, &decl.span, &decl.name),
        detail: format!(
            "field {parent}.{}: {}",
            decl.name,
            crate::ast::type_expr_to_string(&decl.ty)
        ),
    })
}

/// Struct-literal label (`User { name: ... }`): the label before `:` maps
/// to the struct's `FieldDecl`. Values after the colon resolve as normal
/// expressions (locals), never as fields.
fn find_struct_label_definition_at(
    resolved: &Program,
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<Definition> {
    use crate::ast::{Expr, Item, Stmt};
    // Label heuristic: next non-blank char after the token is `:`.
    let tok_end = token_end_at(source, offset)?;
    let mut rest = tok_end;
    while rest < source.len() && matches!(source.as_bytes().get(rest), Some(b' ' | b'\t')) {
        rest += 1;
    }
    if source.as_bytes().get(rest) != Some(&b':') {
        return None;
    }
    // Enclosing struct literal determines the parent type.
    let lit_name = find_enclosing_struct_literal(resolved, source, offset)?;
    let (parent, decl) = find_struct_field_decl(resolved, &lit_name, ident)?;
    // Confirm the cursor is on a label occurrence, not a coincidental
    // `name` elsewhere inside the literal span.
    let _ = parent;
    Some(Definition {
        name: decl.name.clone(),
        kind: DefinitionKind::Field,
        span: name_span_in(source, &decl.span, &decl.name),
        detail: format!(
            "field {parent}.{}: {}",
            decl.name,
            crate::ast::type_expr_to_string(&decl.ty)
        ),
    })
}

/// Walk top-level const values + function bodies for the innermost
/// `StructLit` containing `offset`; returns its type name.
fn find_enclosing_struct_literal(
    resolved: &Program,
    _source: &str,
    offset: usize,
) -> Option<String> {
    use crate::ast::{Expr, FunctionBody, Item, Stmt};
    fn in_expr(expr: &Expr, offset: usize, best: &mut Option<(usize, String)>) {
        let span = expr.span();
        if !span_contains(&span, offset) {
            return;
        }
        if let Expr::StructLit(lit) = expr {
            // Deeper (larger start) literals win.
            let deeper = best.as_ref().is_none_or(|(s, _)| span.start >= *s);
            if deeper {
                *best = Some((span.start, lit.name.clone()));
            }
        }
        match expr {
            Expr::Call(c) => {
                in_expr(&c.callee, offset, best);
                for a in &c.args {
                    in_expr(&a.value, offset, best);
                }
            }
            Expr::Member(m) => in_expr(&m.object, offset, best),
            Expr::Index(i) => {
                in_expr(&i.object, offset, best);
                in_expr(&i.index, offset, best);
            }
            Expr::BinOp(b) => {
                in_expr(&b.left, offset, best);
                in_expr(&b.right, offset, best);
            }
            Expr::UnaryOp(u) => in_expr(&u.operand, offset, best),
            Expr::Try(t) => in_expr(&t.expr, offset, best),
            Expr::Range(r) => {
                in_expr(&r.start, offset, best);
                in_expr(&r.end, offset, best);
            }
            Expr::StringInterp(s) => {
                for p in &s.parts {
                    if let crate::ast::InterpPart::Expr(e) = p {
                        in_expr(e, offset, best);
                    }
                }
            }
            Expr::StructLit(lit) => {
                for f in &lit.fields {
                    if let crate::ast::StructField::Named(_, e) = f {
                        in_expr(e, offset, best);
                    }
                }
            }
            Expr::ListLit(l) => {
                for e in &l.elements {
                    in_expr(e, offset, best);
                }
            }
            Expr::Closure(c) => in_expr(&c.body, offset, best),
            Expr::Spread(s) => in_expr(&s.expr, offset, best),
            _ => {}
        }
    }
    fn in_block(block: &crate::ast::Block, offset: usize, best: &mut Option<(usize, String)>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let(s) => in_expr(&s.value, offset, best),
                Stmt::Var(s) => in_expr(&s.value, offset, best),
                Stmt::Decl(s) => in_expr(&s.value, offset, best),
                Stmt::Assign(s) => {
                    in_expr(&s.target, offset, best);
                    in_expr(&s.value, offset, best);
                }
                Stmt::Expr(e) => in_expr(e, offset, best),
                Stmt::Return(r) => {
                    if let Some(e) = &r.value {
                        in_expr(e, offset, best);
                    }
                }
                Stmt::If(s) => {
                    in_expr(&s.condition, offset, best);
                    in_block(&s.then_block, offset, best);
                    for (_, b) in &s.else_if_clauses {
                        in_block(b, offset, best);
                    }
                    if let Some(b) = &s.else_block {
                        in_block(b, offset, best);
                    }
                }
                Stmt::While(s) => {
                    in_expr(&s.condition, offset, best);
                    in_block(&s.body, offset, best);
                }
                Stmt::Loop(s) => in_block(&s.body, offset, best),
                Stmt::For(s) => {
                    in_expr(&s.iterable, offset, best);
                    in_block(&s.body, offset, best);
                }
                Stmt::Match(s) => {
                    in_expr(&s.scrutinee, offset, best);
                    for arm in &s.arms {
                        if let Some(g) = &arm.guard {
                            in_expr(g, offset, best);
                        }
                        match &arm.body {
                            crate::ast::MatchBody::Block(b) => in_block(b, offset, best),
                            crate::ast::MatchBody::Expr(e) => in_expr(e, offset, best),
                        }
                    }
                }
                Stmt::Function(f) => match &f.body {
                    FunctionBody::Block(b) => in_block(b, offset, best),
                    FunctionBody::Expr(e) => in_expr(e, offset, best),
                },
                _ => {}
            }
        }
    }
    let mut best: Option<(usize, String)> = None;
    for item in &resolved.items {
        match item {
            Item::Const(c) => in_expr(&c.value, offset, &mut best),
            Item::Function(f) => match &f.body {
                FunctionBody::Block(b) => in_block(b, offset, &mut best),
                FunctionBody::Expr(e) => in_expr(e, offset, &mut best),
            },
            _ => {}
        }
    }
    best.map(|(_, n)| n)
}

/// End byte offset of the identifier token containing `offset`.
fn token_end_at(source: &str, offset: usize) -> Option<usize> {
    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }
    let mut end = None;
    for (i, ch) in source.char_indices() {
        let next = i + ch.len_utf8();
        if i <= offset && offset < next {
            if !is_word(ch) {
                return None;
            }
            end = Some(next);
            break;
        }
    }
    let mut end = end?;
    while end < source.len() {
        match source[end..].chars().next() {
            Some(ch) if is_word(ch) => end += ch.len_utf8(),
            _ => break,
        }
    }
    Some(end)
}

/// Deepest HIR `Member` at `offset` whose field is `ident` with the cursor
/// on the field part (after the dot). Returns `(object_ty, field, span)`.
fn find_hir_member_at(
    typed: &Module,
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<(crate::hir::types::Ty, String, Span)> {
    use crate::hir::items::{TypedExpr, TypedExprKind, TypedStmt, TypedStmtKind};
    fn field_start(source: &str, member_span: &Span, field: &str) -> Option<usize> {
        if field.is_empty() || member_span.end < field.len() {
            return None;
        }
        let start = member_span.end.saturating_sub(field.len()).min(source.len());
        if source.get(start..start + field.len()) == Some(field) {
            Some(start)
        } else {
            // Fallback: last word-bounded occurrence inside the span.
            let ns = name_span_in(source, member_span, field);
            Some(ns.start)
        }
    }
    fn in_expr(
        expr: &TypedExpr,
        source: &str,
        offset: usize,
        ident: &str,
        best: &mut Option<(crate::hir::types::Ty, String, Span, usize)>,
    ) {
        if !span_contains(&expr.span, offset) {
            return;
        }
        if let TypedExprKind::Member { object, field } = &expr.kind {
            if field == ident {
                if let Some(fs) = field_start(source, &expr.span, field) {
                    if offset >= fs {
                        let deeper = best.as_ref().is_none_or(|(_, _, _, s)| expr.span.start >= *s);
                        if deeper {
                            *best = Some((object.ty.clone(), field.clone(), expr.span.clone(), expr.span.start));
                        }
                    }
                }
            }
        }
        match &expr.kind {
            TypedExprKind::Call { callee, args } => {
                in_expr(callee, source, offset, ident, best);
                for a in args {
                    in_expr(a, source, offset, ident, best);
                }
            }
            TypedExprKind::BinOp { left, right, .. } => {
                in_expr(left, source, offset, ident, best);
                in_expr(right, source, offset, ident, best);
            }
            TypedExprKind::UnaryOp { operand, .. } => in_expr(operand, source, offset, ident, best),
            TypedExprKind::Member { object, .. } => in_expr(object, source, offset, ident, best),
            TypedExprKind::Try(inner) => in_expr(inner, source, offset, ident, best),
            TypedExprKind::Coalesce { left, right } => {
                in_expr(left, source, offset, ident, best);
                in_expr(right, source, offset, ident, best);
            }
            TypedExprKind::StringInterp(parts) => {
                for p in parts {
                    if let crate::hir::items::TypedInterpPart::Expr(e) = p {
                        in_expr(e, source, offset, ident, best);
                    }
                }
            }
            TypedExprKind::Tuple(es) | TypedExprKind::List(es) => {
                for e in es {
                    in_expr(e, source, offset, ident, best);
                }
            }
            TypedExprKind::Index { object, index } => {
                in_expr(object, source, offset, ident, best);
                in_expr(index, source, offset, ident, best);
            }
            TypedExprKind::Range { start, end, .. } => {
                in_expr(start, source, offset, ident, best);
                in_expr(end, source, offset, ident, best);
            }
            TypedExprKind::IfExpr {
                condition,
                then_expr,
                else_expr,
            } => {
                in_expr(condition, source, offset, ident, best);
                in_expr(then_expr, source, offset, ident, best);
                if let Some(e) = else_expr {
                    in_expr(e, source, offset, ident, best);
                }
            }
            TypedExprKind::MatchExpr { scrutinee, arms } => {
                in_expr(scrutinee, source, offset, ident, best);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        in_expr(g, source, offset, ident, best);
                    }
                    for s in &arm.body {
                        in_stmt(s, source, offset, ident, best);
                    }
                }
            }
            TypedExprKind::StructLit { fields, .. } => {
                for (_, e) in fields {
                    in_expr(e, source, offset, ident, best);
                }
            }
            TypedExprKind::Spread(inner) => in_expr(inner, source, offset, ident, best),
            TypedExprKind::Closure { body, .. } => in_expr(body, source, offset, ident, best),
            TypedExprKind::Some(inner) | TypedExprKind::Ok(inner) | TypedExprKind::Err(inner) => {
                in_expr(inner, source, offset, ident, best)
            }
            _ => {}
        }
    }
    fn in_stmt(
        stmt: &TypedStmt,
        source: &str,
        offset: usize,
        ident: &str,
        best: &mut Option<(crate::hir::types::Ty, String, Span, usize)>,
    ) {
        if !span_contains(&stmt.span, offset) {
            // Bodies may still contain the offset when the stmt span is
            // narrow; only skip cheaply when clearly before.
            if stmt.span.start > offset {
                return;
            }
        }
        match &stmt.kind {
            TypedStmtKind::Let { value, .. }
            | TypedStmtKind::Var { value, .. }
            | TypedStmtKind::Decl { value, .. } => in_expr(value, source, offset, ident, best),
            TypedStmtKind::Expr(e) => in_expr(e, source, offset, ident, best),
            TypedStmtKind::Return(e) => {
                if let Some(e) = e {
                    in_expr(e, source, offset, ident, best);
                }
            }
            TypedStmtKind::Assign { target, value } => {
                in_expr(target, source, offset, ident, best);
                in_expr(value, source, offset, ident, best);
            }
            TypedStmtKind::If {
                condition,
                then_body,
                else_body,
            } => {
                in_expr(condition, source, offset, ident, best);
                for s in then_body {
                    in_stmt(s, source, offset, ident, best);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        in_stmt(s, source, offset, ident, best);
                    }
                }
            }
            TypedStmtKind::While { body, .. } | TypedStmtKind::Loop { body } => {
                for s in body {
                    in_stmt(s, source, offset, ident, best);
                }
            }
            TypedStmtKind::For { iterable, body, .. } => {
                in_expr(iterable, source, offset, ident, best);
                for s in body {
                    in_stmt(s, source, offset, ident, best);
                }
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                in_expr(scrutinee, source, offset, ident, best);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        in_expr(g, source, offset, ident, best);
                    }
                    for s in &arm.body {
                        in_stmt(s, source, offset, ident, best);
                    }
                }
            }
            TypedStmtKind::Break(e) => {
                if let Some(e) = e {
                    in_expr(e, source, offset, ident, best);
                }
            }
            TypedStmtKind::Continue => {}
        }
    }
    let mut best = None;
    for func in &typed.functions {
        for stmt in &func.body {
            in_stmt(stmt, source, offset, ident, &mut best);
        }
    }
    best.map(|(ty, f, sp, _)| (ty, f, sp))
}

/// Scope-aware local resolution for `find_definition_at`.
fn find_local_definition_at(
    resolved: &Program,
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<Definition> {
    use crate::ast::{FunctionBody, Item};
    for item in &resolved.items {
        match item {
            Item::Function(f) if span_contains(&f.span, offset) => {
                for p in &f.params {
                    if p.name == ident {
                        return Some(Definition {
                            name: p.name.clone(),
                            kind: DefinitionKind::Param,
                            span: name_span_in(source, &p.span, &p.name),
                            detail: format!(
                                "param {}: {}",
                                p.name,
                                crate::ast::type_expr_to_string(&p.ty)
                            ),
                        });
                    }
                }
                match &f.body {
                    FunctionBody::Block(b) => {
                        if let Some(d) = find_in_block(b, source, offset, ident) {
                            return Some(d);
                        }
                    }
                    FunctionBody::Expr(_) => {}
                }
            }
            Item::Task(t) if span_contains(&t.span, offset) => {
                for p in &t.params {
                    if p.name == ident {
                        return Some(Definition {
                            name: p.name.clone(),
                            kind: DefinitionKind::Param,
                            span: name_span_in(source, &p.span, &p.name),
                            detail: format!("param {}", p.name),
                        });
                    }
                }
                if let Some(d) = find_in_block(&t.body, source, offset, ident) {
                    return Some(d);
                }
            }
            _ => {}
        }
    }
    None
}

fn stmt_span_of(stmt: &crate::ast::Stmt) -> Span {
    use crate::ast::Stmt;
    match stmt {
        Stmt::Let(s) => s.span.clone(),
        Stmt::Var(s) => s.span.clone(),
        Stmt::State(s) => s.span.clone(),
        Stmt::Assign(s) => s.span.clone(),
        Stmt::Expr(e) => e.span(),
        Stmt::Return(s) => s.span.clone(),
        Stmt::Break(s) => s.span.clone(),
        Stmt::Continue(sp) => sp.clone(),
        Stmt::If(s) => s.span.clone(),
        Stmt::While(s) => s.span.clone(),
        Stmt::Loop(s) => s.span.clone(),
        Stmt::For(s) => s.span.clone(),
        Stmt::Match(s) => s.span.clone(),
        Stmt::Function(f) => f.span.clone(),
        Stmt::Task(t) => t.span.clone(),
        Stmt::Struct(s) => s.span.clone(),
        Stmt::BareField(f) => f.span.clone(),
        Stmt::Decl(s) => s.span.clone(),
    }
}

/// Walk a block in source order, tracking the latest visible binding for
/// `ident`. Nested blocks containing `offset` are searched first (inner
/// shadows outer); otherwise the outer candidate wins. Later statements
/// starting after `offset` are never visible.
fn find_in_block(
    block: &crate::ast::Block,
    source: &str,
    offset: usize,
    ident: &str,
) -> Option<Definition> {
    use crate::ast::Stmt;
    let mut candidate: Option<Definition> = None;
    for stmt in &block.stmts {
        let span = stmt_span_of(stmt);
        if offset < span.start {
            break;
        }
        match stmt {
            Stmt::Let(s) if s.name == ident => {
                candidate = Some(Definition {
                    name: s.name.clone(),
                    kind: DefinitionKind::Local,
                    span: name_span_in(source, &s.span, &s.name),
                    detail: format!("let {}", s.name),
                });
            }
            Stmt::Var(s) if s.name == ident => {
                candidate = Some(Definition {
                    name: s.name.clone(),
                    kind: DefinitionKind::Local,
                    span: name_span_in(source, &s.span, &s.name),
                    detail: format!("var {}", s.name),
                });
            }
            Stmt::Decl(s) if s.name == ident => {
                candidate = Some(Definition {
                    name: s.name.clone(),
                    kind: DefinitionKind::Local,
                    span: name_span_in(source, &s.span, &s.name),
                    detail: format!("let {}", s.name),
                });
            }
            Stmt::State(s) if s.name == ident => {
                candidate = Some(Definition {
                    name: s.name.clone(),
                    kind: DefinitionKind::Local,
                    span: name_span_in(source, &s.span, &s.name),
                    detail: format!("state {}", s.name),
                });
            }
            Stmt::Function(f) => {
                if f.name == ident && span.start <= offset {
                    candidate = Some(Definition {
                        name: f.name.clone(),
                        kind: DefinitionKind::Function,
                        span: name_span_in(source, &f.span, &f.name),
                        detail: format!("fn {}(...)", f.name),
                    });
                }
                // Params + body visible only when cursor is inside.
                if span_contains(&f.span, offset) {
                    for p in &f.params {
                        if p.name == ident {
                            return Some(Definition {
                                name: p.name.clone(),
                                kind: DefinitionKind::Param,
                                span: name_span_in(source, &p.span, &p.name),
                                detail: format!("param {}", p.name),
                            });
                        }
                    }
                    use crate::ast::FunctionBody;
                    if let FunctionBody::Block(b) = &f.body {
                        if span_contains(&b.span, offset) {
                            if let Some(inner) = find_in_block(b, source, offset, ident) {
                                return Some(inner);
                            }
                            if candidate.as_ref().is_some_and(|c| c.name == ident) {
                                return candidate.clone();
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        // Nested blocks containing the cursor: inner shadows outer.
        let nested: Vec<&crate::ast::Block> = match stmt {
            Stmt::If(s) => {
                let mut v = vec![&s.then_block];
                for (_, b) in &s.else_if_clauses {
                    v.push(b);
                }
                if let Some(b) = &s.else_block {
                    v.push(b);
                }
                v
            }
            Stmt::While(s) => vec![&s.body],
            Stmt::Loop(s) => vec![&s.body],
            Stmt::For(s) => {
                // `for binding in ...:` defines the binding for the body.
                if s.binding == ident && span.start <= offset {
                    candidate = Some(Definition {
                        name: s.binding.clone(),
                        kind: DefinitionKind::ForBinding,
                        span: name_span_in(source, &span, &s.binding),
                        detail: format!("for {}", s.binding),
                    });
                }
                vec![&s.body]
            }
            _ => Vec::new(),
        };
        for b in nested {
            if span_contains(&b.span, offset) {
                if let Some(inner) = find_in_block(b, source, offset, ident) {
                    return Some(inner);
                }
                // No inner match: fall through to outer candidate below.
                // Cursor is inside this branch, so later siblings are
                // unreachable — return what we have.
                if candidate.as_ref().is_some_and(|c| c.name == ident) {
                    return candidate.clone();
                }
                return None;
            }
        }
        // Match arms: pattern bindings are per-arm.
        if let Stmt::Match(s) = stmt {
            if span_contains(&span, offset) {
                for arm in &s.arms {
                    if span_contains(&arm.span, offset) {
                        if let Some(d) = find_in_pattern(&arm.pattern, source, ident) {
                            return Some(d);
                        }
                        match &arm.body {
                            crate::ast::MatchBody::Block(b) => {
                                if let Some(inner) = find_in_block(b, source, offset, ident) {
                                    return Some(inner);
                                }
                            }
                            crate::ast::MatchBody::Expr(_) => {}
                        }
                        if candidate.as_ref().is_some_and(|c| c.name == ident) {
                            return candidate.clone();
                        }
                        return None;
                    }
                }
            }
        }
    }
    if candidate.as_ref().is_some_and(|c| c.name == ident) {
        candidate
    } else {
        None
    }
}

fn find_in_pattern(
    pat: &crate::ast::Pattern,
    _source: &str,
    ident: &str,
) -> Option<Definition> {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(name, span) if name == ident => Some(Definition {
            name: name.clone(),
            kind: DefinitionKind::PatternBinding,
            span: span.clone(),
            detail: format!("binding {name}"),
        }),
        Pattern::Variant(_, subs, _) => subs
            .iter()
            .find_map(|p| find_in_pattern(p, _source, ident)),
        _ => None,
    }
}

/// Check if a span contains a byte offset.
fn span_contains(span: &Span, offset: usize) -> bool {
    span.start <= offset && offset < span.end
}

/// Convert LSP Position (UTF-16 code units) to byte offset.
/// Overshooting a short line clamps to the line end (never into the next
/// line); mid-character offsets snap to the character start.
fn position_to_byte_offset(source: &str, position: lsp_types::Position) -> usize {
    let want_line = position.line as usize;
    let want_col = position.character as usize;
    let mut line = 0;
    let mut line_start = 0usize;
    for (i, ch) in source.char_indices() {
        if ch == '\n' {
            if line == want_line {
                break;
            }
            line += 1;
            line_start = i + 1;
        }
        if line == want_line {
            break;
        }
    }
    if line != want_line {
        return source.len();
    }
    let mut col = 0usize;
    for (i, ch) in source[line_start..].char_indices() {
        if ch == '\n' || col >= want_col {
            return line_start + i;
        }
        col += ch.len_utf16();
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
            DefinitionKind::Local
            | DefinitionKind::Param
            | DefinitionKind::ForBinding
            | DefinitionKind::PatternBinding => lsp_types::CompletionItemKind::VARIABLE,
            DefinitionKind::Import => lsp_types::CompletionItemKind::MODULE,
            DefinitionKind::Field => lsp_types::CompletionItemKind::FIELD,
            DefinitionKind::Variant => lsp_types::CompletionItemKind::ENUM_MEMBER,
            DefinitionKind::Method => lsp_types::CompletionItemKind::METHOD,
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
        "impl", "unsafe", "async", "await", "task", "derive",
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

/// Render a hover card for a member-level [`Definition`]
/// (field / variant / method from member, label, or def-at-cursor).
/// `def.span` is already name-only; the card range highlights it.
fn hover_for_definition(
    resolved: &Program,
    source: &str,
    def: &Definition,
    file_label: &str,
) -> lsp_types::Hover {
    let range = Some(span_to_range(source, &def.span));
    let docs = doc_comment_for(source, def.span.start);
    let (signature, ty) = match def.kind {
        DefinitionKind::Field => {
            // `detail` is `field Parent.name: Type`.
            let ty = def
                .detail
                .split_once(": ")
                .map(|(_, t)| t.to_string());
            (def.detail.clone(), ty)
        }
        DefinitionKind::Variant => (def.detail.clone(), None),
        DefinitionKind::Method => (def.detail.clone(), None),
        _ => (def.detail.clone(), None),
    };
    render_hover(
        HoverInfo {
            signature,
            ty,
            declared_in: file_label.to_string(),
            docs,
        },
        range,
    )
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
        "derive" => ("derive Serialize|Deserialize for Type:",
            "Derives JSON serialization for a struct (ADR-018). Expands at resolve time to `to_json_<Type>` / `from_json_<Type>` plus a marker impl; only `Serialize` and `Deserialize` for structs."),
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
/// Lookup order (noctivue-analyzer): innermost scope-aware local/param
/// binding first (shadowing-correct), then top-level definitions, then
/// builtins — a user-defined item always shadows a builtin of the same
/// name, and a local always shadows a top-level item.
fn get_hover_for_identifier(
    resolved: &Program,
    typed: &Module,
    source: &str,
    offset: usize,
    file_label: &str,
) -> Option<lsp_types::Hover> {
    // Extract the identifier at the cursor position
    let ident = extract_identifier_at(source, offset)?;

    // Member-field use + struct-literal label (qualified — beat shadowing).
    if let Some(def) = find_member_definition_at(resolved, typed, source, offset, &ident)
        .or_else(|| find_struct_label_definition_at(resolved, source, offset, &ident))
    {
        return Some(hover_for_definition(resolved, source, &def, file_label));
    }
    // Bare enum variant use (`North`).
    if let Some(def) = find_variant_definition(resolved, source, &ident) {
        // Locals shadow variants (`let North = 1`): check scope first.
        if find_local_definition_at(resolved, source, offset, &ident).is_none() {
            return Some(hover_for_definition(resolved, source, &def, file_label));
        }
    }
    // Field / variant / method definition at the cursor.
    if let Some(def) = find_member_def_at_cursor(resolved, source, offset, &ident) {
        return Some(hover_for_definition(resolved, source, &def, file_label));
    }

    // Scope-aware locals/params (shadow top-level, but not qualified uses).
    if let Some(local_hover) =
        hover_for_scope_local(resolved, typed, source, offset, &ident, file_label)
    {
        return Some(local_hover);
    }

    // Look up the identifier in top-level definitions.
    // Ranges are name-only (noctivue-analyzer P0): the editor highlights
    // the identifier, not the whole item block.
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
                    name_span_in(source, &s.span, &s.name),
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
                    name_span_in(source, &f.span, &f.name),
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
                    name_span_in(source, &e.span, &e.name),
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
                name_span_in(source, &c.span, &c.name),
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
                    name_span_in(source, &t.span, &t.name),
                )
            }
            _ => continue,
        };
        return Some(render_hover(info, Some(span_to_range(source, &span))));
    }

    // Legacy unscoped HIR fallback: catches bindings the scope walk misses
    // (e.g. bodies the AST walk doesn't descend into yet). Scope-correct
    // hits already returned above, so this only fires for otherwise
    // unresolvable names.
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

/// Scope-aware hover for locals/params/pattern bindings (noctivue-analyzer).
///
/// Resolves with the same scope logic as goto (`find_local_definition_at`)
/// so hover and goto never disagree, then attaches types:
/// AST annotations for the signature, HIR for the `Type:` line + inference
/// fallback when unannotated (`let user = greet("Bob")` → `String`).
fn hover_for_scope_local(
    resolved: &Program,
    typed: &Module,
    source: &str,
    offset: usize,
    ident: &str,
    file_label: &str,
) -> Option<lsp_types::Hover> {
    let def = find_local_definition_at(resolved, source, offset, ident)?;
    match def.kind {
        DefinitionKind::Param => {
            let (ast_ty, hir_ty) = param_types(resolved, typed, offset, ident);
            let sig_ty = ast_ty
                .clone()
                .or_else(|| hir_ty.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            return Some(render_hover(
                HoverInfo {
                    signature: format!("param {ident}: {sig_ty}"),
                    ty: hir_ty.or(ast_ty),
                    declared_in: format!("{file_label} (local binding)"),
                    docs: None,
                },
                None,
            ));
        }
        DefinitionKind::Local => {
            let ast_ty = local_annotation(resolved, &def, ident);
            let hir_ty = hir_type_for_name(typed, ident);
            let sig_ty = ast_ty
                .clone()
                .or_else(|| hir_ty.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            // `var` vs `let` from the goto detail (`var x` / `let x`).
            let kw = if def.detail.starts_with("var ") {
                "var"
            } else if def.detail.starts_with("state ") {
                "state"
            } else {
                "let"
            };
            return Some(render_hover(
                HoverInfo {
                    signature: format!("{kw} {ident}: {sig_ty}"),
                    ty: hir_ty.or(ast_ty),
                    declared_in: format!("{file_label} (local binding)"),
                    docs: None,
                },
                None,
            ));
        }
        DefinitionKind::ForBinding | DefinitionKind::PatternBinding => {
            let hir_ty = hir_type_for_name(typed, ident);
            let sig_ty = hir_ty.clone().unwrap_or_else(|| "Unknown".to_string());
            return Some(render_hover(
                HoverInfo {
                    signature: format!("let {ident}: {sig_ty}"),
                    ty: hir_ty,
                    declared_in: format!("{file_label} (local binding)"),
                    docs: None,
                },
                None,
            ));
        }
        DefinitionKind::Function => {
            // Local `fn` (Stmt::Function): minimal card until full
            // signature rendering lands.
            return Some(render_hover(
                HoverInfo {
                    signature: format!("fn {ident}(...)"),
                    ty: None,
                    declared_in: format!("{file_label} (local binding)"),
                    docs: None,
                },
                None,
            ));
        }
        _ => None,
    }
}

/// AST annotation + HIR resolved type for the param visible at `offset`.
fn param_types(
    resolved: &Program,
    typed: &Module,
    offset: usize,
    ident: &str,
) -> (Option<String>, Option<String>) {
    use crate::ast::{FunctionBody, Item};
    for item in &resolved.items {
        match item {
            Item::Function(f) if span_contains(&f.span, offset) => {
                if let Some(p) = f.params.iter().find(|p| p.name == ident) {
                    let ast = Some(crate::ast::type_expr_to_string(&p.ty));
                    let hir = typed
                        .functions
                        .iter()
                        .find(|h| h.name == f.name)
                        .and_then(|h| h.params.iter().find(|(n, _)| n == ident))
                        .map(|(_, ty)| ty.to_string());
                    return (ast, hir);
                }
                // Local fn params.
                if let FunctionBody::Block(b) = &f.body {
                    if let Some(ast) = find_param_annotation_in_block(b, ident) {
                        let hir = typed
                            .functions
                            .iter()
                            .flat_map(|h| h.params.iter())
                            .find(|(n, _)| n == ident)
                            .map(|(_, ty)| ty.to_string());
                        return (Some(ast), hir);
                    }
                }
            }
            Item::Task(t) if span_contains(&t.span, offset) => {
                if let Some(p) = t.params.iter().find(|p| p.name == ident) {
                    let ast = Some(crate::ast::type_expr_to_string(&p.ty));
                    let hir = typed
                        .functions
                        .iter()
                        .find(|h| h.name == t.name)
                        .and_then(|h| h.params.iter().find(|(n, _)| n == ident))
                        .map(|(_, ty)| ty.to_string());
                    return (ast, hir);
                }
            }
            _ => {}
        }
    }
    (None, None)
}

fn find_param_annotation_in_block(block: &crate::ast::Block, ident: &str) -> Option<String> {
    use crate::ast::{FunctionBody, Stmt};
    for stmt in &block.stmts {
        match stmt {
            Stmt::Function(f) => {
                if let Some(p) = f.params.iter().find(|p| p.name == ident) {
                    return Some(crate::ast::type_expr_to_string(&p.ty));
                }
                if let FunctionBody::Block(b) = &f.body {
                    if let Some(t) = find_param_annotation_in_block(b, ident) {
                        return Some(t);
                    }
                }
            }
            Stmt::If(s) => {
                for b in std::iter::once(&s.then_block)
                    .chain(s.else_if_clauses.iter().map(|(_, b)| b))
                    .chain(s.else_block.iter())
                {
                    if let Some(t) = find_param_annotation_in_block(b, ident) {
                        return Some(t);
                    }
                }
            }
            Stmt::While(s) => {
                if let Some(t) = find_param_annotation_in_block(&s.body, ident) {
                    return Some(t);
                }
            }
            Stmt::Loop(s) => {
                if let Some(t) = find_param_annotation_in_block(&s.body, ident) {
                    return Some(t);
                }
            }
            Stmt::For(s) => {
                if let Some(t) = find_param_annotation_in_block(&s.body, ident) {
                    return Some(t);
                }
            }
            Stmt::Match(s) => {
                for arm in &s.arms {
                    if let crate::ast::MatchBody::Block(b) = &arm.body {
                        if let Some(t) = find_param_annotation_in_block(b, ident) {
                            return Some(t);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// AST `let/var` annotation for a scope-resolved local `def`.
/// Matches by the definition's name-span start falling inside the
/// statement span (HIR spans thread from the same AST spans).
fn local_annotation(resolved: &Program, def: &Definition, ident: &str) -> Option<String> {
    use crate::ast::{FunctionBody, Item, Stmt};
    fn in_block(
        block: &crate::ast::Block,
        def_start: usize,
        ident: &str,
    ) -> Option<String> {
        for stmt in &block.stmts {
            let span = stmt_span_of(stmt);
            if def_start < span.start || def_start >= span.end {
                // Definition not in this statement — but nested blocks
                // of an enclosing statement may still hold it.
            }
            match stmt {
                Stmt::Let(s) if s.name == ident && span_contains(&span, def_start) => {
                    return s
                        .ty
                        .as_ref()
                        .map(crate::ast::type_expr_to_string);
                }
                Stmt::Var(s) if s.name == ident && span_contains(&span, def_start) => {
                    return s
                        .ty
                        .as_ref()
                        .map(crate::ast::type_expr_to_string);
                }
                _ => {}
            }
            // Recurse into nested blocks regardless (def may be nested).
            let nested: Vec<&crate::ast::Block> = match stmt {
                Stmt::If(s) => {
                    let mut v = vec![&s.then_block];
                    for (_, b) in &s.else_if_clauses {
                        v.push(b);
                    }
                    if let Some(b) = &s.else_block {
                        v.push(b);
                    }
                    v
                }
                Stmt::While(s) => vec![&s.body],
                Stmt::Loop(s) => vec![&s.body],
                Stmt::For(s) => vec![&s.body],
                Stmt::Function(f) => match &f.body {
                    FunctionBody::Block(b) => vec![b],
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            for b in nested {
                if let Some(t) = in_block(b, def_start, ident) {
                    return Some(t);
                }
            }
            if let Stmt::Match(s) = stmt {
                for arm in &s.arms {
                    if let crate::ast::MatchBody::Block(b) = &arm.body {
                        if let Some(t) = in_block(b, def_start, ident) {
                            return Some(t);
                        }
                    }
                }
            }
        }
        None
    }
    let def_start = def.span.start;
    for item in &resolved.items {
        match item {
            Item::Function(f) => {
                if let FunctionBody::Block(b) = &f.body {
                    if let Some(t) = in_block(b, def_start, ident) {
                        return Some(t);
                    }
                }
            }
            Item::Task(t) => {
                if let Some(ty) = in_block(&t.body, def_start, ident) {
                    return Some(ty);
                }
            }
            _ => {}
        }
    }
    None
}

/// First-match HIR type for `ident` (inference fallback for unannotated
/// bindings). Scope-correctness comes from the AST walk above; this only
/// supplies the type string.
fn hir_type_for_name(typed: &Module, ident: &str) -> Option<String> {
    use crate::hir::items::TypedStmtKind;
    fn in_stmts(stmts: &[crate::hir::items::TypedStmt], ident: &str) -> Option<String> {
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { name, ty, .. }
                | TypedStmtKind::Var { name, ty, .. }
                | TypedStmtKind::Decl { name, ty, .. }
                    if name == ident =>
                {
                    let s = ty.to_string();
                    if s != "Unknown" && s != "Error" {
                        return Some(s);
                    }
                }
                _ => {}
            }
            match &stmt.kind {
                TypedStmtKind::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    if let Some(t) = in_stmts(then_body, ident) {
                        return Some(t);
                    }
                    if let Some(eb) = else_body {
                        if let Some(t) = in_stmts(eb, ident) {
                            return Some(t);
                        }
                    }
                }
                TypedStmtKind::While { body, .. }
                | TypedStmtKind::Loop { body, .. }
                | TypedStmtKind::For { body, .. } => {
                    if let Some(t) = in_stmts(body, ident) {
                        return Some(t);
                    }
                }
                TypedStmtKind::Match { arms, .. } => {
                    for arm in arms {
                        if let Some(t) = in_stmts(&arm.body, ident) {
                            return Some(t);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }
    for func in &typed.functions {
        if let Some(t) = func
            .params
            .iter()
            .find(|(n, _)| n == ident)
            .map(|(_, ty)| ty.to_string())
        {
            if t != "Unknown" && t != "Error" {
                return Some(t);
            }
        }
        if let Some(t) = in_stmts(&func.body, ident) {
            return Some(t);
        }
    }
    None
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

    /// Distinguishes concurrent tests' scratch directories.
    static NEXT_TMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

    /// A project whose manifest declares a `path:` dependency must not
    /// collect editor diagnostics for an import the build resolves
    /// happily. Two false results are pinned, because the second hides the
    /// first: the graph's `E0101` for the unresolvable import, and the
    /// `E0201 unknown identifier` squiggles that follow from a `provided`
    /// set built without the dependency.
    ///
    /// The layout mirrors the real one (`<app>/nestpkg.nvpm` naming
    /// `../libs/dep`, whose package is `<root>/libs/dep/lib/main.nv`),
    /// because the bare-package form only resolves through a root that
    /// HOLDS packages: handing over the package directory itself makes the
    /// resolver look for `dep/dep/lib/...`.
    #[test]
    fn a_path_dependency_produces_no_false_editor_diagnostics() {
        let base = std::env::temp_dir().join(format!(
            "noct-analysis-paths-{}-{}",
            std::process::id(),
            NEXT_TMP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let libs = base.join("libs");
        let dep_lib = libs.join("dep").join("lib");
        std::fs::create_dir_all(&dep_lib).expect("create dep lib");
        let app = base.join("app");
        std::fs::create_dir_all(app.join("lib")).expect("create app lib");

        std::fs::write(dep_lib.join("main.nv"), "export fn helper() -> Int:\n    7\n")
            .expect("write dep");
        std::fs::write(
            app.join("nestpkg.nvpm"),
            "package:\n    name: app\n    version: 0.1.0\n\ndependencies:\n    dep:\n        version: =0.1.0\n        path: ../libs/dep\n",
        )
        .expect("write manifest");
        let main_path = app.join("lib").join("main.nv");
        let source = "import dep\n\nmain():\n    let n: Int = helper()\n    print(\"{n}\")\n";
        std::fs::write(&main_path, source).expect("write main");
        let uri = format!(
            "file:///{}",
            main_path.to_string_lossy().replace('\\', "/")
        );

        let error_codes = |result: &AnalysisResult| -> Vec<String> {
            result
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .filter_map(|d| d.code.clone())
                .collect()
        };

        let with_roots = analyze_file_with_roots(&uri, source, &[libs.clone()]);
        let good = error_codes(&with_roots);
        assert!(
            !good.iter().any(|c| c == "E0101"),
            "a declared path dependency must not report E0101: {good:?}"
        );
        assert!(
            !good.iter().any(|c| c == "E0201"),
            "a name the dependency provides must not report E0201: {good:?}"
        );

        // Without the root the import cannot resolve, which is what makes
        // this a real regression test rather than a vacuous one.
        let without = analyze_file(&uri, source);
        let bad = error_codes(&without);
        assert!(
            bad.iter().any(|c| c == "E0101"),
            "without the root the import must fail to resolve, or this test \
             no longer demonstrates anything: {bad:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── noctivue-analyzer P0: usage → definition + name-only spans ──

    fn goto_at(source: &str, line: u32, character: u32) -> Option<Definition> {
        let a = analyze_file("test.nv", source);
        find_definition_at(
            &a.resolved,
            &a.typed,
            source,
            lsp_types::Position::new(line, character),
        )
    }

    fn text_of(source: &str, span: &Span) -> String {
        source[span.start.min(source.len())..span.end.min(source.len())].to_string()
    }

    #[test]
    fn goto_call_site_resolves_to_callee_name() {
        // `greet` call on line 7 — must jump to `fn greet` name, not None.
        let def = goto_at(DEMO, 7, 11).expect("goto on greet call");
        assert_eq!(def.name, "greet");
        assert_eq!(text_of(DEMO, &def.span), "greet");
    }

    #[test]
    fn goto_whitespace_returns_none_not_enclosing_item() {
        // Leading spaces of the print line: no identifier → None.
        // Old behavior returned the enclosing `main`; that is a bug.
        assert!(goto_at(DEMO, 7, 0).is_none());
    }

    #[test]
    fn goto_definition_name_resolves_to_itself_narrowly() {
        let def = goto_at(DEMO, 3, 4).expect("goto on add def");
        assert_eq!(def.name, "add");
        assert_eq!(text_of(DEMO, &def.span), "add");
    }

    #[test]
    fn goto_local_use_resolves_to_let_binding() {
        // `user` use on line 9 → `let user` on line 8, name-only.
        let def = goto_at(DEMO, 9, 23).expect("goto on local user use");
        assert_eq!(def.name, "user");
        assert_eq!(def.kind, DefinitionKind::Local);
        assert_eq!(text_of(DEMO, &def.span), "user");
    }

    #[test]
    fn goto_param_use_resolves_to_param() {
        let src = "fn greet(name: String) -> String:\n    name\n";
        // `name` use on line 1 → param on line 0.
        let def = goto_at(src, 1, 4).expect("goto on param use");
        assert_eq!(def.name, "name");
        assert_eq!(def.kind, DefinitionKind::Param);
        assert_eq!(text_of(src, &def.span), "name");
    }

    #[test]
    fn goto_shadowing_prefers_inner_let() {
        let src = "main():\n    let x = 1\n    let x = 2\n    print(x)\n";
        // `x` use on line 3 (`    print(x)` — `x` at char 10) → line 2 let.
        let def = goto_at(src, 3, 10).expect("goto on shadowed x");
        assert_eq!(def.kind, DefinitionKind::Local);
        let line_of = |offset: usize| src[..offset].bytes().filter(|b| *b == b'\n').count();
        assert_eq!(line_of(def.span.start), 2, "should resolve to line 2 let");
    }

    #[test]
    fn goto_import_alias_resolves_to_import() {
        let src = "import dep\n\nmain():\n    print(dep)\n";
        let def = goto_at(src, 3, 11).expect("goto on import alias use");
        assert_eq!(def.name, "dep");
        assert_eq!(def.kind, DefinitionKind::Import);
    }

    #[test]
    fn hover_param_use_shows_param_card() {
        let src = "fn greet(name: String) -> String:\n    name\n";
        let text = hover_text_at(src, 1, 4).expect("expected hover on param use");
        assert!(text.contains("param name:"), "got: {text}");
        assert!(text.contains("String"), "got: {text}");
        assert!(text.contains("(local binding)"), "got: {text}");
    }

    #[test]
    fn hover_shadowing_prefers_inner_binding() {
        // Two `let x` in nested blocks; use inside inner block → inner type.
        let src = "main():\n    let x: Int = 1\n    if true:\n        let x: String = \"s\"\n        print(x)\n";
        let text = hover_text_at(src, 4, 14).expect("expected hover on inner x");
        assert!(text.contains("String"), "got: {text}");
        assert!(!text.contains("param"), "got: {text}");
    }

    #[test]
    fn hover_top_level_shadowed_by_local() {
        // Top-level `greet` fn + local `greet` binding: use → local, not fn.
        let src = "fn greet(name: String) -> String:\n    name\n\nmain():\n    let greet: Int = 1\n    print(greet)\n";
        let text = hover_text_at(src, 5, 11).expect("expected hover on local greet");
        assert!(text.contains("(local binding)"), "got: {text}");
        assert!(!text.contains("fn greet(name"), "got: {text}");
    }

    // ── noctivue-analyzer UTF-16 positions (LSP §3.17) ──

    #[test]
    fn positions_round_trip_over_mixed_unicode() {
        // ASCII (1B/1u), é (2B/1u), 中 (3B/1u), 😀 (4B/2u).
        let src = "aé中😀b\nxy";
        // Every char-boundary offset must survive offset → position → offset.
        let mut bounds = vec![0];
        for (i, ch) in src.char_indices() {
            bounds.push(i + ch.len_utf8());
        }
        for off in bounds {
            let pos = byte_offset_to_position(src, off);
            let back = position_to_byte_offset(
                src,
                lsp_types::Position::new(pos.line as u32, pos.character as u32),
            );
            assert_eq!(back, off, "round trip failed at byte {off}");
        }
        // Spot values: `é` ends byte 3 but is character 2 on line 0.
        let e_end = byte_offset_to_position(src, 3);
        assert_eq!((e_end.line, e_end.character), (0, 2));
        // 😀 spans bytes 7..11 and occupies TWO units (chars 4..6).
        let emoji_end = byte_offset_to_position(src, 11);
        assert_eq!((emoji_end.line, emoji_end.character), (0, 6));
        // Line 1 starts at byte 12 (`x`); byte 13 is `y` at char 1.
        let l1 = byte_offset_to_position(src, 12);
        assert_eq!((l1.line, l1.character), (1, 0));
    }

    #[test]
    fn goto_unicode_ident_resolves() {
        // LANGUAGE_SPEC.md §1 identifiers: non-ASCII must not desync.
        let src = "main():\n    let café = 1\n    print(café)\n";
        let def = goto_needle(src, "café", 1).expect("goto on café use");
        assert_eq!(def.kind, DefinitionKind::Local);
        assert_eq!(text_of(src, &def.span), "café");
    }

    #[test]
    fn diagnostic_related_info_points_at_document_uri() {
        let uri: lsp_types::Uri = "file:///test.nv".parse().unwrap();
        let diag = crate::diagnostics::Diagnostic::error("two labels")
            .with_span(Span { start: 0, end: 1 }, "first")
            .with_span(Span { start: 2, end: 3 }, "second");
        let lsp = diagnostic_to_lsp("ab\ncd", &uri, &diag);
        let related = lsp.related_information.expect("related info");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.uri.as_str(), "file:///test.nv");
    }

    #[test]
    fn name_span_in_finds_identifier_not_keyword_prefix() {
        // `mod m:` — searching `m` must not match the `m` in `mod`.
        // Synthetic span (no parser dependency): outer covers `mod m:`.
        let src = "mod m:\n";
        let outer = Span { start: 0, end: 6 };
        let ns = name_span_in(src, &outer, "m");
        assert_eq!(text_of(src, &ns), "m");
        assert_eq!((ns.start, ns.end), (4, 5));
    }

    // ── noctivue-analyzer members: fields, labels, variants ──

    const STRUCT_DEMO: &str = "struct User:\n    name: String\n    age: Int\n\nmake_user(name: String, age: Int) -> User:\n    User { name: name, age: age }\n\nmain():\n    let user = make_user(\"Bob\", 25)\n    print(user.name)\n";

    /// Byte offset of the n-th occurrence of `needle` (0-based).
    fn nth_offset(source: &str, needle: &str, n: usize) -> usize {
        let mut count = 0;
        let mut from = 0;
        while let Some(rel) = source[from..].find(needle) {
            if count == n {
                return from + rel;
            }
            count += 1;
            from += rel + 1;
        }
        panic!("needle {needle:?} occurrence {n} not found");
    }

    fn goto_needle(source: &str, needle: &str, n: usize) -> Option<Definition> {
        let off = nth_offset(source, needle, n) + 1;
        let a = analyze_file("test.nv", source);
        // offset → Position in UTF-16 units (required past ASCII).
        let prefix = &source[..off.min(source.len())];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
        let character: usize = prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .chars()
            .map(|c| c.len_utf16())
            .sum();
        let character = character as u32;
        find_definition_at(
            &a.resolved,
            &a.typed,
            source,
            lsp_types::Position::new(line, character),
        )
    }

    #[test]
    fn goto_member_use_resolves_to_field_decl() {
        // Occurrences: 0 field def, 1 param, 2 label, 3 value, 4 member.
        let def = goto_needle(STRUCT_DEMO, "name", 4).expect("goto on user.name");
        assert_eq!(def.kind, DefinitionKind::Field);
        assert_eq!(def.name, "name");
        assert!(def.detail.contains("User.name"), "got: {}", def.detail);
        assert_eq!(text_of(STRUCT_DEMO, &def.span), "name");
    }

    #[test]
    fn goto_struct_label_resolves_to_field_decl() {
        // `User { name: ...` label (3rd `name`: struct field def is 1st,
        // param is 2nd, label is 3rd).
        let def = goto_needle(STRUCT_DEMO, "name", 2).expect("goto on label");
        assert_eq!(def.kind, DefinitionKind::Field);
        assert!(def.detail.contains("User.name"), "got: {}", def.detail);
    }

    #[test]
    fn goto_field_def_resolves_to_itself() {
        let def = goto_needle(STRUCT_DEMO, "name", 0).expect("goto on field def");
        assert_eq!(def.kind, DefinitionKind::Field);
        assert!(def.detail.contains("User.name"), "got: {}", def.detail);
    }

    #[test]
    fn goto_bare_variant_resolves_to_variant() {
        let src = "enum Direction:\n    North\n    South\n\nmain():\n    print(North)\n";
        let def = goto_needle(src, "North", 1).expect("goto on North use");
        assert_eq!(def.kind, DefinitionKind::Variant);
        assert!(def.detail.contains("Direction::North"), "got: {}", def.detail);
    }

    #[test]
    fn hover_member_use_shows_field_card() {
        // Hover the `name` in `user.name` (occurrence 4).
        let off = nth_offset(STRUCT_DEMO, "name", 4) + 1;
        let a = analyze_file("test.nv", STRUCT_DEMO);
        let prefix = &STRUCT_DEMO[..off];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = prefix.rsplit('\n').next().unwrap_or("").len() as u32;
        let label = file_label_of(&a.file_path);
        let h = get_hover(
            &a.resolved,
            &a.typed,
            STRUCT_DEMO,
            lsp_types::Position::new(line, character),
            &label,
        )
        .expect("hover on user.name");
        let text = match h.contents {
            lsp_types::HoverContents::Markup(m) => m.value,
            _ => panic!("markup"),
        };
        assert!(text.contains("User.name"), "got: {text}");
        assert!(text.contains("String"), "got: {text}");
    }

    // ── noctivue-analyzer cross-file + type-definition ──

    #[test]
    fn nominal_name_unwraps_wrappers_not_primitives() {
        use crate::hir::types::Ty;
        assert_eq!(
            nominal_name_of(&Ty::Option(Box::new(Ty::Named(
                "User".into(),
                vec![]
            )))),
            Some("User".to_string())
        );
        assert_eq!(nominal_name_of(&Ty::String), None);
        assert_eq!(nominal_name_of(&Ty::Unknown), None);
    }

    #[test]
    fn find_type_at_reports_local_struct_type() {
        // `user` use → `User` (make_user returns User).
        let off = nth_offset(STRUCT_DEMO, "user.name", 0) + 2;
        let a = analyze_file("test.nv", STRUCT_DEMO);
        let prefix = &STRUCT_DEMO[..off];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = prefix.rsplit('\n').next().unwrap_or("").len() as u32;
        let ty = find_type_at(&a.typed, STRUCT_DEMO, lsp_types::Position::new(line, character))
            .expect("type at user use");
        assert_eq!(nominal_name_of(&ty), Some("User".to_string()));
    }

    #[test]
    fn cross_file_target_finds_path_dep_export() {
        let base = std::env::temp_dir().join(format!(
            "noct-xfile-{}-{}",
            std::process::id(),
            NEXT_TMP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let libs = base.join("libs");
        let dep_lib = libs.join("dep").join("lib");
        std::fs::create_dir_all(&dep_lib).expect("dep lib");
        let app = base.join("app");
        std::fs::create_dir_all(app.join("lib")).expect("app lib");
        std::fs::write(dep_lib.join("main.nv"), "export fn helper() -> Int:\n    7\n").expect("dep");
        std::fs::write(
            app.join("nestpkg.nvpm"),
            "package:\n    name: app\n    version: 0.1.0\n\ndependencies:\n    dep:\n        version: =0.1.0\n        path: ../libs/dep\n",
        )
        .expect("manifest");
        let main_path = app.join("lib").join("main.nv");
        let source = "import dep\n\nmain():\n    let n: Int = helper()\n    print(\"{n}\")\n";
        std::fs::write(&main_path, source).expect("main");
        let program = parse_resolved(source);
        // Flat use resolves to the dep file with the item name.
        let (target, item) =
            find_cross_file_target(&main_path, &program, "helper", &[libs.clone()])
                .expect("provider for helper");
        assert!(target.ends_with("main.nv"), "got: {}", target.display());
        assert_eq!(item.as_deref(), Some("helper"));
        // Import alias resolves through the decl.
        let (target, _) = find_cross_file_target(&main_path, &program, "dep", &[libs])
            .expect("target for dep import");
        assert!(target.ends_with("main.nv"), "got: {}", target.display());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn trait_context_and_impl_spans() {
        let src = "trait Pet:\n    fn name() -> String\n\nstruct Dog:\n    tag: String\n\nimpl Pet for Dog:\n    fn name() -> String:\n        \"d\"\n";
        let program = parse_resolved(src);
        // Cursor on trait member def (line 1, `name`).
        let off = nth_offset(src, "name", 0) + 1;
        let prefix = &src[..off];
        let pos = lsp_types::Position::new(
            prefix.bytes().filter(|b| *b == b'\n').count() as u32,
            prefix.rsplit('\n').next().unwrap_or("").len() as u32,
        );
        let (trait_name, method) =
            find_trait_context(&program, src, pos, "name").expect("trait ctx");
        assert_eq!(trait_name, "Pet");
        assert_eq!(method.as_deref(), Some("name"));
        let spans = collect_impl_spans(&program, src, "Pet", Some("name"));
        assert_eq!(spans.len(), 1);
        assert_eq!(text_of(src, &spans[0]), "name");
    }

    #[test]
    fn hover_bare_variant_shows_variant_card() {
        let src = "enum Direction:\n    North\n    South\n\nmain():\n    print(North)\n";
        let off = nth_offset(src, "North", 1) + 1;
        let a = analyze_file("test.nv", src);
        let prefix = &src[..off];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = prefix.rsplit('\n').next().unwrap_or("").len() as u32;
        let label = file_label_of(&a.file_path);
        let h = get_hover(
            &a.resolved,
            &a.typed,
            src,
            lsp_types::Position::new(line, character),
            &label,
        )
        .expect("hover on North");
        let text = match h.contents {
            lsp_types::HoverContents::Markup(m) => m.value,
            _ => panic!("markup"),
        };
        assert!(text.contains("Direction::North"), "got: {text}");
    }

    /// Hovering an imported name must name where it comes from. Two gaps
    /// closed here at once: single-file hover returns `None` for anything
    /// not defined in the file, and `find_item_span` — which the lookup
    /// funnels through — did not unwrap `Item::Export`, so even a
    /// successful cross-file resolution went blind on exactly the items a
    /// dependency exists to provide.
    #[test]
    fn hover_on_an_import_names_the_defining_file() {
        let base = std::env::temp_dir().join(format!(
            "noct-analysis-hover-{}-{}",
            std::process::id(),
            NEXT_TMP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let libs = base.join("libs");
        let dep_lib = libs.join("dep").join("lib");
        std::fs::create_dir_all(&dep_lib).expect("create dep lib");
        let app = base.join("app");
        std::fs::create_dir_all(app.join("lib")).expect("create app lib");

        std::fs::write(
            dep_lib.join("main.nv"),
            "/// Adds one.\nexport fn helper() -> Int:\n    7\n",
        )
        .expect("write dep");
        let main_path = app.join("lib").join("main.nv");
        let source = "import dep\n\nmain():\n    let n: Int = helper()\n    print(\"{n}\")\n";
        std::fs::write(&main_path, source).expect("write main");
        let uri = format!(
            "file:///{}",
            main_path.to_string_lossy().replace('\\', "/")
        );

        let a = analyze_file_with_roots(&uri, source, &[libs.clone()]);
        // Line 3 (0-based), inside `helper`.
        let pos = lsp_types::Position::new(3, 22);
        let single = get_hover(&a.resolved, &a.typed, source, pos, "main.nv");
        assert!(
            single.is_none(),
            "single-file hover must not answer for an import (it would be a guess)"
        );
        let cross = hover_cross_file_at(&uri, &a.resolved, source, pos, &[libs])
            .expect("cross-file hover must answer for a resolvable import");
        let lsp_types::HoverContents::Markup(markup) = cross.contents else {
            panic!("hover must be markdown");
        };
        assert!(
            markup.value.contains("fn helper(...)"),
            "the card must carry the signature, got: {}",
            markup.value
        );
        assert!(
            markup.value.contains("main.nv (imported)"),
            "the card must name the defining file, got: {}",
            markup.value
        );
        assert!(
            markup.value.contains("Adds one."),
            "the card must carry the target's doc comment, got: {}",
            markup.value
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
