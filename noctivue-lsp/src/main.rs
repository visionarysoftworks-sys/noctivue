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
const SERVER_VERSION: &str = "0.0.7-phase6";

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
            references_provider: Some(OneOf::Left(true)),
            rename_provider: Some(OneOf::Left(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            document_formatting_provider: Some(OneOf::Left(true)),
            semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
                SemanticTokensOptions {
                    legend: SemanticTokensLegend {
                        token_types: vec![
                            "namespace".into(), "type".into(), "function".into(),
                            "variable".into(), "keyword".into(), "string".into(),
                            "number".into(), "comment".into(),
                        ],
                        token_modifiers: vec![],
                    },
                    range: Some(true),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                    ..Default::default()
                },
            )),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(vec!["(".into(), ",".into()]),
                retrigger_characters: Some(vec![",".into()]),
                work_done_progress_options: Default::default(),
            }),
            code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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
            "textDocument/references" => self.handle_references(req),
            "textDocument/rename" => self.handle_rename(req),
            "textDocument/documentSymbol" => self.handle_document_symbols(req),
            "workspace/symbol" => self.handle_workspace_symbols(req),
            "textDocument/formatting" => self.handle_formatting(req),
            "textDocument/semanticTokens/full" => self.handle_semantic_tokens(req),
            "textDocument/semanticTokens/range" => self.handle_semantic_tokens_range(req),
            "shutdown" => self.handle_shutdown(req),
            "textDocument/signatureHelp" => self.handle_signature_help(req),
            "textDocument/codeAction" => self.handle_code_action(req),
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
            "exit" => std::process::exit(0),
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

    fn text_at(&self, uri: &Uri) -> Option<&str> {
        self.documents.get(uri).map(String::as_str)
    }

    fn handle_references(&mut self, req: Request) {
        let params: ReferenceParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid references params: {e}")); return; }
        };
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let result = self.text_at(&uri).map(|text| {
            let word = word_at(text, position);
            occurrences(text, &word).into_iter().map(|range| Location::new(uri.clone(), range)).collect::<Vec<_>>()
        }).unwrap_or_default();
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_rename(&mut self, req: Request) {
        let params: RenameParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid rename params: {e}")); return; }
        };
        let uri = params.text_document_position.text_document.uri;
        let result = self.text_at(&uri).map(|text| {
            let old = word_at(text, params.text_document_position.position);
            let edits = occurrences(text, &old).into_iter()
                .map(|range| TextEdit { range, new_text: params.new_name.clone() })
                .collect();
            WorkspaceEdit { changes: Some(HashMap::from([(uri.clone(), edits)])), ..Default::default() }
        });
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_document_symbols(&mut self, req: Request) {
        let params: DocumentSymbolParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid document symbols params: {e}")); return; }
        };
        let uri = params.text_document.uri;
        let result = self.text_at(&uri).map(document_symbols).unwrap_or_default();
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_workspace_symbols(&mut self, req: Request) {
        let params: WorkspaceSymbolParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid workspace symbols params: {e}")); return; }
        };
        let query = params.query.to_lowercase();
        let mut result = Vec::new();
        for (uri, text) in &self.documents {
            for symbol in document_symbols(text) {
                if symbol.name.to_lowercase().contains(&query) {
                    result.push(SymbolInformation {
                        name: symbol.name,
                        kind: symbol.kind,
                        tags: None,
                        deprecated: None,
                        location: Location::new(uri.clone(), symbol.range),
                        container_name: None,
                    });
                }
            }
        }
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_formatting(&mut self, req: Request) {
        let params: DocumentFormattingParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid formatting params: {e}")); return; }
        };
        let uri = params.text_document.uri;
        let result = self.text_at(&uri).map(|text| {
            let formatted = text.lines().map(|line| line.trim_end()).collect::<Vec<_>>().join("\n") + "\n";
            vec![TextEdit { range: full_range(text), new_text: formatted }]
        }).unwrap_or_default();
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_semantic_tokens(&mut self, req: Request) {
        let params: SemanticTokensParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid semantic token params: {e}")); return; }
        };
        let uri = params.text_document.uri;
        let data = self.text_at(&uri).map(semantic_tokens).unwrap_or_default();
        let result = SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data });
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_semantic_tokens_range(&mut self, req: Request) {
        let params: SemanticTokensRangeParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid semantic token range params: {e}")); return; }
        };
        let uri = params.text_document.uri;
        let range = params.range;
        let data = self.text_at(&uri).map(|text| semantic_tokens_in_range(text, range)).unwrap_or_default();
        let result = SemanticTokensRangeResult::Tokens(SemanticTokens { result_id: None, data });
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_shutdown(&mut self, req: Request) {
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::Value::Null,
        )));
    }

    fn handle_signature_help(&mut self, req: Request) {
        let params: SignatureHelpParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid signature help params: {e}")); return; }
        };
        let uri = params.text_document_position_params.text_document.uri;
        let result = self.text_at(&uri).and_then(|text| {
            let prefix = text_before(text, params.text_document_position_params.position);
            let name = prefix.rsplit_once('(')?.1.split_whitespace().last()?;
            let signatures = [
                ("print(value: String)", "Prints without a trailing newline."),
                ("println(value: String)", "Prints with a trailing newline."),
                ("assert(condition: Bool, message: String)", "Panics when condition is false."),
            ];
            signatures.iter().find(|(sig, _)| sig.starts_with(name)).map(|(sig, doc)| SignatureHelp {
                signatures: vec![SignatureInformation {
                    label: (*sig).into(), documentation: Some(Documentation::String((*doc).into())),
                    parameters: None, active_parameter: None,
                }],
                active_signature: Some(0), active_parameter: Some(0),
            })
        });
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_code_action(&mut self, req: Request) {
        let _params: CodeActionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid code action params: {e}")); return; }
        };
        let result: Vec<CodeActionOrCommand> = Vec::new();
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }
}

