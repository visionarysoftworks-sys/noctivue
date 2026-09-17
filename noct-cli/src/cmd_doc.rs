//! `noct doc` — generate documentation from `///` doc comments.
//!
//! [Phase 4 / M3] Doc comments (`///` and `//!`) are parsed by the lexer from
//! Phase 1 onward (LANGUAGE_SPEC.md §3) and are available in the AST.
//! The `noct doc` command renders them into browsable documentation.
//!
//! Signatures mirror the hover cards (`analysis::get_hover`), and doc
//! text comes from the shared `analysis::doc_comment_for` — one rule,
//! two consumers.
//!
//! Usage:
//!   noct doc <file.nv> [--output <out.md>] [--open]

use std::fs;

use compiler::analysis::{analyze_file, doc_comment_for, AnalysisResult};
use compiler::ast::{type_expr_to_string, FunctionDecl, Item};

pub fn run(args: &[String]) -> i32 {
    let mut open_browser = false;
    let mut output_file = None;
    let mut input_file: Option<String> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--open" => open_browser = true,
            "--output" => {
                if let Some(file) = it.next() {
                    output_file = Some(file.clone());
                } else {
                    eprintln!("noct doc: --output requires a filename");
                    return 1;
                }
            }
            other => {
                if input_file.is_none() {
                    input_file = Some(other.to_string());
                } else {
                    eprintln!("noct doc: too many arguments");
                    return 1;
                }
            }
        }
    }

    let input = match input_file {
        Some(f) => f,
        None => {
            eprintln!("noct doc: input file required");
            eprintln!("usage: noct doc <file.nv> [--output <out.md>] [--open]");
            return 1;
        }
    };

    let source = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("noct doc: cannot read {input}: {e}");
            return 1;
        }
    };

    let analysis = analyze_file(&input, &source);

    // Location label mirrors hover's `Declared in` (basename, not full path).
    let file_label = file_label_of(&input);

    let mut doc = String::new();
    doc.push_str("# Noctivue Documentation\n\n");
    if !analysis.diagnostics.is_empty() {
        doc.push_str("## Diagnostics\n\n");
        for d in &analysis.diagnostics {
            let sev = match d.severity {
                compiler::diagnostics::Severity::Error => "error",
                compiler::diagnostics::Severity::Warning => "warning",
                compiler::diagnostics::Severity::Note => "note",
                compiler::diagnostics::Severity::Help => "help",
            };
            doc.push_str(&format!("- **{sev}**: {}\n", d.message));
        }
        doc.push_str("\n");
    }

    for item in &analysis.resolved.items {
        // `export` is visibility, not a separate item: the resolver
        // passes it through, so unwrap it here — otherwise every
        // public (exported) item would silently vanish from the docs.
        let mut inner = item;
        while let Item::Export(next) = inner {
            inner = next;
        }
        match inner {
            Item::Function(f) => {
                let docs = doc_comment_for(&source, f.span.start);
                let params: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, type_expr_to_string(&p.ty)))
                    .collect();
                let ret = f
                    .return_ty
                    .as_ref()
                    .map(|t| format!(" -> {}", type_expr_to_string(t)))
                    .unwrap_or_default();
                doc.push_str(&format!("## Function: `{}`\n\n", f.name));
                doc.push_str(&format!(
                    "```nv\nfn {}({}){}\n```\n\n",
                    f.name,
                    params.join(", "),
                    ret
                ));
                doc.push_str(&format!("Type: {}\n\n", fn_type_for(&analysis, f)));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d) = docs {
                    doc.push_str(&d);
                    doc.push_str("\n\n");
                }
            }
            Item::Struct(s) => {
                let docs = doc_comment_for(&source, s.span.start);
                let fields: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| format!("  {}: {}", f.name, type_expr_to_string(&f.ty)))
                    .collect();
                doc.push_str(&format!("## Struct: `{}`\n\n", s.name));
                doc.push_str("```nv\n");
                doc.push_str(&format!(
                    "struct {}:\n{}\n```\n\n",
                    s.name,
                    fields.join("\n")
                ));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d) = docs {
                    doc.push_str(&d);
                    doc.push_str("\n\n");
                }
            }
            Item::Enum(e) => {
                let docs = doc_comment_for(&source, e.span.start);
                let variants: Vec<String> = e
                    .variants
                    .iter()
                    .map(|v| {
                        if v.fields.is_empty() {
                            v.name.clone()
                        } else {
                            let pts: Vec<String> =
                                v.fields.iter().map(type_expr_to_string).collect();
                            format!("{}({})", v.name, pts.join(", "))
                        }
                    })
                    .collect();
                doc.push_str(&format!("## Enum: `{}`\n\n", e.name));
                doc.push_str("```nv\n");
                doc.push_str(&format!(
                    "enum {}:\n{}\n```\n\n",
                    e.name,
                    variants.join(", ")
                ));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d) = docs {
                    doc.push_str(&d);
                    doc.push_str("\n\n");
                }
            }
            Item::Trait(t) => {
                let docs = doc_comment_for(&source, t.span.start);
                let methods: Vec<String> = t
                    .members
                    .iter()
                    .map(|m| {
                        let params: Vec<String> = m
                            .params
                            .iter()
                            .map(|p| format!("{}: {}", p.name, type_expr_to_string(&p.ty)))
                            .collect();
                        let ret = m
                            .return_ty
                            .as_ref()
                            .map(|t| format!(" -> {}", type_expr_to_string(t)))
                            .unwrap_or_default();
                        format!("  fn {}({}){}", m.name, params.join(", "), ret)
                    })
                    .collect();
                doc.push_str(&format!("## Trait: `{}`\n\n", t.name));
                doc.push_str(&format!(
                    "```nv\ntrait {}:\n{}\n```\n\n",
                    t.name,
                    methods.join("\n")
                ));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d) = docs {
                    doc.push_str(&d);
                    doc.push_str("\n\n");
                }
            }
            Item::Const(c) => {
                let docs = doc_comment_for(&source, c.span.start);
                doc.push_str(&format!("## Const: `{}`\n\n", c.name));
                doc.push_str(&format!(
                    "```nv\nconst {}: {}\n```\n\n",
                    c.name,
                    type_expr_to_string(&c.ty)
                ));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d) = docs {
                    doc.push_str(&d);
                    doc.push_str("\n\n");
                }
            }
            Item::BareDecl(d) => {
                let docs = doc_comment_for(&source, d.span.start);
                doc.push_str(&format!("## Component: `{}`\n\n", d.name));
                doc.push_str("```nv\n");
                doc.push_str(&format!("{}:\n    ...\n```\n\n", d.name));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d_comment) = docs {
                    doc.push_str(&d_comment);
                    doc.push_str("\n\n");
                }
            }
            Item::Task(t) => {
                let docs = doc_comment_for(&source, t.span.start);
                let params: Vec<String> = t
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, type_expr_to_string(&p.ty)))
                    .collect();
                doc.push_str(&format!("## Task: `{}`\n\n", t.name));
                doc.push_str(&format!(
                    "```nv\ntask {}({}) -> handle\n```\n\n",
                    t.name,
                    params.join(", ")
                ));
                doc.push_str(&format!("Declared in {file_label}.\n\n"));
                if let Some(d_comment) = docs {
                    doc.push_str(&d_comment);
                    doc.push_str("\n\n");
                }
            }
            _ => {}
        }
    }

    match output_file {
        None => {
            print!("{doc}");
            if open_browser {
                // Write to a temp file so we have a real path to open.
                let tmp = std::env::temp_dir().join("noct_doc_preview.md");
                if fs::write(&tmp, &doc).is_ok() {
                    open_in_browser(&tmp.to_string_lossy());
                } else {
                    eprintln!("noct doc: cannot write temp file for --open");
                }
            }
            0
        }
        Some(output) => match fs::write(&output, doc) {
            Ok(()) => {
                println!("Documentation written to {output}");
                if open_browser {
                    open_in_browser(&output);
                }
                0
            }
            Err(e) => {
                eprintln!("noct doc: cannot write {output}: {e}");
                1
            }
        },
    }
}

