//! Noctivue Language Server Protocol (LSP) server.
//!
//! This server wraps the Noctivue compiler frontend and provides LSP features:
//! - Diagnostics (textDocument/publishDiagnostics)
//! - Hover (textDocument/hover)
//! - Go to Definition (textDocument/definition)
//! - Completion (textDocument/completion)
//! - DidOpen / DidChange / DidClose notifications

use std::collections::HashMap;

use anyhow::Result;
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::*;
use compiler::analysis::{
    analyze_file, diagnostic_to_lsp, find_definition_at, get_completions, get_hover, span_to_range,
};

/// Bump on every behavior-changing server release so the `window/logMessage`
/// beacon in the client's Output panel identifies the running binary.
const SERVER_VERSION: &str = "0.0.2-hover2";

struct ServerState {
    connection: Connection,
    documents: HashMap<Uri, String>,
    capabilities: ServerCapabilities,
}

impl ServerState {
    fn new(connection: Connection) -> Self {
        let capabilities = ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(false),
                trigger_characters: Some(vec![".".to_string(), ":".to_string(), "(".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        Self {
            connection,
            documents: HashMap::new(),
            capabilities,
        }
    }

    fn run(&mut self) -> Result<()> {
        // Wait for initialize request
        let (_init_params, init_id) = self.wait_for_initialize()?;

        let init_response = InitializeResult {
            capabilities: self.capabilities.clone(),
            server_info: Some(ServerInfo {
                name: "noctivue-lsp".to_string(),
                version: Some(SERVER_VERSION.to_string()),
            }),
            ..Default::default()
        };
        self.connection.sender.send(Message::Response(Response::new_ok(
            init_id,
            serde_json::to_value(init_response)?,
        )))?;

        // Wait for initialized notification
        loop {
            let msg = self.connection.receiver.recv()?;
            if let Message::Notification(notif) = msg {
                if notif.method == "initialized" {
                    break;
                }
            }
        }

        // Version beacon: proves in the client's Output panel exactly which
        // server binary is answering requests.
        let _ = self.connection.sender.send(Message::Notification(Notification::new(
            "window/logMessage".to_string(),
            serde_json::json!({
                "type": 3,
                "message": format!("noctivue-lsp {SERVER_VERSION} ready"),
            }),
        )));

        // Main loop
        loop {
            let msg = self.connection.receiver.recv()?;
            match msg {
                Message::Request(req) => self.handle_request(req)?,
                Message::Notification(notif) => self.handle_notification(notif)?,
                Message::Response(_) => {}
            }
        }
        // unreachable
    }

    fn wait_for_initialize(&mut self) -> Result<(InitializeParams, lsp_server::RequestId)> {
        loop {
            let msg = self.connection.receiver.recv()?;
            match msg {
                Message::Request(req) if req.method == "initialize" => {
                    let params: InitializeParams = serde_json::from_value(req.params)?;
                    return Ok((params, req.id));
                }
                Message::Notification(notif) if notif.method == "exit" => {
                    std::process::exit(0);
                }
                _ => {}
            }
        }
    }

    fn handle_request(&mut self, req: Request) -> Result<()> {
        match req.method.as_str() {
            "textDocument/hover" => self.handle_hover(req),
            "textDocument/definition" => self.handle_definition(req),
            "textDocument/completion" => self.handle_completion(req),
            _ => {
                self.connection.sender.send(Message::Response(Response::new_err(
                    req.id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("Method not found: {}", req.method),
                )))?;
            }
        }
        Ok(())
    }

    fn handle_notification(&mut self, notif: Notification) -> Result<()> {
        match notif.method.as_str() {
            "textDocument/didOpen" => self.handle_did_open(notif),
            "textDocument/didChange" => self.handle_did_change(notif),
            "textDocument/didClose" => self.handle_did_close(notif),
            _ => {}
        }
        Ok(())
    }

    fn handle_did_open(&mut self, notif: Notification) {
        let params: DidOpenTextDocumentParams = match serde_json::from_value(notif.params) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to parse didOpen params: {e}");
                return;
            }
        };
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.documents.insert(uri.clone(), text.clone());
        self.analyze_and_publish(&uri, &text);
    }

    fn handle_did_change(&mut self, notif: Notification) {
        let params: DidChangeTextDocumentParams = match serde_json::from_value(notif.params) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to parse didChange params: {e}");
                return;
            }
        };
        let uri = params.text_document.uri;
        for change in params.content_changes {
            // Use the text field directly (it's a String for Full/FullWithRange events)
            let text = change.text;
            self.documents.insert(uri.clone(), text);
            if let Some(text) = self.documents.get(&uri).cloned() {
                self.analyze_and_publish(&uri, &text);
            }
            break;
        }
    }

    fn handle_did_close(&mut self, notif: Notification) {
        let params: DidCloseTextDocumentParams = match serde_json::from_value(notif.params) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to parse didClose params: {e}");
                return;
            }
        };
        self.documents.remove(&params.text_document.uri);
        self.publish_diagnostics(&params.text_document.uri, vec![]);
    }

    fn analyze_and_publish(&self, uri: &Uri, text: &str) {
        let result = analyze_file(uri.to_string(), text);
        let diagnostics: Vec<Diagnostic> = result
            .diagnostics
            .iter()
            .map(|d| diagnostic_to_lsp(&result.source, d))
            .collect();
        self.publish_diagnostics(uri, diagnostics);
    }

    fn publish_diagnostics(&self, uri: &Uri, diagnostics: Vec<Diagnostic>) {
        let params = PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics,
            version: None,
        };
        let _ = self.connection.sender.send(Message::Notification(Notification::new(
            "textDocument/publishDiagnostics".to_string(),
            serde_json::to_value(params).unwrap(),
        )));
    }

    fn handle_hover(&mut self, req: Request) {
        let params: HoverParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => {
                self.send_error(req.id, format!("Invalid hover params: {e}"));
                return;
            }
        };
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let result = self.documents.get(&uri).map(|text| {
            let analysis = analyze_file(uri.to_string(), text);
            let label = uri.as_str().rsplit('/').next().unwrap_or("untitled.nv").to_string();
            get_hover(&analysis.resolved, &analysis.typed, &analysis.source, position, &label)
        }).flatten();

        self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::to_value(result).unwrap(),
        )));
    }

    fn handle_definition(&mut self, req: Request) {
        let params: GotoDefinitionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => {
                self.send_error(req.id, format!("Invalid definition params: {e}"));
                return;
            }
        };
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let result = self.documents.get(&uri).and_then(|text| {
            let analysis = analyze_file(uri.to_string(), text);
            find_definition_at(&analysis.resolved, &analysis.source, position).map(|def| {
                Location::new(
                    uri.clone(),
                    span_to_range(&analysis.source, &def.span),
                )
            })
        });

        self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::to_value(result).unwrap(),
        )));
    }

    fn handle_completion(&mut self, req: Request) {
        let params: CompletionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => {
                self.send_error(req.id, format!("Invalid completion params: {e}"));
                return;
            }
        };
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let result = self.documents.get(&uri).map(|text| {
            let analysis = analyze_file(uri.to_string(), text);
            get_completions(&analysis.resolved, &analysis.source, position)
        }).unwrap_or_default();

        self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::to_value(result).unwrap(),
        )));
    }

    fn send_error(&self, id: lsp_server::RequestId, message: String) {
        let _ = self.connection.sender.send(Message::Response(Response::new_err(
            id,
            lsp_server::ErrorCode::InternalError as i32,
            message,
        )));
    }
}

fn main() -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let mut server = ServerState::new(connection);
    let result = server.run();
    io_threads.join().unwrap();
    result
}