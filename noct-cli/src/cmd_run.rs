//! `noct run` — build and execute a `.nv` program via the tree-walking interpreter.
//!
//! [Phase 1] Pipeline: lex → parse → resolve → typecheck → interpret.
//!
//! Usage:
//!   noct run [files...]
//!   noct run                       # runs main.nv in the current package
//!   noct run tank/math.nv app.nv   # multi-file: concatenated in order,
//!                                  # entry point last (see `read_sources`)

use std::fs;
use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

/// Read several `.nv` files and join them into one compilation unit.
///
/// PRE-MODULE STOPGAP (tank libraries): `import` parses but resolves
/// nothing, so multi-file programs are expressed as explicit file lists,
/// concatenated in order with a newline separator — libraries first,
/// entry point last. Returns the joined source plus `(path,
/// base_offset)` per file so diagnostics can name the owning file
/// (spans are whole-unit byte offsets; see `owner_file`). Dies the day
/// real imports land — do not build anything else on it.
pub(crate) fn read_sources(paths: &[&str]) -> Result<(String, Vec<(String, usize)>), String> {
    let mut joined = String::new();
    let mut files = Vec::new();
    for p in paths {
        let src = fs::read_to_string(Path::new(p))
            .map_err(|e| format!("error: cannot read `{p}`: {e}"))?;
        files.push((p.to_string(), joined.len()));
        joined.push_str(&src);
        if !joined.ends_with('\n') {
            joined.push('\n');
        }
    }
    Ok((joined, files))
}

/// Name the file owning a whole-unit byte offset (see `read_sources`).
/// Files are ordered by base offset, so the owner is the last file whose
/// base is at or before the offset. Wrong-owner output here would send
/// the operator to the wrong file — the multi-file error-path test in
/// the tank demos covers exactly this.
pub(crate) fn owner_file(files: &[(String, usize)], offset: usize) -> &str {
    let mut owner = "<unknown>";
    for (path, base) in files {
        if *base <= offset {
            owner = path.as_str();
        } else {
            break;
        }
    }
    owner
}

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    // 1. Resolve file paths (all non-flag args, or "main.nv" in cwd)
    let paths: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();
    let paths = if paths.is_empty() {
        vec!["main.nv"]
    } else {
        paths
    };

    // 2. Read + join source files
    let (source, files) = match read_sources(&paths) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
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
        print_diagnostics_multi(&files, &sink);
        return 1;
    }

    // 9. Run interpreter
    let mut interp = interp::Interpreter::new();
    let exit_code = interp.run(&module, &mut sink);

    // Print any diagnostics emitted during interpretation
    if sink.has_errors() {
        print_diagnostics_multi(&files, &sink);
    }

    exit_code
}

/// Print all diagnostics in the sink to stderr in a human-readable format.
pub(crate) fn print_diagnostics(path: &str, sink: &DiagnosticSink) {
    print_diagnostics_multi(&[(path.to_string(), 0)], sink);
}

/// Multi-file variant: each label names its owning file (see
/// `read_sources`/`owner_file`). Single-file units delegate with base 0.
pub(crate) fn print_diagnostics_multi(files: &[(String, usize)], sink: &DiagnosticSink) {
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
            let owner = owner_file(files, label.span.start);
            eprintln!("  --> {owner}:{}:{}", label.span.start, label.span.end);
            if !label.message.is_empty() {
                eprintln!("  | {}", label.message);
            }
        }
        for note in &diag.notes {
            eprintln!("  = note: {note}");
        }
    }
}
