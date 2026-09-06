//! `noct fmt` — run the official Noctivue formatter (v1).
//!
//! [Phase 4 / M3] Scope is P-004 v1: a trivia canonicalizer doing
//! byte-level line ops only (strip trailing whitespace, collapse blank
//! runs, exactly-one trailing newline, LF endings). String-aware:
//! `"""…"""` spans are opaque, `//` comments and `"…"`/`'…'` literals
//! (with escapes) are skipped correctly so comment text resembling a
//! span opener never corrupts values.
//!
//! Canonical decisions D1–D6 live in `stdlib/PROPOSALS.md` P-004. v1
//! enforces D1 (tabs in leading whitespace are a loud error, never a
//! guess) and D2 (blank/ending rules). D3–D5 (semicolons, expanded
//! density, 100-col cap) need the v2 AST printer, gated on comment
//! attachment — v1 never moves, adds, or removes code tokens, so it
//! cannot violate them. D6 (concise/explicit equivalence) is untouched
//! by construction.
//!
//! In-command safety (P-004): the output is re-pipelined through the
//! formatter (idempotency — `fmt(fmt(x)) != fmt(x)` is a formatter bug,
//! loud, never persisted) and through lex+parse (no-new-errors — the
//! output must not contain MORE error diagnostics than the input).
//! Violations refuse to write and exit 1.
//!
//! Usage:
//!   noct fmt [file.nv ... | --check file.nv ...]
//!   noct fmt --v2 [file.nv ... | --check file.nv ...]
//!   noct fmt < stdin.nv            (no file args: filter stdin→stdout)
//!
//! `--v2` selects the AST printer (`fmt_v2`: expanded canonical form,
//! comment attachment, density heuristic) instead of the default v1
//! trivia canonicalizer. v2 carries its own gates (AST-equivalence,
//! idempotency, no-new-errors) on top of the checks below.

use std::fs;
use std::path::Path;

/// Loud refusal to format (tab indent, guard trip, I/O failure).
struct FmtRefusal {
    message: String,
}

pub fn run(args: &[String]) -> i32 {
    let mut check_mode = false;
    let mut v2_mode = false;
    let mut files: Vec<&str> = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--check" => check_mode = true,
            "--v2" => v2_mode = true,
            other => {
                if other.starts_with('-') {
                    eprintln!("noct fmt: unknown flag `{other}`");
                    return 1;
                }
                files.push(other);
            }
        }
    }

    if files.is_empty() {
        return format_stdin(v2_mode);
    }

    let mut exit_code = 0;
    for file in files {
        let path = Path::new(file);
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("noct fmt: {file}: cannot read: {e}");
                exit_code = 1;
                continue;
            }
        };

        let formatted = match format_any(&content, v2_mode) {
            Ok(f) => f,
            Err(refusal) => {
                eprintln!("noct fmt: {file}: {}", refusal.message);
                exit_code = 1;
                continue;
            }
        };

        if check_mode {
            if content != formatted {
                println!("would reformat {file}");
                exit_code = 1;
            }
            continue;
        }

        if content == formatted {
            continue;
        }

        // Guards before any write: v1 re-checks here (idempotency +
        // no-new-errors); v2 already ran its own (stronger) gates
        // inside `format_v2`, so it skips the v1 re-check.
        if !v2_mode {
            match guard_format(&content, &formatted) {
                Ok(()) => {}
                Err(refusal) => {
                    eprintln!("noct fmt: {file}: {}", refusal.message);
                    exit_code = 1;
                    continue;
                }
            }
        }
        match fs::write(path, &formatted) {
            Ok(()) => println!("Formatted {file}"),
            Err(e) => {
                eprintln!("noct fmt: {file}: cannot write: {e}");
                exit_code = 1;
            }
        }
    }
    exit_code
}

fn format_stdin(v2_mode: bool) -> i32 {
    use std::io::{self, Read};
    let mut buffer = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut buffer) {
        eprintln!("noct fmt: cannot read stdin: {e}");
        return 1;
    }
    let formatted = match format_any(&buffer, v2_mode) {
        Ok(f) => f,
        Err(refusal) => {
            eprintln!("noct fmt: stdin: {}", refusal.message);
            return 1;
        }
    };
    if !v2_mode {
        if let Err(refusal) = guard_format(&buffer, &formatted) {
            eprintln!("noct fmt: stdin: {}", refusal.message);
            return 1;
        }
    }
    print!("{formatted}");
    0
}

