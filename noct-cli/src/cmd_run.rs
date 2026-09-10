//! `noct run` — build and execute a `.nv` program via the tree-walking interpreter.
//!
//! [Phase 1] Pipeline: lex → parse → resolve → typecheck → interpret.
//!
//! Usage:
//!   noct run [files...]
//!   noct run                       # runs main.nv in the current package
//!   noct run lib/main.nv           # loads local imports automatically

use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

/// Read roots and their local import graph into one source-backed diagnostic
/// unit. The compiler module loader still parses each file independently;
/// joining here preserves the existing command-line diagnostic format.
pub(crate) fn read_module_graph(paths: &[&str]) -> Result<compiler::modules::ModuleGraph, String> {
    let roots = paths.iter().map(|p| Path::new(p).to_path_buf()).collect::<Vec<_>>();
    compiler::modules::ModuleGraph::load(&roots)
}

/// Name the file owning a whole-unit byte offset (see `read_module_graph`).
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
    // Same package gate as `build` (P-003 §6); silent without a manifest.
    if let Err(message) = crate::registry::require_package_current(Path::new(".")) {
        eprintln!("noct run: {message}");
        return 1;
    }
    // 0. Auto-load `./*.nv.env` (ADR-017): sorted, process wins,
    // malformed lines warn. Runs before everything so `config_or` /
    // `env_get` see file values during interpretation.
    autoload_dotenv_files();
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
    let graph = match read_module_graph(&paths) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let (source, files) = graph.joined_source();

    // 3. Create diagnostic sink
    let mut sink = DiagnosticSink::new();

    // 4. Lex
    let tokens = compiler::lexer::lex(&source, &mut sink);

    // 5. Parse
    let program = compiler::parser::parse(&tokens, &mut sink);
    graph.emit_diagnostics(&mut sink);
    let program = graph.link(program);

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

/// Auto-load every `./*.nv.env` file (ADR-017) into the process
/// environment before running. Files are loaded in sorted order;
/// the process environment always wins; malformed lines warn on
/// stderr (via the shared `interp::dotenv_load_file` core) and are
/// skipped. A missing/unreadable directory simply loads nothing.
fn autoload_dotenv_files() {
    let mut names: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".nv.env") {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    for name in names {
        let _ = interp::dotenv_load_file(&name, |m| eprintln!("{m}"));
    }
}

/// Print all diagnostics in the sink to stderr in a human-readable format.
pub(crate) fn print_diagnostics(path: &str, sink: &DiagnosticSink) {
    print_diagnostics_multi(&[(path.to_string(), 0)], sink);
}

/// Multi-file variant: each label names its owning file (see
/// `read_module_graph`/`owner_file`). Single-file units delegate with base 0.
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
