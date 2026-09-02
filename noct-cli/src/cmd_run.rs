//! `noct run` — build and execute a `.nv` program via the tree-walking interpreter.
//!
//! [Phase 1] Pipeline: lex → parse → resolve → typecheck → interpret.
//!
//! Usage:
//!   noct run [file.nv]
//!   noct run          # runs main.nv in the current package

use std::fs;
use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // 1. Resolve file path (first arg, or "main.nv" in cwd)
    let path = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .unwrap_or("main.nv");

    // 2. Read source file
    let source = match fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{path}`: {e}");
            return 1;
        }
    };

    // 3. Create diagnostic sink
    let mut sink = DiagnosticSink::new();

    // 4. Lex
    let tokens = compiler::lexer::lex(&source, &mut sink);

    // 5. Parse
    let program = compiler::parser::parse(&tokens, &mut sink);

    // 6. Resolve
    let program = compiler::resolver::resolve(program, &mut sink);

    // 7. Type-check
    let module = compiler::typeck::typecheck(program, &mut sink);

    // 8. Check for errors before interpreting
    if sink.has_errors() {
        print_diagnostics(path, &sink);
        return 1;
    }

    // 9. Run interpreter
    let mut interp = interp::Interpreter::new();
    let exit_code = interp.run(&module, &mut sink);

    // Print any diagnostics emitted during interpretation
    if sink.has_errors() {
        print_diagnostics(path, &sink);
    }

    exit_code
}

/// Print all diagnostics in the sink to stderr in a human-readable format.
pub(crate) fn print_diagnostics(path: &str, sink: &DiagnosticSink) {
    for diag in sink.diagnostics() {
        let severity = match diag.severity {
            compiler::diagnostics::Severity::Error => "error",
            compiler::diagnostics::Severity::Warning => "warning",
            compiler::diagnostics::Severity::Note => "note",
            compiler::diagnostics::Severity::Help => "help",
        };
        let code = diag
            .code
            .as_deref()
            .map(|c| format!("[{c}] "))
            .unwrap_or_default();
        eprintln!("{severity}: {code}{}", diag.message);
        for label in &diag.labels {
            eprintln!("  --> {path}:{}:{}", label.span.start, label.span.end);
            if !label.message.is_empty() {
                eprintln!("  | {}", label.message);
            }
        }
        for note in &diag.notes {
            eprintln!("  = note: {note}");
        }
    }
}
