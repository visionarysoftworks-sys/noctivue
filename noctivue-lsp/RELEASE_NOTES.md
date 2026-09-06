# Noctivue LSP Release Notes

These notes are maintainer notes for the local Noctivue development
toolchain, not a public release commitment.

## 0.0.5-phase6

### Added

- Semantic-token capability and full-document semantic-token requests.
- Document symbols and workspace-symbol search over open documents.
- Reference search and symbol rename edits.
- Conservative document formatting.
- Signature help for core I/O and assertion builtins.
- Code-action protocol support as a stable extension point.
- Phase-aware server version beacon:
  `noctivue-lsp 0.0.5-phase6 ready`.

### Updated

- The VS Code extension was bumped to `0.0.5`.
- The extension package was rebuilt as `editors/vscode/noctivue-0.0.5.vsix`.
- The local VS Code installation was updated to that VSIX.
- The client README now documents the Phase 5/6 editor surface.

### Known limitations

- The server analyzes open documents and does not yet maintain a
  persistent project-wide file index.
- Formatting is intentionally minimal and does not yet perform a full
  Noctivue density-level reformat.
- Code actions currently return an empty list.
- Cross-file symbols and imported definitions require the compiler module
  graph to be complete; the LSP must not paper over missing compiler
  semantics.

## 0.0.2-hover2

- Added the version beacon and richer hover cards.

## 0.0.1

- Initial diagnostics, hover, definition, completion, and document-sync
  support.
