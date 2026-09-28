//! `noct run` — build and execute a `.nv` program via the tree-walking interpreter.
//!
//! [Phase 1] Pipeline: lex → parse → resolve → typecheck → interpret.
//!
//! Usage:
//!   noct run [files...] [--offline] [--frozen] [--index <dir>]
//!   noct run                       # runs main.nv in the current package
//!   noct run lib/main.nv           # loads local imports automatically
//!
//! The package gate (lock + store) runs first, and can be narrowed with
//! `--frozen` (metadata-only) or `--offline` (no fetching); `--index`
//! lets a missing locked dependency be fetched on the spot. See
//! `registry::Gate`.

use std::path::Path;

use compiler::diagnostics::DiagnosticSink;

/// Read roots and their local import graph into one source-backed diagnostic
/// unit. The compiler module loader still parses each file independently;
/// joining here preserves the existing command-line diagnostic format.
///
/// The global content store is handed to the resolver as an extra
/// package root, so `import <pkg>::…` finds `trees/<sha256>/` through
/// the store's by-name pointers. `vendor/` still comes first (the
/// committed escape hatch wins over fetched content), and the legacy
/// in-tree `.noct/packages/` is no longer searched at all — in-tree
/// holds only build outputs now.
pub(crate) fn read_module_graph(paths: &[&str]) -> Result<compiler::modules::ModuleGraph, String> {
    let roots = paths.iter().map(|p| Path::new(p).to_path_buf()).collect::<Vec<_>>();
    let mut package_roots = vec![crate::store::Store::open().resolution_root()];
    package_roots.extend(path_dependency_roots()?);
    compiler::modules::ModuleGraph::load_with(&roots, &package_roots)
}

/// Directories of the project's `path:` dependencies, for the resolver.
///
/// The module resolver never reads `nestpkg.nvpm` — it searches sibling
/// trees, `vendor/`, and the package roots it is handed. That left a
/// manifest dependency of `path: ../../libs/ui` unresolvable, so a
/// project with a first-party dependency failed at `import` with
/// `E0101 cannot resolve imported module 'ui'` and a cascade of
/// unknown-identifier errors behind it — a message that points at the
/// import rather than at the manifest line that caused it. Resolving
/// path deps here, at the toolchain seam, keeps manifest parsing out of
/// the compiler crate (which has no manifest parser) and reuses the
/// same `package_roots` channel the store already uses.
///
/// A declared path dependency whose directory does not exist is a loud
/// error naming the dependency and the path it claimed. Skipping it
/// would reproduce the confusing `E0101` this exists to prevent.
fn path_dependency_roots() -> Result<Vec<std::path::PathBuf>, String> {
    let manifest_path = Path::new("nestpkg.nvpm");
    let Ok(text) = std::fs::read_to_string(manifest_path) else {
        // No manifest: single-file use, no package context.
        return Ok(Vec::new());
    };
    let manifest = crate::manifest::parse_manifest(&text)
        .map_err(|e| format!("invalid manifest: {e}"))?;
    let base = std::path::absolute(manifest_path)
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for dep in manifest.dependencies.iter().chain(&manifest.dev_dependencies) {
        let crate::manifest::Source::Path(rel) = &dep.source else {
            continue;
        };
        let dir = base.join(rel);
        if !dir.is_dir() {
            return Err(format!(
                "dependency `{}` declares path `{rel}`, but `{}` is not a directory",
                dep.name,
                dir.display()
            ));
        }
        // Hand over the CONTAINING directory, not the package directory:
        // the resolver's `package_roots` are roots that hold packages
        // (the store, a `vendor/` tree), and it finds `<root>/<pkg>/lib/…`
        // by listing them. Passing `libs/ui` would make it look for
        // `libs/ui/ui/…`. Sibling path deps share a parent, so dedupe.
        if let Some(parent) = dir.parent() {
            let parent = parent.to_path_buf();
            if !roots.contains(&parent) {
                roots.push(parent);
            }
        }
    }
    Ok(roots)
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
    // Dependency flags are parsed (and stripped) before anything else,
    // so the file list below stays a file list.
    let mut gate = crate::registry::Gate::default();
    let mut files: Vec<&str> = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--offline" => gate.offline = true,
            "--frozen" | "--locked" => gate.frozen = true,
            "--index" => match it.next() {
                Some(v) => gate.index = Some(Path::new(v).to_path_buf()),
                None => {
                    eprintln!("noct run: --index needs a directory");
                    return 1;
                }
            },
            "-h" | "--help" => {
                eprintln!("usage: noct run [files...] [--frozen] [--offline] [--index <dir>]");
                eprintln!("  --frozen: metadata-only; the lock is checked, the store is not read");
                eprintln!("  --offline: never fetch; a missing dependency is an error");
                eprintln!("  --index <dir>: fetch a locked dependency missing from the store");
                return 0;
            }
            // An unrecognised flag is an error, never a file name: this
            // used to be filtered out silently, which turned a typo into
            // "the wrong program ran".
            other if other.starts_with('-') => {
                eprintln!("noct run: unknown flag `{other}`");
                return 1;
            }
            other => files.push(other),
        }
    }
    // Same package gate as `build` (P-003 §6); silent without a manifest.
    if let Err(message) = crate::registry::require_package_current(Path::new("."), &gate) {
        eprintln!("noct run: {message}");
        return 1;
    }
    // 0. Auto-load `./*.nv.env` (ADR-017): sorted, process wins,
    // malformed lines warn. Runs before everything so `config_or` /
    // `env_get` see file values during interpretation.
    autoload_dotenv_files();
    // 1. Resolve file paths (all non-flag args, or "main.nv" in cwd)
    let paths = if files.is_empty() {
        vec!["main.nv"]
    } else {
        files
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
    //
    // On a scoped thread with a large stack, NOT the main thread. The main
    // thread's stack is 1 MiB on Windows (8 MiB for a spawned thread), and
    // the tree-walking interpreter's per-frame cost meant a recursion of
    // ~15 ordinary calls already overflowed it. That is shallow enough to
    // make any recursive algorithm — tree walking, rendering, diffing —
    // unusable, and it surfaced as a hard `thread 'main' has overflowed
    // its stack` crash with no diagnostic. Same reasoning, and the same
    // shape, as the interpreter's HTTP worker threads.
    let ran = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn_scoped(scope, || {
                let mut interp = interp::Interpreter::new();
                let mut sink = DiagnosticSink::new();
                let code = interp.run(&module, &mut sink);
                (code, sink.take())
            })
            .map(|handle| handle.join())
    });

    let exit_code = match ran {
        Ok(Ok((code, diags))) => {
            if !diags.is_empty() {
                let mut sink = DiagnosticSink::new();
                for diag in diags {
                    sink.emit(diag);
                }
                print_diagnostics_multi(&files, &sink);
            }
            code
        }
        // The interpreter thread panicked: a user-level bug (a stack
        // overflow, an explicit panic) must still exit with a message
        // rather than taking the whole CLI down with it.
        Ok(Err(_)) => {
            eprintln!("noct run: the interpreter thread panicked");
            1
        }
        Err(e) => {
            eprintln!("noct run: cannot start the interpreter thread: {e}");
            1
        }
    };

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
