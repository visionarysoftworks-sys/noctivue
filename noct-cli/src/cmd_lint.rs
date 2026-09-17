//! `noct lint` — run the official Noctivue linter.
//!
//! [Phase 4 / M3] Default-on rules enforce things the grammar can't
//! (TOOLCHAIN.md §5). Style-preference rules (e.g. "prefer concise
//! over explicit declaration style") ship **default-off** per
//! DECISIONS.md Issue 7 — none exist yet; when the first lands it
//! must be opt-in, never silently on.
//!
//! Rule L-001 (default-on, P-002): a match arm whose pattern is a
//! bare `Ident(name)` where `name` equals a visible enum variant
//! name (top-level `Item::Enum` variants in the file, plus the
//! prelude constructors `Some`/`Ok`/`Err`/`None`) AND the arm is not
//! the last arm of the match is a binding that matches everything —
//! every arm below it is dead, and the match silently tests nothing.
//! The parser cannot reject it and the type checker treats it as a
//! legal binding, so only a lint can catch it.
//!
//! Deliberately NOT warned: trailing bare-`None` arms (the
//! codebase-wide `Some(v):`/`None:` idiom — positionally correct by
//! accident of order), `_` (parses as `Wildcard`, never `Ident`),
//! and `Variant(…)` arms (already the correct form).
//!
//! Detection for both rules runs pre-typeck on the parsed `Program`
//! (no type info needed, so no false positives on ordinary bindings
//! like `x`). Warnings go to stderr (or stdout as JSON with
//! `--json`); exit code stays 0 on warnings (they are warnings, not
//! errors). `--fix` is honestly unimplemented.
//!
//! Rule L-002 (default-on): a top-level `import`/`use` binding that
//! is never referenced anywhere in the file (expression idents,
//! callee positions, type expressions, struct-literal type names —
//! never patterns, which bind rather than reference, and never bare
//! member-field names). Shadowing errs toward silence (a missed
//! warning, never a false positive).
//!
//! Usage:
//!   noct lint [file.nv ...] [--json] [--fix]
//!   noct lint --help

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use compiler::ast::{
    Block, Expr, FunctionBody, FunctionDecl, Item, MatchStmt, Pattern, Program, Stmt, TypeExpr,
};

pub fn run(args: &[String]) -> i32 {
    let mut fix_mode = false;
    let mut json_mode = false;
    let mut files: Vec<String> = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--fix" => fix_mode = true,
            "--json" => json_mode = true,
            "--help" | "-h" => {
                println!("noct lint — run the official Noctivue linter");
                println!();
                println!("USAGE:");
                println!("    noct lint [file.nv ...] [--json] [--fix]");
                println!();
                println!("OPTIONS:");
                println!("    --json  Output warnings as JSON to stdout");
                println!("    --fix   Apply L-001 autofixes (bare-ident → Ident() in match arms)");
                return 0;
            }
            other => {
                if other.starts_with('-') {
                    eprintln!("noct lint: unknown flag `{other}`");
                    return 1;
                }
                files.push(other.to_string());
            }
        }
    }

    if files.is_empty() {
        // Lint all .nv files in current directory.
        let entries = match std::fs::read_dir(".") {
            Ok(e) => e,
            Err(e) => {
                eprintln!("noct lint: cannot read directory: {e}");
                return 1;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("nv") {
                if let Some(file) = path.to_str() {
                    files.push(file.to_string());
                }
            }
        }
    }

    // (file, source, warnings) in argument order — JSON mode needs them buffered.
    let mut all: Vec<(String, String, Vec<Warning>)> = Vec::new();
    for file in &files {
        let path = Path::new(file);
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("noct lint: {file}: cannot read: {e}");
                return 1;
            }
        };
        let warnings = lint_file(&source);
        all.push((file.clone(), source, warnings));
    }
    let warnings: usize = all.iter().map(|(_, _, ws)| ws.len()).sum();

    if json_mode {
        print_json(&all);
    } else {
        for (file, _, ws) in &all {
            for w in ws {
                eprintln!("{file}:{}: {}", w.line, w.message);
                for extra in &w.details {
                    eprintln!("{extra}");
                }
            }
        }
    }

    if fix_mode && warnings > 0 {
        let mut fixed = 0usize;
        for (file, source, ws) in &all {
            if let Some(fixed_src) = apply_fixes(source, ws) {
                if let Err(e) = fs::write(file, fixed_src) {
                    eprintln!("noct lint: cannot write fix to {file}: {e}");
                    return 1;
                }
                fixed += 1;
            }
        }
        if fixed > 0 {
            eprintln!("noct lint: fixed {fixed} file(s)");
        } else {
            eprintln!("noct lint: --fix found no autofixable warnings");
        }
        // After fixing, re-lint to report any remaining warnings.
        eprintln!("noct lint: re-run lint to check remaining warnings");
    }
    0
}

