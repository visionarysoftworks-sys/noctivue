//! Stdlib foundation tests — Task: Phase 4 stdlib track.
//!
//! Covers `stdlib/` per `stdlib/SPEC.md` §7 (gates 1–2):
//!   1. Every foundation file lexes/parses/resolves/typechecks with zero
//!      error diagnostics.
//!   2. Every reserved file parses with zero items (header comments only)
//!      and zero error diagnostics — locks the `reserved` state.
//!   3. Both smoke mains run concatenated with the foundation (SPEC.md
//!      §5.1 order) and exit 0. The smokes are self-verifying: every
//!      expectation is an `assert_*`, so exit 0 proves all values.
//!
//! Golden transcripts (`tests/golden/stdlib/*.stdout`) are compared via
//! the CLI (`noct run <foundation files> <smoke>`) per SPEC.md §7 — the
//! interpreter writes directly to process stdout, so byte comparison
//! lives outside this in-process harness. The differential gate
//! (`run` vs `run-vm`) is recorded blocked in SPEC.md §10: the VM
//! rejects `to_int`/`to_float` builtins, fn-refs-as-values, and
//! multi-file units independent of stdlib code.

use compiler::diagnostics::DiagnosticSink;

// ── File lists (SPEC.md §5.1 order + §9 reserved set) ────────────────────────

/// Runnable foundation modules, in canonical concatenation order.
const FOUNDATION: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/math/constants.nv",
    "stdlib/math/basic.nv",
    "stdlib/collections/list.nv",
    "stdlib/collections/iterator.nv",
    "stdlib/time/duration.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/concurrency/task.nv",
];

/// Reserved (header-only) files. Each must parse to zero items.
const RESERVED: &[&str] = &[
    "stdlib/core/prelude.nv",
    "stdlib/core/types.nv",
    "stdlib/collections/map.nv",
    "stdlib/collections/set.nv",
    "stdlib/collections/stack.nv",
    "stdlib/collections/queue.nv",
    "stdlib/math/statistics.nv",
    "stdlib/math/trigonometry.nv",
    "stdlib/strings/builder.nv",
    "stdlib/strings/unicode.nv",
    "stdlib/testing/test.nv",
    "stdlib/testing/property.nv",
    "stdlib/testing/mock.nv",
];

/// (Smoke main, display name) pairs.
const SMOKES: &[(&str, &str)] = &[
    ("tests/fixtures/stdlib/smoke_core.nv", "smoke_core"),
    (
        "tests/fixtures/stdlib/smoke_lists_strings.nv",
        "smoke_lists_strings",
    ),
];

/// Phase-5 path/file/io/error/env surface (all runnable, no syscalls
/// beyond the local filesystem).
const M4_FILES: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/fs/path.nv",
    "stdlib/fs/file.nv",
    "stdlib/io/stdout.nv",
    "stdlib/error/types.nv",
    "stdlib/env/variables.nv",
    "stdlib/env/config.nv",
    "stdlib/env/dotenv.nv",
];

/// Phase-5 JSON metadata surface.
const JSON_FILES: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/encoding/json/value.nv",
    "stdlib/encoding/json/parser.nv",
    "stdlib/encoding/json/writer.nv",
];

/// Phase-5 process + file-watch surface.
const PROCESS_FILES: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/collections/list.nv",
    "stdlib/process/command.nv",
    "stdlib/process/process.nv",
    "stdlib/fs/metadata.nv",
    "stdlib/fs/file.nv",
    "stdlib/fs/watch.nv",
];

/// Phase-5 log + SQLite surface (includes the dotenv/config/env
/// helpers the log tests lean on).
const LOG_DB_FILES: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/collections/list.nv",
    "stdlib/env/variables.nv",
    "stdlib/env/config.nv",
    "stdlib/env/dotenv.nv",
    "stdlib/log/log.nv",
    "stdlib/db/sqlite.nv",
    "stdlib/db/pool.nv",
];

/// Unique scratch path in the OS temp dir (forward slashes: the `.nv`
/// lexer sees the path inside a double-quoted string).
fn tmp(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("noctivue-stdlib-{}-{}", std::process::id(), name))
        .to_string_lossy()
        .replace('\\', "/")
}

