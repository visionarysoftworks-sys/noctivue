//! net.http completion tests — Phase 5/M4.
//!
//! 1. The `net/http` surface lexes/parses/resolves/typechecks with
//!    zero error diagnostics (it did not: tuple-typed headers, bare
//!    enum syntax, and a 3-param `http_send_builtin` signature).
//! 2. Client/server round trip over loopback with the server loop in
//!    a `task`: GET handler output, POST body echo (handlers receive
//!    the body only), 404 for unknown routes, custom headers.
//! 3. Loud refusals: `https://` (no TLS stack) and refused ports
//!    surface as `Err`, never panics.

use compiler::diagnostics::DiagnosticSink;

const HTTP_FILES: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/net/http/request.nv",
    "stdlib/net/http/response.nv",
    "stdlib/net/http/client.nv",
    "stdlib/net/http/server.nv",
];

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

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

/// Allocate a loopback port by binding `:0`, then release it. The
/// test rebinds it immediately (standard TOCTOU, negligible window).
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    listener.local_addr().expect("local addr").port()
}

#[test]
fn http_surface_zero_errors() {
    let sources: Vec<String> = HTTP_FILES.iter().map(|p| read(p)).collect();
    let _ = check(&join(&sources), "http_surface");
}

#[test]
fn http_round_trip_in_task() {
    let port = free_port();
    let libs: Vec<String> = HTTP_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"fn hello_handler(body: String) -> String:
    "hello"

fn echo_handler(body: String) -> String:
    body

task serve(srv: Int):
    http_server_serve(srv)

fn main():
    match http_server_listen({port}):
        Ok(srv):
            http_server_route(srv, "GET", "/hi", "hello_handler")
            http_server_route(srv, "POST", "/echo", "echo_handler")
            let t = serve(srv)
            sleep_builtin(100)
            match http_get("http://127.0.0.1:{port}/hi"):
                Ok(b):
                    assert_string_eq(b, "hello")
                Err(e):
                    assert(false, "GET failed")
            match http_get_with_headers("http://127.0.0.1:{port}/hi", [["X-T", "v"]]):
                Ok(b):
                    assert_string_eq(b, "hello")
                Err(e):
                    assert(false, "headered GET failed")
            match http_post("http://127.0.0.1:{port}/echo", "ping"):
                Ok(b):
                    assert_string_eq(b, "ping")
                Err(e):
                    assert(false, "POST failed")
            match http_get("http://127.0.0.1:{port}/missing"):
                Ok(b):
                    assert_string_eq(b, "Not Found")
                Err(e):
                    assert(false, "404 failed")
            http_server_stop(srv)
            await t
        Err(e):
            assert(false, "listen failed")
"#,
        port = port
    ));
    let module = check(&join(&sources), "http_round_trip");
    run_ok(&module, "http_round_trip");
}

#[test]
fn http_refusals_are_results() {
    let port = free_port();
    let libs: Vec<String> = HTTP_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(format!(
        r#"fn main():
    match http_get("https://127.0.0.1:{port}/"):
        Ok(_):
            assert(false, "https passed")
        Err(e):
            assert_true(string_contains(e, "http://"), "names http")
    match http_get("ftp://127.0.0.1:{port}/"):
        Ok(_):
            assert(false, "ftp passed")
        Err(_):
            assert(true, "")
    match http_get("http://127.0.0.1:{port}/"):
        Ok(_):
            assert(false, "refused port passed")
        Err(e):
            assert_true(string_contains(e, "connection failed"), "names failure")
"#,
        port = port
    ));
    let module = check(&join(&sources), "http_refusals");
    run_ok(&module, "http_refusals");
}

#[test]
fn http_method_vocabulary_runs() {
    let libs: Vec<String> = HTTP_FILES.iter().map(|p| read(p)).collect();
    let mut sources = libs;
    sources.push(
        r#"fn main():
    assert_string_eq(http_method_string(Get), "GET")
    assert_string_eq(http_method_string(Delete), "DELETE")
    match http_method_from_string("POST"):
        Some(name):
            assert_string_eq(name, "Post")
        None:
            assert(false, "POST unknown")
    match http_method_from_string("BREW"):
        Some(_):
            assert(false, "BREW known")
        None:
            assert(true, "")
    let r = http_response(404, "gone")
    assert_int_eq(r.status, 404)
    assert_string_eq(r.reason, "Not Found")
    assert_true(http_is_success(http_response(200, "ok")), "200 success")
    assert_true(http_is_success(r) == false, "404 not success")
"#
        .to_string(),
    );
    let module = check(&join(&sources), "http_vocabulary");
    run_ok(&module, "http_vocabulary");
}
