//! Protocol smoke for `noctivue-lsp`: initialize → didOpen →
//! hover over stdio, asserting the §6.1 hover card (signature +
//! documentation + Declared-in) comes back. Catches server-wiring
//! regressions that unit tests on `analysis` cannot (framing,
//! dispatch, document tracking).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

fn lsp_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noctivue-lsp"))
}

struct Conn {
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
}

impl Conn {
    fn send(&mut self, body: &str) {
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)
            .expect("write to lsp stdin");
        self.stdin.flush().expect("flush lsp stdin");
    }

    fn recv(&mut self) -> serde_json::Value {
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("read header");
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().expect("parse length");
            }
        }
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).expect("read body");
        serde_json::from_slice(&buf).expect("parse message")
    }

    /// Skip server→client notifications until the response with `id`.
    fn recv_response(&mut self, id: u64) -> serde_json::Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for response {id}"
            );
            let msg = self.recv();
            if msg.get("id") == Some(&serde_json::json!(id)) {
                return msg;
            }
        }
    }
}

const SRC: &str = "/// Adds two numbers.\nfn add(a: Int, b: Int) -> Int:\n    a + b\n\nmain():\n    println(add(1, 2))\n";
const URI: &str = "file:///c%3A/tmp/smoke.nv";

#[test]
fn hover_over_stdio_returns_section_ordered_card() {
    let mut child = Command::new(lsp_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn noctivue-lsp");
    let mut conn = Conn {
        stdin: child.stdin.take().expect("stdin"),
        reader: BufReader::new(child.stdout.take().expect("stdout")),
    };

    conn.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{}}}"#);
    let init = conn.recv_response(1);
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    conn.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":URI,"languageId":"noctivue","version":1,"text":SRC}}})
            .to_string(),
    );
    // Hover over `add` at the call site (0-based line 5, char 12).
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":URI},"position":{"line":5,"character":12}}})
            .to_string(),
    );
    let hover = conn.recv_response(2);
    let text = serde_json::to_string(hover.get("result").unwrap_or(&serde_json::Value::Null))
        .expect("serialize result");
    for needle in [
        "fn add(a: Int, b: Int) -> Int",
        "Adds two numbers.",
        "Declared in",
    ] {
        assert!(
            text.contains(needle),
            "hover card missing {needle:?}: {text}"
        );
    }
    let _ = child.kill();
}

/// Diagnostics carry the document version, and later requests observe the
/// latest change (the cached-analysis path), not a stale pipeline result.
#[test]
fn diagnostics_carry_versions_and_hover_sees_latest_change() {
    let (mut child, mut conn) = spawn_server();
    conn.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{}}}"#);
    let init = conn.recv_response(1);
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    fn next_diagnostics(conn: &mut Conn) -> serde_json::Value {
        for _ in 0..20 {
            let msg = conn.recv();
            if msg.get("method") == Some(&serde_json::json!("textDocument/publishDiagnostics")) {
                return msg.get("params").cloned().unwrap_or(serde_json::Value::Null);
            }
        }
        panic!("no publishDiagnostics received");
    }

    conn.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":URI,"languageId":"noctivue","version":1,"text":SRC}}})
            .to_string(),
    );
    let diag1 = next_diagnostics(&mut conn);
    assert_eq!(diag1.get("version"), Some(&serde_json::json!(1)), "open version: {diag1}");

    // Rename `add` → `add2` everywhere (version 2); hover must follow.
    let src2 = SRC.replace("add", "add2");
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":URI,"version":2},"contentChanges":[{"text":src2}]}})
            .to_string(),
    );
    let diag2 = next_diagnostics(&mut conn);
    assert_eq!(diag2.get("version"), Some(&serde_json::json!(2)), "change version: {diag2}");

    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":URI},"position":{"line":5,"character":12}}})
            .to_string(),
    );
    let hover = conn.recv_response(2);
    let text = serde_json::to_string(hover.get("result").unwrap_or(&serde_json::Value::Null))
        .expect("serialize result");
    assert!(text.contains("fn add2(a: Int, b: Int) -> Int"), "stale hover: {text}");
    let _ = child.kill();
}

const NESTPKG_SRC: &str = "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    py_model:\n        version: 1.0.0\n        tier: foreign-runtime\n        opt_in: true\n";
const NESTPKG_URI: &str = "file:///c%3A/tmp/nestpkg.nvpm";