/// Run a checked module; fail the test on nonzero exit or error diags.
fn run_ok(module: &compiler::hir::Module, what: &str) {
    let mut sink = DiagnosticSink::new();
    let mut interp = interp::Interpreter::new();
    let exit = interp.run(module, &mut sink);
    let errors = error_texts(&sink);
    assert!(
        exit == 0,
        "[{what}] interpreter exit code {exit} (expected 0):\n  {}",
        errors.join("\n  ")
    );
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

/// Join several sources like `noct run` does (libraries first, entry last).
fn join(sources: &[String]) -> String {
    let mut out = String::new();
    for s in sources {
        out.push_str(s);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn error_texts(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let code = d.code.as_deref().unwrap_or("?");
            format!("[{code}] {}", d.message)
        })
        .collect()
}

/// Full pipeline through typecheck. Returns the HIR module; panics on error.
fn check(source: &str, what: &str) -> compiler::hir::Module {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let module = compiler::typeck::typecheck(program, &mut sink);
    let errors = error_texts(&sink);
    assert!(
        errors.is_empty(),
        "{what} produced error diagnostics:\n  {}",
        errors.join("\n  ")
    );
    module
}

// ── Gate 1: foundation files are error-free ─────────────────────────────────

#[test]
fn stdlib_foundation_zero_errors() {
    for path in FOUNDATION {
        let source = read(path);
        let mut sink = DiagnosticSink::new();
        let tokens = compiler::lexer::lex(&source, &mut sink);
        assert!(
            !tokens.is_empty(),
            "{path} lexed to zero tokens (reserved files belong in RESERVED)"
        );
        let program = compiler::parser::parse(&tokens, &mut sink);
        assert!(
            !program.items.is_empty(),
            "{path} parsed to zero items (reserved files belong in RESERVED)"
        );
        let program = compiler::resolver::resolve(program, &mut sink);
        let _ = compiler::typeck::typecheck(program, &mut sink);
        let errors = error_texts(&sink);
        assert!(
            errors.is_empty(),
            "{path} produced error diagnostics:\n  {}",
            errors.join("\n  ")
        );
    }
}

// ── Gate 2: reserved files stay header-only ─────────────────────────────────

#[test]
fn stdlib_reserved_zero_items_zero_errors() {
    for path in RESERVED {
        let source = read(path);
        let mut sink = DiagnosticSink::new();
        let tokens = compiler::lexer::lex(&source, &mut sink);
        let program = compiler::parser::parse(&tokens, &mut sink);
        assert!(
            program.items.is_empty(),
            "{path} must stay header-only (zero items); found {} item(s) — \
             promote it to a runnable module with a SPEC.md amendment instead",
            program.items.len()
        );
        let errors = error_texts(&sink);
        assert!(
            errors.is_empty(),
            "{path} produced error diagnostics:\n  {}",
            errors.join("\n  ")
        );
    }
}

// ── Gate 3: smoke mains exit 0 over the concatenated foundation ─────────────

#[test]
fn stdlib_smoke_exit_zero() {
    let libs: Vec<String> = FOUNDATION.iter().map(|p| read(p)).collect();
    for (path, name) in SMOKES {
        let mut sources = libs.clone();
        sources.push(read(path));
        let module = check(&join(&sources), name);
        run_ok(&module, name);
    }
}

// ── Phase-5 surface tests ───────────────────────────────────────────────────
// Each concatenates its file list with a self-verifying `main`
// (every expectation is an `assert_*`; exit 0 proves all values).

#[test]
fn stdlib_m4_path_and_file_surface_runs() {
    let path = tmp("m4.txt");
    let _ = std::fs::remove_file(&path);
    let libs: Vec<String> = M4_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"fn must_write(path: String, contents: String) -> Unit:
    match file_write_text(path, contents):
        Ok(_):
            assert(true, "")
        Err(e):
            assert(false, "write failed")
fn must_read(path: String) -> String:
    var out = ""
    match file_read_text(path):
        Ok(text):
            out = text
        Err(e):
            assert(false, "read failed")
    out
fn main():
    assert_string_eq(path_join("a", "b"), "a/b")
    assert_string_eq(path_join("a/", "b"), "a/b")
    assert_string_eq(path_basename("a/b/c.nv"), "c.nv")
    assert_string_eq(path_dirname("a/b/c.nv"), "a/b")
    assert_string_eq(path_dirname("c.nv"), ".")
    assert_string_eq(path_extension("c.nv"), ".nv")
    assert_true(path_is_absolute("/x"), "rooted")
    assert_true(path_is_absolute("C:/x"), "drive")
    assert_true(path_is_absolute("rel") == false, "relative")
    assert_true(file_exists("{path}") == false, "missing at start")
    must_write("{path}", "hello")
    assert_true(file_exists("{path}"), "written file exists")
    assert_string_eq(must_read("{path}"), "hello")
    let info = error_info("E0", "oops", "fs")
    assert_string_eq(error_info_display(info), "fs: E0: oops")
    let bare = error_info("E1", "bad", "")
    assert_string_eq(error_info_display(bare), "E1: bad")
    let with_ctx = error_info_context(info, "while testing")
    assert_string_eq(with_ctx.code, "E0")
    stdout_write("m4-ok")
"#,
        path = path
    ));
    let module = check(&join(&sources), "m4_path_and_file");
    run_ok(&module, "m4_path_and_file");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn stdlib_m4_errors_and_config_surface_runs() {
    let kv_path = tmp("config.kv");
    let env_path = tmp("cfg.nv.env");
    let _ = std::fs::remove_file(&kv_path);
    let _ = std::fs::remove_file(&env_path);
    std::fs::write(&kv_path, "# comment\nANSWER=42\n").unwrap();
    std::fs::write(&env_path, "T2_LOADED=yes\n").unwrap();
    let libs: Vec<String> = M4_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"fn main():
    assert_string_eq(config_or("NOCTIVUE_TEST_ABSENT_XYZ", "dflt"), "dflt")
    match config_require("NOCTIVUE_TEST_ABSENT_XYZ"):
        Ok(_):
            assert(false, "missing require passed")
        Err(e):
            assert_true(string_contains(e, "NOCTIVUE_TEST_ABSENT_XYZ"), "names the variable")
    match config_file("{kv}", "ANSWER"):
        Ok(v):
            assert_string_eq(v, "42")
        Err(e):
            assert(false, "config file read failed")
    match config_file("{kv}", "MISSING"):
        Ok(_):
            assert(false, "missing key passed")
        Err(_):
            assert(true, "")
    match config_env("NOCTIVUE_TEST_ABSENT_XYZ"):
        Some(_):
            assert(false, "absent env present")
        None:
            assert(true, "")
    match dotenv_load("{env}"):
        Ok(n):
            assert_int_eq(n, 1)
        Err(e):
            assert(false, "dotenv load failed")
    assert_string_eq(config_or("T2_LOADED", "no"), "yes")
"#,
        kv = kv_path,
        env = env_path
    ));
    let module = check(&join(&sources), "m4_errors_and_config");
    run_ok(&module, "m4_errors_and_config");
    std::env::remove_var("T2_LOADED");
    let _ = std::fs::remove_file(&kv_path);
    let _ = std::fs::remove_file(&env_path);
}