fn text_before(text: &str, position: Position) -> String {
    text.lines().take(position.line as usize + 1).collect::<Vec<_>>().join("\n")
}

fn word_at(text: &str, position: Position) -> String {
    let line = text.lines().nth(position.line as usize).unwrap_or("");
    let col = position.character as usize;
    let bytes = line.as_bytes();
    let mut start = col.min(bytes.len());
    let mut end = start;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') { start -= 1; }
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') { end += 1; }
    line[start..end].to_string()
}

fn position_at(text: &str, offset: usize) -> Position {
    let prefix = &text[..offset.min(text.len())];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
    let character = prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32;
    Position::new(line, character)
}

fn full_range(text: &str) -> Range {
    Range::new(Position::new(0, 0), position_at(text, text.len()))
}

fn occurrences(text: &str, word: &str) -> Vec<Range> {
    if word.is_empty() { return Vec::new(); }
    text.match_indices(word).filter(|(offset, _)| {
        let before = text[..*offset].chars().next_back();
        let after = text[*offset + word.len()..].chars().next();
        before.map_or(true, |c| !c.is_alphanumeric() && c != '_')
            && after.map_or(true, |c| !c.is_alphanumeric() && c != '_')
    }).map(|(offset, found)| Range::new(position_at(text, offset), position_at(text, offset + found.len()))).collect()
}

fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    text.lines().enumerate().filter_map(|(line, source)| {
        let trimmed = source.trim_start();
        let (kind, name) = if let Some(rest) = trimmed.strip_prefix("fn ") {
            (SymbolKind::FUNCTION, rest.split(['(', ':', ' ']).next()?)
        } else if let Some(rest) = trimmed.strip_prefix("struct ") {
            (SymbolKind::STRUCT, rest.split([':', ' ', '<']).next()?)
        } else if let Some(rest) = trimmed.strip_prefix("enum ") {
            (SymbolKind::ENUM, rest.split([':', ' ', '<']).next()?)
        } else if let Some(rest) = trimmed.strip_prefix("trait ") {
            (SymbolKind::INTERFACE, rest.split([':', ' ', '<']).next()?)
        } else { return None };
        let start = source.len() - trimmed.len();
        let range = Range::new(Position::new(line as u32, start as u32), Position::new(line as u32, source.len() as u32));
        Some(DocumentSymbol { name: name.into(), detail: None, kind, tags: None, deprecated: None, range, selection_range: range, children: None })
    }).collect()
}

struct AbsoluteToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
}

