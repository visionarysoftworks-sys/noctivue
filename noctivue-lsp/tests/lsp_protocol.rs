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
