//! `noct ast` — dump the AST of a `.nv` file as JSON.
//!
//! [Phase 1] AI-native compiler interface (AI_TOOLING.md §2, ADR-011).
//!
//! Usage:
//!   noct ast [file.nv]
//!   noct ast --json [file.nv]

use std::fs;
use std::path::Path;

use compiler::ast::{FunctionBody, Item};
use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // Strip --json flag (it's the default / only output format for now)
    let path = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .unwrap_or("main.nv");

    let source = match fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{path}`: {e}");
            return 1;
        }
    };

    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(&source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);

    // Build JSON output
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"source_file\": {},\n", json_string(path)));

    // items array
    out.push_str("  \"items\": [\n");
    let items: Vec<String> = program.items.iter().map(item_to_json).collect();
    for (i, item_json) in items.iter().enumerate() {
        out.push_str(item_json);
        if i + 1 < items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ],\n");

    out.push_str(&format!("  \"import_count\": {},\n", program.imports.len()));

    // diagnostics array
    out.push_str("  \"diagnostics\": [\n");
    let diags: Vec<String> = sink.diagnostics().iter().map(diag_to_json).collect();
    for (i, dj) in diags.iter().enumerate() {
        out.push_str(dj);
        if i + 1 < diags.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n");
    out.push('}');

    println!("{out}");
    0
}

// ── Item serialization ────────────────────────────────────────────────────────

fn item_to_json(item: &Item) -> String {
    match item {
        Item::BareDecl(d) => {
            let params = match &d.params {
                None => "null".to_string(),
                Some(ps) => {
                    let parts: Vec<String> = ps
                        .iter()
                        .map(|p| {
                            format!(
                                "{{\"name\": {}, \"ty\": {}}}",
                                json_string(&p.name),
                                json_string(&type_expr_str(&p.ty))
                            )
                        })
                        .collect();
                    format!("[{}]", parts.join(", "))
                }
            };
            let return_ty = match &d.return_ty {
                None => "null".to_string(),
                Some(ty) => json_string(&type_expr_str(ty)),
            };
            let body_stmt_count = d.body.stmts.len();
            format!(
                "    {{\"kind\": \"BareDecl\", \"name\": {}, \"params\": {params}, \
                 \"return_ty\": {return_ty}, \"body_stmt_count\": {body_stmt_count}}}",
                json_string(&d.name)
            )
        }
        Item::Function(f) => {
            let params: Vec<String> = f
                .params
                .iter()
                .map(|p| {
                    format!(
                        "{{\"name\": {}, \"ty\": {}}}",
                        json_string(&p.name),
                        json_string(&type_expr_str(&p.ty))
                    )
                })
                .collect();
            let return_ty = f
                .return_ty
                .as_ref()
                .map(|t| json_string(&type_expr_str(t)))
                .unwrap_or_else(|| "null".to_string());
            let body_stmt_count = match &f.body {
                FunctionBody::Block(b) => b.stmts.len(),
                FunctionBody::Expr(_) => 1,
            };
            format!(
                "    {{\"kind\": \"Function\", \"name\": {}, \"params\": [{}], \
                 \"return_ty\": {return_ty}, \"body_stmt_count\": {body_stmt_count}}}",
                json_string(&f.name),
                params.join(", ")
            )
        }
        Item::Task(t) => {
            let params: Vec<String> = t
                .params
                .iter()
                .map(|p| {
                    format!(
                        "{{\"name\": {}, \"ty\": {}}}",
                        json_string(&p.name),
                        json_string(&type_expr_str(&p.ty))
                    )
                })
                .collect();
            format!(
                "    {{\"kind\": \"Task\", \"name\": {}, \"params\": [{}], \
                 \"body_stmt_count\": {}}}",
                json_string(&t.name),
                params.join(", "),
                t.body.stmts.len()
            )
        }
        Item::Struct(s) => {
            let fields: Vec<String> = s
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "{{\"name\": {}, \"ty\": {}}}",
                        json_string(&f.name),
                        json_string(&type_expr_str(&f.ty))
                    )
                })
                .collect();
            format!(
                "    {{\"kind\": \"Struct\", \"name\": {}, \"fields\": [{}]}}",
                json_string(&s.name),
                fields.join(", ")
            )
        }
        Item::Enum(e) => {
            let variants: Vec<String> = e
                .variants
                .iter()
                .map(|v| {
                    format!(
                        "{{\"name\": {}, \"field_count\": {}}}",
                        json_string(&v.name),
                        v.fields.len()
                    )
                })
                .collect();
            format!(
                "    {{\"kind\": \"Enum\", \"name\": {}, \"variants\": [{}]}}",
                json_string(&e.name),
                variants.join(", ")
            )
        }
        Item::Const(c) => {
            format!(
                "    {{\"kind\": \"Const\", \"name\": {}, \"ty\": {}}}",
                json_string(&c.name),
                json_string(&type_expr_str(&c.ty))
            )
        }
        Item::Trait(t) => {
            format!(
                "    {{\"kind\": \"Trait\", \"name\": {}}}",
                json_string(&t.name)
            )
        }
        Item::Impl(i) => {
            format!(
                "    {{\"kind\": \"Impl\", \"ty\": {}, \"method_count\": {}}}",
                json_string(&type_expr_str(&i.ty)),
                i.methods.len()
            )
        }
        Item::Export(inner) => {
            let inner_json = item_to_json(inner);
            // Wrap it, remove the leading 4-space indent from inner
            let trimmed = inner_json.trim_start();
            format!("    {{\"kind\": \"Export\", \"item\": {trimmed}}}")
        }
        Item::Mod(m) => {
            format!(
                "    {{\"kind\": \"Mod\", \"name\": {}, \"item_count\": {}}}",
                json_string(&m.name),
                m.items.len()
            )
        }
    }
}

// ── Diagnostic serialization ──────────────────────────────────────────────────

pub(crate) fn diag_to_json(diag: &compiler::diagnostics::Diagnostic) -> String {
    let severity = match diag.severity {
        compiler::diagnostics::Severity::Error => "error",
        compiler::diagnostics::Severity::Warning => "warning",
        compiler::diagnostics::Severity::Note => "note",
        compiler::diagnostics::Severity::Help => "help",
    };
    let code = diag
        .code
        .as_deref()
        .map(json_string)
        .unwrap_or_else(|| "null".to_string());

    let labels: Vec<String> = diag
        .labels
        .iter()
        .map(|l| {
            format!(
                "{{\"span\": {{\"start\": {}, \"end\": {}}}, \"message\": {}}}",
                l.span.start,
                l.span.end,
                json_string(&l.message)
            )
        })
        .collect();

    let notes: Vec<String> = diag.notes.iter().map(|n| json_string(n)).collect();

    let suggested_fix = match &diag.suggested_fix {
        Some(s) => format!(
            "{{\"message\": {}, \"replacement\": {}, \"span\": {{\"start\": {}, \"end\": {}}}}}",
            json_string(&s.message),
            json_string(&s.replacement),
            s.span.start,
            s.span.end,
        ),
        None => "null".to_string(),
    };

    format!(
        "    {{\"severity\": {}, \"code\": {code}, \"message\": {}, \
         \"labels\": [{}], \"notes\": [{}], \"suggested_fix\": {suggested_fix}}}",
        json_string(severity),
        json_string(&diag.message),
        labels.join(", "),
        notes.join(", ")
    )
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Serialize a string as a JSON string literal, escaping special characters.
pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Convert a TypeExpr to a human-readable string for JSON serialization.
pub(crate) fn type_expr_str(ty: &compiler::ast::TypeExpr) -> String {
    match ty {
        compiler::ast::TypeExpr::Named(name, args, _) => {
            if args.is_empty() {
                name.clone()
            } else {
                let arg_strs: Vec<String> = args.iter().map(type_expr_str).collect();
                format!("{}<{}>", name, arg_strs.join(", "))
            }
        }
        compiler::ast::TypeExpr::Tuple(elems, _) => {
            let parts: Vec<String> = elems.iter().map(type_expr_str).collect();
            format!("({})", parts.join(", "))
        }
        compiler::ast::TypeExpr::Collection(inner, _) => {
            format!("[{}]", type_expr_str(inner))
        }
        compiler::ast::TypeExpr::Function(params, ret, _) => {
            let param_strs: Vec<String> = params.iter().map(type_expr_str).collect();
            format!("({}) -> {}", param_strs.join(", "), type_expr_str(ret))
        }
    }
}