fn absolute_tokens(text: &str) -> Vec<AbsoluteToken> {
    use compiler::lexer::Token;

    // Lex with the real compiler frontend so token boundaries and kinds are
    // exact. Anything the lexer cannot classify (operators, punctuation) is
    // deliberately left without a semantic token so the TextMate grammar
    // keeps painting it.
    let mut sink = compiler::diagnostics::DiagnosticSink::new();
    let spanned = compiler::lexer::lex(text, &mut sink);

    // Byte offset of the start of every line.
    let mut line_starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }

    let mut tokens = Vec::new();
    for st in &spanned {
        let token_type = match &st.node {
            Token::Indent | Token::Dedent | Token::Newline | Token::Eof => continue,
            // Core (§7.1), type/system (§7.2), memory (§7.3) and reserved (§7.4)
            // keywords all render as `keyword` (legend index 4).
            Token::As | Token::Async | Token::Await | Token::Break | Token::Const
            | Token::Continue | Token::Else | Token::Export | Token::For | Token::If
            | Token::Import | Token::In | Token::Let | Token::Loop | Token::Match
            | Token::Mod | Token::Return | Token::Use | Token::Var | Token::While
            | Token::Derive | Token::Enum | Token::Fn | Token::Impl | Token::Struct
            | Token::Trait | Token::Type | Token::Unsafe | Token::Owned
            | Token::Borrow | Token::Managed | Token::Weak | Token::Unowned
            | Token::Task | Token::Actor | Token::Defer | Token::Extern
            | Token::Macro | Token::Native | Token::Operator | Token::Protocol
            | Token::Reflect | Token::Spawn | Token::Static | Token::Where
            | Token::Yield | Token::True | Token::False | Token::BoolLit(_) => 4,
            Token::Ident(name) => {
                let is_type = name.chars().next().is_some_and(|c| c.is_uppercase());
                // Function when the identifier is applied: the next non-blank
                // character on the same line is `(`. Covers both definitions
                // (`fn add(`) and calls (`println(`/ `obj.method(`).
                let mut is_call = false;
                if !is_type {
                    if let Some(rest) = text.get(st.span.end.min(text.len())..) {
                        let mut chars = rest.chars();
                        while matches!(chars.clone().next(), Some(' ' | '\t')) {
                            chars.next();
                        }
                        is_call = matches!(chars.next(), Some('('));
                    }
                }
                if is_call {
                    2
                } else if is_type {
                    1
                } else {
                    3
                }
            }
            Token::IntLit(_) | Token::FloatLit(_) => 6,
            Token::CharLit(_)
            | Token::StringLit(_)
            | Token::InterpStart(_)
            | Token::InterpMiddle(_)
            | Token::InterpEnd(_) => 5,
            Token::LineComment(_)
            | Token::BlockComment(_)
            | Token::DocComment(_)
            | Token::ModDocComment(_) => 7,
            // Operators & punctuation: no semantic token (TextMate paints them).
            _ => continue,
        };
        push_span_fragments(text, &line_starts, st.span.start, st.span.end, token_type, &mut tokens);
    }
    tokens
}

/// Emit one [`AbsoluteToken`] per line covered by the byte range
/// `[start, end)`. Semantic tokens must not span lines, so block comments
/// and multi-line strings are split into per-line fragments.
fn push_span_fragments(
    text: &str,
    line_starts: &[usize],
    start: usize,
    end: usize,
    token_type: u32,
    out: &mut Vec<AbsoluteToken>,
) {
    let start = start.min(text.len());
    let end = end.min(text.len()).max(start);
    if end == start {
        return;
    }
    let mut line = line_starts.partition_point(|&s| s <= start) - 1;
    while line < line_starts.len() && line_starts[line] < end {
        let line_end = if line + 1 < line_starts.len() {
            line_starts[line + 1] - 1 // exclude the '\n'
        } else {
            text.len()
        };
        let seg_start = start.max(line_starts[line]);
        let seg_end = end.min(line_end);
        if seg_end > seg_start {
            out.push(AbsoluteToken {
                line: line as u32,
                start: (seg_start - line_starts[line]) as u32,
                length: (seg_end - seg_start) as u32,
                token_type,
            });
        }
        line += 1;
    }
}

fn encode_tokens(absolute: &[AbsoluteToken]) -> Vec<SemanticToken> {
    let mut tokens = Vec::with_capacity(absolute.len());
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;
    for (i, tok) in absolute.iter().enumerate() {
        let (delta_line, delta_start) = if i == 0 {
            (tok.line, tok.start)
        } else {
            let dl = tok.line - previous_line;
            let ds = if dl == 0 { tok.start - previous_start } else { tok.start };
            (dl, ds)
        };
        tokens.push(SemanticToken { delta_line, delta_start, length: tok.length, token_type: tok.token_type, token_modifiers_bitset: 0 });
        previous_line = tok.line;
        previous_start = tok.start;
    }
    tokens
}

fn semantic_tokens(text: &str) -> Vec<SemanticToken> {
    encode_tokens(&absolute_tokens(text))
}

fn semantic_tokens_in_range(text: &str, range: Range) -> Vec<SemanticToken> {
    let all = absolute_tokens(text);
    let filtered: Vec<AbsoluteToken> = all
        .into_iter()
        .filter(|tok| {
            if tok.line < range.start.line || tok.line > range.end.line {
                return false;
            }
            if tok.line == range.start.line && tok.start + tok.length <= range.start.character {
                return false;
            }
            if tok.line == range.end.line && tok.start >= range.end.character {
                return false;
            }
            true
        })
        .collect();
    encode_tokens(&filtered)
}

fn main() -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let mut server = ServerState::new(connection);
    let result = server.run();
    io_threads.join().unwrap();
    result
}