#[test]
fn stdlib_json_metadata_round_trip_runs() {
    let libs: Vec<String> = JSON_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(
        r#"fn main():
    let obj = json_object_string_pair("name", "Ada", "lang", "nv")
    match json_validate(obj):
        Ok(_):
            assert(true, "")
        Err(e):
            assert(false, "valid object rejected")
    match json_validate("nope \{"):
        Ok(_):
            assert(false, "invalid json passed")
        Err(_):
            assert(true, "")
    match json_parse(obj):
        Ok(_):
            assert(true, "")
        Err(e):
            assert(false, "parse failed")
    match json_parse_string_field(obj, "name"):
        Ok(v):
            assert_string_eq(v, "Ada")
        Err(e):
            assert(false, "field read failed")
    match json_get_string(obj, "lang"):
        Ok(v):
            assert_string_eq(v, "nv")
        Err(e):
            assert(false, "get_string failed")
"#
        .to_string(),
    );
    let module = check(&join(&sources), "json_metadata");
    run_ok(&module, "json_metadata");
}

#[test]
fn stdlib_process_and_watch_surface_runs() {
    let watch_path = tmp("watch.txt");
    let _ = std::fs::remove_file(&watch_path);
    std::fs::write(&watch_path, "v1").unwrap();
    let (program, args) = if cfg!(windows) {
        ("cmd".to_string(), r#"["/c", "echo", "hi"]"#.to_string())
    } else {
        ("echo".to_string(), r#"["hi"]"#.to_string())
    };
    let libs: Vec<String> = PROCESS_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"fn main():
    match process_run("{program}", {args}):
        Ok(out):
            assert_true(process_succeeded(out), "echo succeeds")
            assert_true(string_contains(process_stdout(out), "hi"), "echo output")
        Err(e):
            assert(false, "process run failed")
    match process_run("noctivue-test-absent-binary-xyz", []):
        Ok(_):
            assert(false, "absent binary passed")
        Err(_):
            assert(true, "")
    match watch_snapshot("{watch}"):
        Ok(first):
            sleep_builtin(15)
            match file_write_text("{watch}", "v2"):
                Ok(_):
                    assert(true, "")
                Err(e):
                    assert(false, "rewrite failed")
            match watch_changed("{watch}", first):
                Ok(changed):
                    assert_true(changed, "rewrite detected")
                Err(e):
                    assert(false, "changed check failed")
            match watch_snapshot("{watch}"):
                Ok(second):
                    match watch_changed("{watch}", second):
                        Ok(changed):
                            assert_true(changed == false, "no spurious change")
                        Err(e):
                            assert(false, "recheck failed")
                Err(e):
                    assert(false, "resnapshot failed")
        Err(e):
            assert(false, "snapshot failed")
"#,
        program = program,
        args = args,
        watch = watch_path
    ));
    let module = check(&join(&sources), "process_and_watch");
    run_ok(&module, "process_and_watch");
    let _ = std::fs::remove_file(&watch_path);
}