/// Apply all L-001 fixes (bare-ident → `Ident()` in match arms) to source.
/// Returns the fixed source if any changes were made, None otherwise.
/// Fixes are applied right-to-left so byte offsets stay valid.
fn apply_fixes(source: &str, warnings: &[Warning]) -> Option<String> {
    let fixes: Vec<&Fix> = warnings.iter().filter_map(|w| w.fix.as_ref()).collect();
    if fixes.is_empty() {
        return None;
    }
    let mut fixed = source.to_string();
    let mut sorted: Vec<&Fix> = fixes;
    sorted.sort_by(|a, b| b.start.cmp(&a.start));
    for fix in sorted {
        fixed.replace_range(fix.start..fix.end, &fix.replacement);
    }
    Some(fixed)
}

/// Machine-readable form: one JSON array to stdout (stderr stays
/// clean for piping). Hand-rolled escaping — dependency-free like
/// the manifest parser, and the schema is three fields plus details.
fn print_json(all: &[(String, String, Vec<Warning>)]) {
    fn esc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out
    }
    let mut items = Vec::new();
    for (file, ws) in all.iter().map(|(f, _, ws)| (f, ws)) {
        for w in ws {
            let details: Vec<String> = w
                .details
                .iter()
                .map(|d| format!("\"{}\"", esc(d)))
                .collect();
            items.push(format!(
                "{{\"rule\":\"{}\",\"file\":\"{}\",\"line\":{},\"message\":\"{}\",\"details\":[{}]}}",
                w.rule,
                esc(file),
                w.line,
                esc(&w.message),
                details.join(",")
            ));
        }
    }
    println!("[{}]", items.join(","));
}

/// Parse `source` (lex+parse only — both rules run pre-typeck by
/// design) and return all warnings. Parse errors never block the
/// rules (the parser returns a partial tree; reporting syntax is
/// `diagnostics`' job, not lint's).
fn lint_file(source: &str) -> Vec<Warning> {
    let mut sink = compiler::diagnostics::DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);

    let mut out = Vec::new();
    let variants = collect_variant_names(&program);
    check_program(&program, &variants, source, &mut out);
    check_unused_imports(&program, source, &mut out);
    // Stable order: by line, L-001 before L-002 on ties.
    out.sort_by(|a, b| (a.line, a.rule).cmp(&(b.line, b.rule)));
    out
}

struct Warning {
    rule: &'static str,
    line: usize,
    message: String,
    details: Vec<String>,
    fix: Option<Fix>,
}

/// A source-level edit: replace bytes [start, end) with `replacement`.
struct Fix {
    start: usize,
    end: usize,
    replacement: String,
}

fn warn_l001(name: &str, line: usize, span_start: usize) -> Warning {
    Warning {
        rule: "L-001",
        line,
        message: format!(
            "warning[L-001]: `{name}` here is a binding that matches everything, not a test for the `{name}` variant"
        ),
        details: vec![
            "  = note: arms below this one are unreachable".to_string(),
            format!("  = help: match the variant with `{name}():` (or `{name}(x):` for payloads)"),
        ],
        fix: Some(Fix {
            start: span_start,
            end: span_start + name.len(),
            replacement: format!("{name}()"),
        }),
    }
}

fn warn_l002(name: &str, line: usize) -> Warning {
    Warning {
        rule: "L-002",
        line,
        message: format!("warning[L-002]: unused import `{name}`"),
        details: vec![
            format!("  = note: `{name}` is never referenced in this file"),
            "  = help: remove the import or use it".to_string(),
        ],
        fix: None,
    }
}

