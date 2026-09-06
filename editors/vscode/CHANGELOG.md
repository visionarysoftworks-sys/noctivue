# Noctivue VS Code Extension Release Notes

These notes describe the local development releases of the Noctivue
VS Code extension.

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
