//! `noct diagnostics` — dump compiler diagnostics for a `.nv` file as JSON.
//!
//! [Phase 1] AI-native compiler interface (AI_TOOLING.md §2, ADR-011).
//! Runs the full compiler frontend (lex → parse → resolve → typecheck) and
//! emits all diagnostics as structured JSON to stdout.
//!
//! Usage:
//!   noct diagnostics [file.nv]
//!   noct diagnostics --json [file.nv]

use std::fs;
use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

use crate::cmd_ast::{diag_to_json, json_string};

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

    // Full pipeline: lex → parse → resolve → typecheck
    let tokens = compiler::lexer::lex(&source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let _module = compiler::typeck::typecheck(program, &mut sink);

    // Emit JSON
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"source_file\": {},\n", json_string(path)));
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