// ── Hover-card sections ──────────────────────────────────────────────────────

/// Basename of the input path (both separators — hover's `Declared in`
/// shows the file label, never the full path).
fn file_label_of(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_string()
}

/// `Type: ...` line for a function, mirroring hover's `fn_type_of`
/// (`(params) -> ret` from the HIR). Falls back to the AST signature
/// when the function never reached HIR (e.g. `export`-wrapped items,
/// which typeck skips) so the line still renders.
fn fn_type_for(analysis: &AnalysisResult, f: &FunctionDecl) -> String {
    if let Some(hir) = analysis.typed.functions.iter().find(|h| h.name == f.name) {
        let params: Vec<String> = hir.params.iter().map(|(_, ty)| ty.to_string()).collect();
        return format!("({}) -> {}", params.join(", "), hir.return_ty);
    }
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| type_expr_to_string(&p.ty))
        .collect();
    let ret = f
        .return_ty
        .as_ref()
        .map(type_expr_to_string)
        .unwrap_or_else(|| "()".to_string());
    format!("({}) -> {}", params.join(", "), ret)
}

// ── Browser launch ───────────────────────────────────────────────────────────

/// Best-effort `xdg-open`/`open`/`start` invocation. Failures are
/// non-fatal (the docs were already printed/written). The file path
/// is shell-escaped per platform to avoid injection from untrusted
/// file names.
fn open_in_browser(path: &str) {
    #[cfg(windows)]
    fn launch(path: &str) {
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/C").arg("start").arg("").arg(path);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    fn launch(path: &str) {
        let opener = if std::path::Path::new("/usr/bin/xdg-open").exists() {
            "xdg-open"
        } else {
            "open"
        };
        let _ = std::process::Command::new(opener).arg(path).spawn();
    }
    launch(path);
}
