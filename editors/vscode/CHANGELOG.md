# Noctivue VS Code Extension Release Notes

These notes describe the local development releases of the Noctivue
VS Code extension.

## 0.0.7

### Fixed

- Semantic tokens are now driven by the real compiler lexer instead of
  a whitespace heuristic: keywords, functions, types, variables,
  strings, numbers, and comments (including multi-line block comments)
  get exact spans and kinds. Operators and punctuation are left to the
  TextMate grammar. Previously almost every word was reported as
  `variable`, flattening highlighting to a single color.
- Added `semanticTokenColors` to the Noctivue Dark theme so server-side
  semantic tokens render in the theme palette.

### Updated

- The extension version is now `0.0.7`.
- The packaged artifact is `noctivue-0.0.7.vsix`.
- The LSP server version beacon is `0.0.7-phase6`.

## 0.0.6

### Fixed

- Implemented the `textDocument/semanticTokens/range` request on the
  server. The capability advertised range support but the handler was
  missing, so VS Code logged `Method not found:
  textDocument/semanticTokens/range` on every visible range.
- Added `shutdown` / `exit` lifecycle handling for clean restarts.

### Updated

- The extension version is now `0.0.6`.
- The packaged artifact is `noctivue-0.0.6.vsix`.
- The LSP server version beacon is `0.0.6-phase6`.

### Known limitations

- The LSP currently indexes open documents rather than maintaining a
  persistent project-wide symbol database.
- Formatting is conservative and currently normalizes whitespace and the
  final newline.
- Code actions are protocol-ready but do not yet provide automatic fixes.
- Cross-file imported symbols depend on the compiler module graph.

## 0.0.5

### Added

- Updated the language-server client contract for Phase 5/6 tooling.
- Semantic tokens for Noctivue source files.
- Document symbols and workspace-symbol search.
- References and rename support.
- Document formatting support.
- Signature help for core builtins.
- Code-action request support.
- A workspace-features setting for disabling the newer LSP features when
  testing an older server binary.

### Updated

- The extension version is now `0.0.5`.
- The packaged artifact is `noctivue-0.0.5.vsix`.
- The LSP server version beacon is `0.0.5-phase6`.

### Known limitations

- The LSP currently indexes open documents rather than maintaining a
  persistent project-wide symbol database.
- Formatting is conservative and currently normalizes whitespace and the
  final newline.
- Code actions are protocol-ready but do not yet provide automatic fixes.
- Cross-file imported symbols depend on the compiler module graph.

## 0.0.4

- Added the current extension packaging, file icon, theme, snippets, and
  generated TextMate grammar workflow.

## 0.0.1

- Initial Noctivue language support for VS Code.