/// Dispatch between the v1 trivia canonicalizer (default) and the
/// v2 AST printer (`--v2`, which runs its own gates internally).
fn format_any(source: &str, v2_mode: bool) -> Result<String, FmtRefusal> {
    if v2_mode {
        return crate::fmt_v2::format_v2(source).map_err(|e| FmtRefusal {
            message: e.message,
        });
    }
    format_source(source)
}
fn guard_format(original: &str, formatted: &str) -> Result<(), FmtRefusal> {
    let twice = format_source(formatted).map_err(|refusal| FmtRefusal {
        message: format!(
            "formatter bug (reformat refused: {}); output not written",
            refusal.message
        ),
    })?;
    if twice != formatted {
        return Err(FmtRefusal {
            message: "formatter bug (output is not idempotent); output not written".to_string(),
        });
    }
    let before = error_count(original);
    let after = error_count(formatted);
    if after > before {
        return Err(FmtRefusal {
            message: format!(
                "refusing to write: formatted output has more errors ({after}) than input ({before})"
            ),
        });
    }
    Ok(())
}

/// Number of error-severity diagnostics from lex+parse (resolver and
/// typeck intentionally excluded — v1 must not depend on name
/// resolution to judge trivia safety).
fn error_count(source: &str) -> usize {
    let mut sink = compiler::diagnostics::DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let _ = compiler::parser::parse(&tokens, &mut sink);
    sink.diagnostics().iter().filter(|d| d.is_error()).count()
}

/// Canonicalize trivia. Leading whitespace (indentation) is NEVER
/// touched — the lexer judges it, not v1. Tabs there are a loud error
/// (D1). Returns the canonical text (always ends in exactly one `\n`;
/// empty/whitespace-only input becomes `"\n"`).
fn format_source(source: &str) -> Result<String, FmtRefusal> {
    // D2: LF endings — `\r` dies here, never survives into a line.
    let normalized = source.replace("\r\n", "\n").replace('\r', "");
    let mut out: Vec<String> = Vec::new();
    let mut in_triple = false;
    let mut blanks = 0usize;

    for (index, line) in normalized.split('\n').enumerate() {
        let line_no = index + 1;
        if in_triple {
            // Whole line is span content (opener-rest, body, and closer
            // lines are all opaque). An odd `"""` count toggles state;
            // even (open+close on one line) leaves it unchanged.
            out.push(line.to_string());
            blanks = 0;
            if line.matches("\"\"\"").count() % 2 == 1 {
                in_triple = false;
            }
            continue;
        }
        match scan_code_line(line) {
            LineKind::Blank => {
                blanks += 1;
            }
            LineKind::Code {
                stripped,
                opens_triple,
            } => {
                if let Some(indent_error) = check_leading_tabs(line, line_no) {
                    return Err(indent_error);
                }
                if blanks > 0 {
                    // Collapse runs; leading blanks die with the run
                    // (nothing pushed yet means file start).
                    if !out.is_empty() {
                        out.push(String::new());
                    }
                    blanks = 0;
                }
                out.push(stripped);
                if opens_triple {
                    in_triple = true;
                }
            }
        }
    }

    if out.is_empty() {
        return Ok("\n".to_string());
    }
    let mut result = out.join("\n");
    result.push('\n');
    Ok(result)
}

enum LineKind {
    Blank,
    Code {
        stripped: String,
        opens_triple: bool,
    },
}

/// Classify one physical line outside a `"""` span. Returns the line
/// with trailing whitespace removed plus whether a span opens on it
/// (in which case the whole line is kept byte-identical — the opener
/// line's bytes are values from the `"""` onward, and the code before
/// it is left alone by the same conservative call).
fn scan_code_line(line: &str) -> LineKind {
    if line.trim().is_empty() {
        return LineKind::Blank;
    }
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                // `//` outside a literal: rest is comment. A `"""`
                // in here is comment text, never a span opener.
                break;
            }
            b'"' => {
                if line[i..].starts_with("\"\"\"") {
                    return LineKind::Code {
                        stripped: line.to_string(),
                        opens_triple: true,
                    };
                }
                i = skip_quoted(line, i, b'"');
                continue;
            }
            b'\'' => {
                i = skip_quoted(line, i, b'\'');
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    LineKind::Code {
        stripped: strip_trailing_ws(line),
        opens_triple: false,
    }
}

/// Byte index just past the closing quote of the literal opening at
/// `open` (or end of line when unterminated — a lex error the
/// no-new-errors guard keeps honest; the strip still applies).
fn skip_quoted(line: &str, open: usize, quote: u8) -> usize {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

fn strip_trailing_ws(line: &str) -> String {
    line.trim_end_matches([' ', '\t']).to_string()
}

/// D1: tabs in leading whitespace of a content line are a loud error.
/// Blank lines are exempt (their whitespace is stripped, never kept);
/// span-content lines never reach here.
fn check_leading_tabs(line: &str, line_no: usize) -> Option<FmtRefusal> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    if line[..indent_len].contains('\t') {
        Some(FmtRefusal {
            message: format!("line {line_no}: tab in leading whitespace (use 4 spaces per level)"),
        })
    } else {
        None
    }
}