/// Visible enum variant names: top-level `Item::Enum` variants plus
/// the four prelude constructors.
fn collect_variant_names(program: &Program) -> HashSet<String> {
    let mut set: HashSet<String> = ["Some", "Ok", "Err", "None"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for item in &program.items {
        if let Item::Enum(e) = item {
            for v in &e.variants {
                set.insert(v.name.clone());
            }
        }
    }
    set
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

fn check_program(
    program: &Program,
    variants: &HashSet<String>,
    source: &str,
    out: &mut Vec<Warning>,
) {
    for item in &program.items {
        check_item(item, variants, source, out);
    }
}

fn check_item(item: &Item, variants: &HashSet<String>, source: &str, out: &mut Vec<Warning>) {
    match item {
        Item::Function(f) => check_function(f, variants, source, out),
        Item::Task(t) => check_block(&t.body, variants, source, out),
        Item::BareDecl(b) => check_block(&b.body, variants, source, out),
        Item::Impl(ib) => {
            for m in &ib.methods {
                check_function(m, variants, source, out);
            }
        }
        Item::Mod(m) => check_stmts(&m.items, variants, source, out),
        Item::Export(inner) => check_item(inner, variants, source, out),
        _ => {}
    }
}

fn check_function(
    f: &FunctionDecl,
    variants: &HashSet<String>,
    source: &str,
    out: &mut Vec<Warning>,
) {
    if let FunctionBody::Block(b) = &f.body {
        check_block(b, variants, source, out);
    }
}

fn check_block(block: &Block, variants: &HashSet<String>, source: &str, out: &mut Vec<Warning>) {
    check_stmts(&block.stmts, variants, source, out);
}

fn check_stmts(stmts: &[Stmt], variants: &HashSet<String>, source: &str, out: &mut Vec<Warning>) {
    for stmt in stmts {
        match stmt {
            Stmt::Match(m) => check_match(m, variants, source, out),
            Stmt::If(i) => {
                check_block(&i.then_block, variants, source, out);
                for (_, b) in &i.else_if_clauses {
                    check_block(b, variants, source, out);
                }
                if let Some(b) = &i.else_block {
                    check_block(b, variants, source, out);
                }
            }
            Stmt::While(w) => check_block(&w.body, variants, source, out),
            Stmt::Loop(l) => check_block(&l.body, variants, source, out),
            Stmt::For(f) => check_block(&f.body, variants, source, out),
            Stmt::Function(f) => check_function(f, variants, source, out),
            Stmt::Task(t) => check_block(&t.body, variants, source, out),
            _ => {}
        }
    }
}

fn check_match(m: &MatchStmt, variants: &HashSet<String>, source: &str, out: &mut Vec<Warning>) {
    let last = m.arms.len().saturating_sub(1);
    for (i, arm) in m.arms.iter().enumerate() {
        // Non-final bare-Ident arm naming a visible variant: the trap.
        // (`_` parses as Wildcard, never Ident; trailing arms are out
        // by position — including the `Some(v):`/`None:` idiom.)
        if i < last {
            if let Pattern::Ident(name, span) = &arm.pattern {
                if name != "_" && variants.contains(name) {
                    out.push(warn_l001(name, line_of(source, span.start), span.start));
                }
            }
        }
        // Nested matches hide in arm bodies (block form only —
        // expression bodies contain no statements by construction).
        if let compiler::ast::MatchBody::Block(b) = &arm.body {
            check_block(b, variants, source, out);
        }
    }
}

// ── L-002: unused imports ─────────────────────────────────────────────────────

/// Warn on top-level imports whose bound name never appears in a
/// reference position. Reference positions are expression idents
/// (including callee and member-base positions), type-expression
/// names, struct-literal type names, and generic bounds. NOT
/// references: patterns (they bind), member-field names, argument
/// labels, and declarations. Shadowing errs toward silence.
fn check_unused_imports(program: &Program, source: &str, out: &mut Vec<Warning>) {
    if program.imports.is_empty() {
        return;
    }
    let mut refs: HashSet<String> = HashSet::new();
    for item in &program.items {
        collect_item_refs(item, &mut refs);
    }
    for import in &program.imports {
        let bound = import
            .alias
            .clone()
            .unwrap_or_else(|| import.path.last().cloned().unwrap_or_default());
        if bound.is_empty() || bound == "_" {
            continue;
        }
        if !refs.contains(&bound) {
            out.push(warn_l002(&bound, line_of(source, import.span.start)));
        }
    }
}

fn collect_item_refs(item: &Item, refs: &mut HashSet<String>) {
    match item {
        Item::Function(f) => {
            for p in &f.params {
                collect_type_refs(&p.ty, refs);
                if let Some(v) = &p.default {
                    collect_expr_refs(v, refs);
                }
            }
            if let Some(t) = &f.return_ty {
                collect_type_refs(t, refs);
            }
            for g in &f.generic_params {
                refs.extend(g.bounds.iter().cloned());
            }
            match &f.body {
                FunctionBody::Block(b) => collect_block_refs(b, refs),
                FunctionBody::Expr(e) => collect_expr_refs(e, refs),
            }
        }
        Item::Task(t) => {
            for p in &t.params {
                collect_type_refs(&p.ty, refs);
                if let Some(v) = &p.default {
                    collect_expr_refs(v, refs);
                }
            }
            collect_block_refs(&t.body, refs);
        }
        // Derive decls name a trait + type only — no references to
        // collect (expansion happens in the resolver, past lint).
        Item::Derive(_) => {}
        Item::BareDecl(b) => collect_block_refs(&b.body, refs),
        Item::Struct(s) => {
            for _ in &s.generic_params {}
            for f in &s.fields {
                collect_type_refs(&f.ty, refs);
            }
        }
        Item::Enum(e) => {
            for v in &e.variants {
                for t in &v.fields {
                    collect_type_refs(t, refs);
                }
            }
        }
        Item::Trait(t) => {
            for m in &t.members {
                for p in &m.params {
                    collect_type_refs(&p.ty, refs);
                }
                if let Some(r) = &m.return_ty {
                    collect_type_refs(r, refs);
                }
            }
        }
        Item::Impl(ib) => {
            if let Some(t) = &ib.for_trait {
                collect_type_refs(t, refs);
            }
            for m in &ib.methods {
                collect_item_refs(&Item::Function(m.clone()), refs);
            }
        }
        Item::Const(c) => {
            collect_type_refs(&c.ty, refs);
            collect_expr_refs(&c.value, refs);
        }
        Item::Mod(m) => {
            for s in &m.items {
                collect_stmt_refs(s, refs);
            }
        }
        Item::Export(inner) => collect_item_refs(inner, refs),
    }
}

fn collect_block_refs(block: &Block, refs: &mut HashSet<String>) {
    for s in &block.stmts {
        collect_stmt_refs(s, refs);
    }
}

fn collect_stmt_refs(stmt: &Stmt, refs: &mut HashSet<String>) {
    use compiler::ast::MatchBody;
    match stmt {
        Stmt::Let(l) => {
            if let Some(t) = &l.ty {
                collect_type_refs(t, refs);
            }
            collect_expr_refs(&l.value, refs);
        }
        Stmt::Var(v) => {
            if let Some(t) = &v.ty {
                collect_type_refs(t, refs);
            }
            collect_expr_refs(&v.value, refs);
        }
        Stmt::State(s) => collect_expr_refs(&s.value, refs),
        Stmt::Assign(a) => {
            collect_expr_refs(&a.target, refs);
            collect_expr_refs(&a.value, refs);
        }
        Stmt::Expr(e) => collect_expr_refs(e, refs),
        Stmt::Return(r) => {
            if let Some(v) = &r.value {
                collect_expr_refs(v, refs);
            }
        }
        Stmt::Break(b) => {
            if let Some(v) = &b.value {
                collect_expr_refs(v, refs);
            }
        }
        Stmt::Continue(_) => {}
        Stmt::If(i) => {
            collect_expr_refs(&i.condition, refs);
            collect_block_refs(&i.then_block, refs);
            for (c, b) in &i.else_if_clauses {
                collect_expr_refs(c, refs);
                collect_block_refs(b, refs);
            }
            if let Some(b) = &i.else_block {
                collect_block_refs(b, refs);
            }
        }
        Stmt::While(w) => {
            collect_expr_refs(&w.condition, refs);
            collect_block_refs(&w.body, refs);
        }
        Stmt::Loop(l) => collect_block_refs(&l.body, refs),
        Stmt::For(f) => {
            collect_expr_refs(&f.iterable, refs);
            collect_block_refs(&f.body, refs);
        }
        Stmt::Match(m) => {
            collect_expr_refs(&m.scrutinee, refs);
            for arm in &m.arms {
                if let Some(g) = &arm.guard {
                    collect_expr_refs(g, refs);
                }
                match &arm.body {
                    MatchBody::Block(b) => collect_block_refs(b, refs),
                    MatchBody::Expr(e) => collect_expr_refs(e, refs),
                }
            }
        }
        Stmt::Function(f) => collect_item_refs(&Item::Function(f.clone()), refs),
        Stmt::Task(t) => collect_item_refs(&Item::Task(t.clone()), refs),
        Stmt::Struct(s) => collect_item_refs(&Item::Struct(s.clone()), refs),
        Stmt::BareField(f) => collect_type_refs(&f.ty, refs),
        Stmt::Decl(d) => collect_expr_refs(&d.value, refs),
    }
}

fn collect_expr_refs(expr: &Expr, refs: &mut HashSet<String>) {
    use compiler::ast::{InterpPart, StructField};
    match expr {
        Expr::Literal(_, _) => {}
        Expr::Ident(name, _) => {
            refs.insert(name.clone());
        }
        Expr::Call(c) => {
            collect_expr_refs(&c.callee, refs);
            for a in &c.args {
                collect_expr_refs(&a.value, refs);
            }
            if let Some(b) = &c.trailing_block {
                collect_block_refs(b, refs);
            }
        }
        Expr::Member(m) => {
            // Only the object is a reference; the field name is not.
            collect_expr_refs(&m.object, refs);
        }
        Expr::Index(i) => {
            collect_expr_refs(&i.object, refs);
            collect_expr_refs(&i.index, refs);
        }
        Expr::BinOp(b) => {
            collect_expr_refs(&b.left, refs);
            collect_expr_refs(&b.right, refs);
        }
        Expr::UnaryOp(u) => collect_expr_refs(&u.operand, refs),
        Expr::Try(t) => collect_expr_refs(&t.expr, refs),
        Expr::Range(r) => {
            collect_expr_refs(&r.start, refs);
            collect_expr_refs(&r.end, refs);
        }
        Expr::StringInterp(s) => {
            for part in &s.parts {
                if let InterpPart::Expr(e) = part {
                    collect_expr_refs(e, refs);
                }
            }
        }
        Expr::StructLit(s) => {
            // The type name IS a reference (constructing needs it).
            refs.insert(s.name.clone());
            for f in &s.fields {
                match f {
                    StructField::Named(_, v) => collect_expr_refs(v, refs),
                    StructField::Spread(e) => collect_expr_refs(e, refs),
                }
            }
        }
        Expr::ListLit(l) => {
            for e in &l.elements {
                collect_expr_refs(e, refs);
            }
        }
        Expr::Closure(c) => collect_expr_refs(&c.body, refs),
        Expr::Spread(s) => collect_expr_refs(&s.expr, refs),
    }
}

fn collect_type_refs(ty: &TypeExpr, refs: &mut HashSet<String>) {
    match ty {
        TypeExpr::Named(name, args, _) => {
            refs.insert(name.clone());
            for a in args {
                collect_type_refs(a, refs);
            }
        }
        TypeExpr::Tuple(elems, _) => {
            for e in elems {
                collect_type_refs(e, refs);
            }
        }
        TypeExpr::Collection(inner, _) => collect_type_refs(inner, refs),
        TypeExpr::Function(params, ret, _) => {
            for p in params {
                collect_type_refs(p, refs);
            }
            collect_type_refs(ret, refs);
        }
    }
}

/// Import path → bound-name rendering for messages (alias wins).
#[allow(dead_code)]
fn import_display(path: &[String], alias: &Option<String>) -> String {
    match alias {
        Some(a) => format!("{} as {a}", path.join("::")),
        None => path.join("::"),
    }
}
