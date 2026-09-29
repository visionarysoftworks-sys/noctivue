//! noctivue-analyzer — Noctivue Language Server Protocol (LSP) server.
//!
//! Engine name: `noctivue-analyzer` (the rust-analyzer / tsserver equivalent
//! for Noctivue: hover, go-to-definition, completions, references).
//! Binary name: `noctivue-lsp` (kept for VS Code extension + script compat).
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
    analyze_file_with_roots, collect_impl_spans, definition_name_span, diagnostic_to_lsp,
    find_cross_file_target, find_definition_at, find_item_span, find_trait_context, find_type_at,
    fs_path_to_uri, get_completions, get_hover, hover_cross_file_at, identifier_at, name_span_in,
    nominal_name_of, parse_resolved, span_to_range, uri_to_fs_path, AnalysisResult,
};

mod nestpkg;

/// Bump on every behavior-changing server release so the `window/logMessage`
/// beacon in the client's Output panel identifies the running binary.
///
/// Engine: `noctivue-analyzer`. The beacon keeps the `noctivue-lsp` binary
/// name in parentheses so old Output-panel filters still match.
const SERVER_VERSION: &str = "0.0.14-analyzer";

/// Open-document state (noctivue-analyzer).
///
/// `analysis` is the import-aware pipeline result computed once per
/// change in `analyze_and_publish` and shared by hover, definition,
/// type-definition, implementation, and completion. Handlers must read
/// this instead of re-running lex → parse → resolve → typecheck per
/// request (previously 3–4 full pipelines per keystroke).
struct OpenDocument {
    text: String,
    version: i32,
    /// `None` for nestpkg manifests/locks and `.nvir`/`.nvc` artifacts,
    /// which ride different pipelines (or none).
    analysis: Option<AnalysisResult>,
}

struct ServerState {
    connection: Connection,
    documents: HashMap<Uri, OpenDocument>,
    capabilities: ServerCapabilities,
}