#[test]
fn stdlib_log_db_surface_runs() {
    let db_path = tmp("t5.db");
    let _ = std::fs::remove_file(&db_path);
    let libs: Vec<String> = LOG_DB_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"fn seed_users(db: Db) -> Unit:
    match db_exec(db, "CREATE TABLE users(name TEXT, age INT)", "[]"):
        Ok(_):
            assert(true, "")
        Err(e):
            assert(false, "create failed")
    match db_exec(db, "INSERT INTO users(name, age) VALUES ('Alice', 30)", "[]"):
        Ok(n):
            assert_int_eq(n, 1)
        Err(e):
            assert(false, "insert alice failed")
    match db_exec(db, "INSERT INTO users(name, age) VALUES ('Bob', 25)", "[]"):
        Ok(n):
            assert_int_eq(n, 1)
        Err(e):
            assert(false, "insert bob failed")
fn assert_user_rows(db: Db) -> Unit:
    match db_query(db, "SELECT name, age FROM users ORDER BY name", "[]"):
        Ok(rows):
            assert_int_eq(list_len_string(rows), 2)
            match list_first_string(rows):
                Some(first):
                    assert_true(string_contains(first, "Alice"), "alice first")
                None:
                    assert(false, "no rows")
        Err(e):
            assert(false, "query failed")
fn assert_pool(path: String) -> Unit:
    match pool_open(path, 0):
        Ok(_):
            assert(false, "zero pool opened")
        Err(_):
            assert(true, "")
    match pool_open(path, 2):
        Ok(pool):
            match pool_checkout(pool):
                Ok(co):
                    match db_exec(co.db, "INSERT INTO users(name, age) VALUES ('Cara', 41)", "[]"):
                        Ok(_):
                            assert(true, "")
                        Err(e):
                            assert(false, "pool insert failed")
                    let pool2 = pool_checkin(co)
                    match pool_close(pool2):
                        Ok(_):
                            assert(true, "")
                        Err(e):
                            assert(false, "pool close failed")
                Err(e):
                    assert(false, "checkout failed")
        Err(e):
            assert(false, "pool open failed")
fn main():
    log_info("db surface running")
    log_debug("debug line")
    log_warn("warn line")
    match db_open("{db}"):
        Ok(db):
            seed_users(db)
            assert_user_rows(db)
            match db_close(db):
                Ok(_):
                    assert(true, "")
                Err(e):
                    assert(false, "close failed")
        Err(e):
            assert(false, "open failed")
    assert_pool("{db}")
"#,
        db = db_path
    ));
    let module = check(&join(&sources), "log_db");
    run_ok(&module, "log_db");
    let _ = std::fs::remove_file(&db_path);
}