fn spawn_server() -> (std::process::Child, Conn) {
    let mut child = Command::new(lsp_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn noctivue-lsp");
    let conn = Conn {
        stdin: child.stdin.take().expect("stdin"),
        reader: BufReader::new(child.stdout.take().expect("stdout")),
    };
    (child, conn)
}

/// Cross-file goto (noctivue-analyzer): a flat `helper()` use from a
/// `path:` dep jumps to the dep's `lib/main.nv`, not to null.
#[test]
fn definition_over_stdio_jumps_to_path_dep() {
    let base = std::env::temp_dir().join(format!(
        "noct-lsp-xfile-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let dep_lib = base.join("libs").join("dep").join("lib");
    std::fs::create_dir_all(&dep_lib).expect("dep lib");
    let app_lib = base.join("app").join("lib");
    std::fs::create_dir_all(&app_lib).expect("app lib");
    std::fs::write(dep_lib.join("main.nv"), "export fn helper() -> Int:\n    7\n").expect("dep");
    std::fs::write(
        base.join("app").join("nestpkg.nvpm"),
        "package:\n    name: app\n    version: 0.1.0\n\ndependencies:\n    dep:\n        version: =0.1.0\n        path: ../libs/dep\n",
    )
    .expect("manifest");
    let main_path = app_lib.join("main.nv");
    let src = "import dep\n\nmain():\n    let n: Int = helper()\n    print(\"{n}\")\n";
    std::fs::write(&main_path, src).expect("main");
    let uri = format!("file:///{}", main_path.to_string_lossy().replace('\\', "/"));

    let (mut child, mut conn) = spawn_server();
    conn.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{}}}"#);
    let init = conn.recv_response(1);
    assert!(init.get("error").is_none(), "initialize failed: {init}");
    // New capabilities must be advertised (TS parity).
    let caps = init.get("result").and_then(|r| r.get("capabilities")).cloned().unwrap_or_default();
    let caps = serde_json::to_string(&caps).unwrap_or_default();
    assert!(caps.contains("typeDefinitionProvider"), "missing typeDefinition: {caps}");
    assert!(caps.contains("implementationProvider"), "missing implementation: {caps}");

    conn.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"noctivue","version":1,"text":src}}})
            .to_string(),
    );
    // `helper` use on line 3 (0-based), inside the call.
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":3,"character":19}}})
            .to_string(),
    );
    let def = conn.recv_response(2);
    let result = def.get("result").cloned().unwrap_or(serde_json::Value::Null);
    let text = serde_json::to_string(&result).unwrap_or_default();
    assert!(
        text.contains("dep") && text.contains("main.nv"),
        "cross-file goto missed dep main: {text}"
    );
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn nestpkg_hover_over_stdio_explains_tier() {
    let (mut child, mut conn) = spawn_server();
    conn.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{}}}"#);
    let init = conn.recv_response(1);
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    conn.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":NESTPKG_URI,"languageId":"nestpkg","version":1,"text":NESTPKG_SRC}}})
            .to_string(),
    );
    // Hover over `foreign-runtime` (line 6, inside the tier value).
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":NESTPKG_URI},"position":{"line":6,"character":16}}})
            .to_string(),
    );
    let hover = conn.recv_response(2);
    let text = serde_json::to_string(hover.get("result").unwrap_or(&serde_json::Value::Null))
        .expect("serialize result");
    assert!(
        text.contains("foreign-runtime"),
        "nestpkg hover missing tier explanation: {text}"
    );
    let _ = child.kill();
}

#[test]
fn nestpkg_completion_over_stdio_suggests_tiers() {
    let (mut child, mut conn) = spawn_server();
    conn.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{}}}"#);
    let init = conn.recv_response(1);
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    conn.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    let src = "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    a:\n        version: 1.0.0\n        tier: \n";
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":NESTPKG_URI,"languageId":"nestpkg","version":1,"text":src}}})
            .to_string(),
    );
    conn.send(
        &serde_json::json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":NESTPKG_URI},"position":{"line":6,"character":14}}})
            .to_string(),
    );
    let completion = conn.recv_response(2);
    let text = serde_json::to_string(completion.get("result").unwrap_or(&serde_json::Value::Null))
        .expect("serialize result");
    assert!(
        text.contains("foreign-runtime"),
        "nestpkg completion missing tier values: {text}"
    );
    let _ = child.kill();
}