impl ServerState {
    fn new(connection: Connection) -> Self {
        let capabilities = ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            // noctivue-analyzer: TS-parity navigation.
            type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
            implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
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
                name: "noctivue-analyzer".to_string(),
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
        // engine binary is answering requests. Engine name first, binary in
        // parens for back-compat with old Output-panel filters.
        let _ = self.connection.sender.send(Message::Notification(Notification::new(
            "window/logMessage".to_string(),
            serde_json::json!({
                "type": 3,
                "message": format!("noctivue-analyzer {SERVER_VERSION} ready (noctivue-lsp)"),
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
            "textDocument/typeDefinition" => self.handle_type_definition(req),
            "textDocument/implementation" => self.handle_implementation(req),
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
        let version = params.text_document.version;
        self.documents.insert(
            uri.clone(),
            OpenDocument {
                text: text.clone(),
                version,
                analysis: None,
            },
        );
        self.analyze_and_publish(&uri, version, &text);
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
        let version = params.text_document.version;
        // FULL sync: the first change carries the whole document.
        if let Some(change) = params.content_changes.into_iter().next() {
            let text = change.text;
            self.documents.insert(
                uri.clone(),
                OpenDocument {
                    text: text.clone(),
                    version,
                    analysis: None,
                },
            );
            self.analyze_and_publish(&uri, version, &text);
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
        self.publish_diagnostics(&params.text_document.uri, None, vec![]);
    }

    fn analyze_and_publish(&mut self, uri: &Uri, version: i32, text: &str) {
        if nestpkg::is_nestpkg_uri(uri) {
            self.publish_diagnostics(
                uri,
                Some(version),
                nestpkg::diagnostics_for_uri(uri, text),
            );
            return;
        }
        if is_artifact_uri(uri) {
            // Build artifacts ride the `.nv` language (hover, completion,
            // symbols all work on dumps) but never get diagnostics:
            // they are generated, so error squiggles would be noise
            // (STYLE_GUIDE.md §6.7).
            self.publish_diagnostics(uri, Some(version), Vec::new());
            return;
        }
        let result = analyze_file_with_roots(uri.to_string(), text, &package_roots_for(uri));
        let diagnostics: Vec<Diagnostic> = result
            .diagnostics
            .iter()
            .map(|d| diagnostic_to_lsp(&result.source, uri, d))
            .collect();
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.analysis = Some(result);
        }
        self.publish_diagnostics(uri, Some(version), diagnostics);
    }

    /// Shared pipeline result for hover/definition/completion: the analysis
    /// cached at the last open/change, recomputed on demand only when the
    /// document was never analyzed (e.g. a request racing didOpen).
    fn cached_analysis(&mut self, uri: &Uri) -> Option<AnalysisResult> {
        if let Some(doc) = self.documents.get(uri) {
            if let Some(analysis) = &doc.analysis {
                return Some(analysis.clone());
            }
        }
        let text = self.documents.get(uri)?.text.clone();
        if nestpkg::is_nestpkg_uri(uri) || is_artifact_uri(uri) {
            return None;
        }
        let analysis = analyze_file_with_roots(uri.to_string(), &text, &package_roots_for(uri));
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.analysis = Some(analysis.clone());
        }
        Some(analysis)
    }

    fn publish_diagnostics(&self, uri: &Uri, version: Option<i32>, diagnostics: Vec<Diagnostic>) {
        let params = PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics,
            version,
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

        if nestpkg::is_nestpkg_uri(&uri) {
            let result = self.text_at(&uri).and_then(|text| {
                let is_lock = nestpkg::is_lock_uri(&uri);
                nestpkg::get_hover(text, position, is_lock)
            });
            self.connection.sender.send(Message::Response(Response::new_ok(
                req.id,
                serde_json::to_value(result).unwrap(),
            )));
            return;
        }
        let result = self.cached_analysis(&uri).and_then(|analysis| {
            // noctivue-analyzer: hover reuses the import-aware analysis
            // cached at the last open/change, so `path:` deps don't
            // produce false unknowns at the hovered call site. Single-file
            // first; an imported name falls through to the cross-file card,
            // which names the defining file (a hover that cannot say where
            // a name comes from is half an answer for an import).
            let label = uri.as_str().rsplit('/').next().unwrap_or("untitled.nv").to_string();
            get_hover(&analysis.resolved, &analysis.typed, &analysis.source, position, &label)
                .or_else(|| {
                    hover_cross_file_at(
                        uri.as_str(),
                        &analysis.resolved,
                        &analysis.source,
                        position,
                        &package_roots_for(&uri),
                    )
                })
        });

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
        let uri = params.text_document_position_params.text_document.uri.clone();
        // nestpkg files have no cross-file definitions: return null
        // instead of running the `.nv` resolver on manifest text.
        if nestpkg::is_nestpkg_uri(&uri) {
            let _ = self.connection.sender.send(Message::Response(Response::new_ok(
                req.id,
                serde_json::Value::Null,
            )));
            return;
        }
        let position = params.text_document_position_params.position;

        let result = self.cached_analysis(&uri).and_then(|analysis| {
            // noctivue-analyzer: goto reuses the cached import-aware
            // analysis; targets are name-only spans (see
            // `definition_name_span`). Cross-file (imports, flat dep uses,
            // stdlib) resolves through the module graph before falling
            // back to same-file.
            let roots = package_roots_for(&uri);
            let text = analysis.source.clone();
            let single = find_definition_at(
                &analysis.resolved,
                &analysis.typed,
                &analysis.source,
                position,
            );
            let ident = identifier_at(&text, position);
            // Cross-file first for imports and unknown names; same-file
            // locals/fields/variants never leave the file.
            let cross_first = match (&single, &ident) {
                (Some(def), _) => matches!(
                    def.kind,
                    compiler::analysis::DefinitionKind::Import
                ),
                (None, Some(_)) => true,
                (None, None) => false,
            };
            if cross_first {
                if let Some(name) = ident {
                    if let Some(loc) = self.cross_file_location(&uri, &analysis.resolved, &name, &roots) {
                        return Some(loc);
                    }
                }
            }
            if let Some(def) = single {
                let name_span = definition_name_span(&analysis.source, &def);
                return Some(Location::new(
                    uri.clone(),
                    span_to_range(&analysis.source, &name_span),
                ));
            }
            None
        });

        self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::to_value(result).unwrap(),
        )));
    }

    fn handle_type_definition(&mut self, req: Request) {
        let params: GotoDefinitionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => {
                self.send_error(req.id, format!("Invalid typeDefinition params: {e}"));
                return;
            }
        };
        let uri = params.text_document_position_params.text_document.uri.clone();
        if nestpkg::is_nestpkg_uri(&uri) {
            let _ = self.connection.sender.send(Message::Response(Response::new_ok(
                req.id,
                serde_json::Value::Null,
            )));
            return;
        }
        let position = params.text_document_position_params.position;
        let result = self.cached_analysis(&uri).and_then(|analysis| {
            let roots = package_roots_for(&uri);
            let text = analysis.source.clone();
            // Expression type first (`user.name` → field type).
            let mut nominal = find_type_at(&analysis.typed, &text, position)
                .and_then(|ty| nominal_name_of(&ty));
            // Fallback: cursor on a type name itself (`User` in `-> User`).
            if nominal.is_none() {
                if let Some(ident) = identifier_at(&text, position) {
                    let is_type = find_item_span(&analysis.resolved, &ident).is_some_and(
                        |(kind, _, _)| {
                            matches!(
                                kind,
                                compiler::analysis::DefinitionKind::Struct
                                    | compiler::analysis::DefinitionKind::Enum
                                    | compiler::analysis::DefinitionKind::Trait
                            )
                        },
                    );
                    if is_type {
                        nominal = Some(ident);
                    }
                }
            }
            let name = nominal?;
            // Same-file first, then graph (deps, vendor/, stdlib).
            if let Some((_, span, _)) = find_item_span(&analysis.resolved, &name) {
                let range = span_to_range(&text, &name_span_in(&text, &span, &name));
                return Some(Location::new(uri.clone(), range));
            }
            self.cross_file_location(&uri, &analysis.resolved, &name, &roots)
        });
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::to_value(result).unwrap(),
        )));
    }

    fn handle_implementation(&mut self, req: Request) {
        let params: GotoDefinitionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => {
                self.send_error(req.id, format!("Invalid implementation params: {e}"));
                return;
            }
        };
        let uri = params.text_document_position_params.text_document.uri.clone();
        if nestpkg::is_nestpkg_uri(&uri) {
            let _ = self.connection.sender.send(Message::Response(Response::new_ok(
                req.id,
                serde_json::Value::Null,
            )));
            return;
        }
        let position = params.text_document_position_params.position;
        let result: Vec<Location> = self
            .cached_analysis(&uri)
            .map(|analysis| {
                let roots = package_roots_for(&uri);
                let text = analysis.source.clone();
                let ident = identifier_at(&text, position).unwrap_or_default();
                let Some((trait_name, method)) = find_trait_context(
                    &analysis.resolved,
                    &text,
                    position,
                    &ident,
                ) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                // Same-file impls.
                for span in
                    collect_impl_spans(&analysis.resolved, &text, &trait_name, method.as_deref())
                {
                    out.push(Location::new(uri.clone(), span_to_range(&text, &span)));
                }
                // Cross-file impls across the graph closure.
                out.extend(self.cross_file_impls(&uri, &trait_name, method.as_deref(), &roots));
                out
            })
            .unwrap_or_default();
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(
            req.id,
            serde_json::to_value(result).unwrap(),
        )));
    }

    /// Resolve `ident` through the module graph to a Location in another
    /// file (imports, flat dep uses, stdlib). Returns `None` when the name
    /// is same-file or unresolvable. Open documents shadow disk reads so
    /// unsaved edits still jump correctly.
    fn cross_file_location(
        &self,
        from_uri: &Uri,
        program: &compiler::ast::Program,
        ident: &str,
        roots: &[std::path::PathBuf],
    ) -> Option<Location> {
        let from_disk = uri_to_fs_path(from_uri.as_str())?;
        if !from_disk.is_file() {
            return None;
        }
        let (target_path, item) = find_cross_file_target(&from_disk, program, ident, roots)?;
        let (target_uri, target_text) = self.target_source(&target_path)?;
        match item {
            Some(item_name) => {
                let target_program = parse_resolved(&target_text);
                let (_, span, _) = find_item_span(&target_program, &item_name)?;
                let range =
                    span_to_range(&target_text, &name_span_in(&target_text, &span, &item_name));
                Some(Location::new(target_uri, range))
            }
            // `import pkg` (bare module, no item): jump to file start.
            None => Some(Location::new(
                target_uri,
                Range::new(Position::new(0, 0), Position::new(0, 0)),
            )),
        }
    }

    /// All cross-file `impl Trait` method locations (excludes the current file).
    fn cross_file_impls(
        &self,
        from_uri: &Uri,
        trait_name: &str,
        method: Option<&str>,
        roots: &[std::path::PathBuf],
    ) -> Vec<Location> {
        let Some(from_disk) = uri_to_fs_path(from_uri.as_str()) else {
            return Vec::new();
        };
        let Ok(graph) = compiler::modules::ModuleGraph::load_with(
            &[from_disk.clone()],
            roots,
        ) else {
            return Vec::new();
        };
        let canonical = from_disk.canonicalize().unwrap_or(from_disk);
        let mut out = Vec::new();
        for file in &graph.files {
            if file.path == canonical {
                continue;
            }
            let (uri, text) = match self.target_source(&file.path) {
                Some(t) => t,
                None => continue,
            };
            let program = parse_resolved(&text);
            for span in collect_impl_spans(&program, &text, trait_name, method) {
                out.push(Location::new(uri.clone(), span_to_range(&text, &span)));
            }
        }
        out
    }

    /// Target file URI + text: open-document state wins, else disk.
    fn target_source(&self, path: &std::path::Path) -> Option<(Uri, String)> {
        let uri_str = fs_path_to_uri(path);
        let uri: Uri = uri_str.parse().ok()?;
        if let Some(doc) = self.documents.get(&uri) {
            return Some((uri, doc.text.clone()));
        }
        let text = std::fs::read_to_string(path).ok()?;
        Some((uri, text))
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

        if nestpkg::is_nestpkg_uri(&uri) {
            let result = self.text_at(&uri).map(|text| {
                nestpkg::get_completions(&uri, text, position)
            }).unwrap_or_default();
            self.connection.sender.send(Message::Response(Response::new_ok(
                req.id,
                serde_json::to_value(result).unwrap(),
            )));
            return;
        }
        let result = self.cached_analysis(&uri).map(|analysis| {
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
        self.documents.get(uri).map(|doc| doc.text.as_str())
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
        let result = self.text_at(&uri).map(|text| {
            if nestpkg::is_nestpkg_uri(&uri) {
                return nestpkg::document_symbols(text, nestpkg::is_lock_uri(&uri));
            }
            document_symbols(text)
        }).unwrap_or_default();
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_workspace_symbols(&mut self, req: Request) {
        let params: WorkspaceSymbolParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid workspace symbols params: {e}")); return; }
        };
        let query = params.query.to_lowercase();
        let mut result = Vec::new();
        for (uri, doc) in &self.documents {
            for symbol in document_symbols(&doc.text) {
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
            if nestpkg::is_nestpkg_uri(&uri) {
                return nestpkg::formatting(&uri, text);
            }
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
        // nestpkg coloring comes from the TextMate grammar; the `.nv`
        // lexer would mis-tokenize manifest text, so return empty.
        let data = self.text_at(&uri).map(|text| {
            if nestpkg::is_nestpkg_uri(&uri) {
                return Vec::new();
            }
            semantic_tokens(text)
        }).unwrap_or_default();
        let result = SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data });
        let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
    }

    fn handle_semantic_tokens_range(&mut self, req: Request) {
        let params: SemanticTokensRangeParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => { self.send_error(req.id, format!("Invalid semantic token range params: {e}")); return; }
        };
        let uri = params.text_document.uri;
        if nestpkg::is_nestpkg_uri(&uri) {
            let result = SemanticTokensRangeResult::Tokens(SemanticTokens { result_id: None, data: Vec::new() });
            let _ = self.connection.sender.send(Message::Response(Response::new_ok(req.id, serde_json::to_value(result).unwrap())));
            return;
        }
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

/// Byte offset → LSP position (UTF-16 code units, not bytes/chars).
fn position_at(text: &str, offset: usize) -> Position {
    let prefix = &text[..offset.min(text.len())];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
    let character: usize = prefix
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| c.len_utf16())
        .sum();
    Position::new(line, character as u32)
}

fn full_range(text: &str) -> Range {
    Range::new(Position::new(0, 0), position_at(text, text.len()))
}

/// Build artifacts (`.nvc`) and IR dumps (`.nvir`) share the `.nv`
/// language outright — same grammar, same analysis path — with exactly
/// one tweak: no diagnostics (see `analyze_and_publish`). Query
/// parameters are ignored the same way, so `?v=2`-style URIs route
/// identically.
fn is_artifact_uri(uri: &Uri) -> bool {
    let s = uri.as_str();
    let path = s.split('?').next().unwrap_or(s);
    path.ends_with(".nvir") || path.ends_with(".nvc")
}

/// Package roots for the module graph, derived from the file's own
/// project manifest: the directories holding each declared `path:`
/// dependency.
///
/// The editor was reporting a false `E0101 cannot resolve imported
/// module` plus a flood of false `E0201 unknown identifier` squiggles for
/// any project with a first-party dependency, because the graph was built
/// with no roots while `noct run` on the very same file succeeded. The
/// CLI had the identical bug; both are fixed by handing the graph the
/// roots the manifest declares.
///
/// Two limits, stated rather than hidden:
/// - The content store is NOT supplied. Its layout lives in the
///   toolchain (`noct-cli/src/store.rs`), not the compiler or the shared
///   `nestpkg` crate, and duplicating that XDG/`NOCT_STORE` logic here
///   would be a second source of truth that drifts. An import that
///   resolves only through the store can still show a false `E0101` in
///   the editor; `noct get` (or a committed `vendor/`) avoids it.
/// - Path dependencies are read from the nearest `nestpkg.nvpm`, walking
///   up from the file, which is the same project the CLI resolves against.
fn package_roots_for(uri: &Uri) -> Vec<std::path::PathBuf> {
    let Some(disk) = uri_to_fs_path(uri.as_str()) else {
        return Vec::new();
    };
    let mut dir = disk.parent().map(|p| p.to_path_buf());
    let mut manifest_dir = None;
    while let Some(d) = dir.clone() {
        let candidate = d.join("nestpkg.nvpm");
        if candidate.is_file() {
            manifest_dir = Some(d);
            break;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    let Some(manifest_dir) = manifest_dir else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(manifest_dir.join("nestpkg.nvpm")) else {
        return Vec::new();
    };
    let Ok(manifest) = ::nestpkg::parse_manifest(&text) else {
        // A malformed manifest is the manifest language's job to report,
        // not this function's: returning no roots degrades to the old
        // single-file behaviour instead of inventing errors.
        return Vec::new();
    };
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for dep in manifest.dependencies.iter().chain(&manifest.dev_dependencies) {
        let ::nestpkg::Source::Path(rel) = &dep.source else {
            continue;
        };
        let dir = manifest_dir.join(rel);
        // The resolver's roots hold packages, so hand over the containing
        // directory: it finds `<root>/<pkg>/lib/…` by listing them.
        if let Some(parent) = dir.parent() {
            let parent = parent.to_path_buf();
            if parent.is_dir() && !roots.contains(&parent) {
                roots.push(parent);
            }
        }
    }
    roots
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

#[cfg(test)]
mod artifact_tests {
    use super::*;

    fn uri(path: &str) -> Uri {
        path.parse().unwrap()
    }

    #[test]
    fn artifacts_route_quietly_but_stay_on_the_nv_path() {
        assert!(is_artifact_uri(&uri("file:///pkg/app.nvir")));
        assert!(is_artifact_uri(&uri("file:///pkg/app.nvc")));
        // Query strings must not change routing.
        assert!(is_artifact_uri(&uri("file:///pkg/app.nvc?v=2")));
        // Everything else is untouched: manifests, locks, sources,
        // and lookalike suffixes that must NOT match.
        assert!(!is_artifact_uri(&uri("file:///p/nestpkg.nvpm")));
        assert!(!is_artifact_uri(&uri("file:///p/nestpkg.lock")));
        assert!(!is_artifact_uri(&uri("file:///p/main.nv")));
        assert!(!is_artifact_uri(&uri("file:///p/nvc.rs")));
        assert!(!is_artifact_uri(&uri("file:///p/anvir.nv")));
    }
}