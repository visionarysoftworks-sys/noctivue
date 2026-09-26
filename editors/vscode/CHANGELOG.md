# Noctivue VS Code Extension Release Notes

These notes describe the local development releases of the Noctivue
VS Code extension.

## 0.0.13

### Added

- `noctivue.excludeFetchedDependencies` (default on) keeps `.noct/`
  out of the Explorer, search, and the file watcher, via
  `configurationDefaults` plus a merge-not-replace sync at activation.
  A dependency's sources are not the user's code: letting the server
  analyze them produces diagnostics that read as the user's own, and
  watching a multi-megabyte tree churns the workspace for nothing.
  Unrelated user excludes are preserved; a read-only workspace logs a
  warning instead of failing activation.

## 0.0.12

### Added

- `.nvir` / `.nvc` ride the `noctivue` language outright (same
  grammar, same file identity — `fileTypes` now `nv, nvir, nvc`,
  generated from `compiler/src/tools/gen_syntax_highlighting.rs`).
  Server tweak: artifacts get hover/completion/symbols but no
  diagnostics (STYLE_GUIDE.md §6.7). Requires a freshly built
  server binary.

## 0.0.11

### Fixed

- `nestpkg` highlighting now uses broad standard scopes so keys,
  dependency names, tiers, and bare values color under stock themes
  too, not just Noctivue Dark: keys/names are `entity.name.tag`
  (YAML convention), bare values `string.unquoted.value`, tiers stay
  `entity.name.type`. Verified with `audit-theme.js --grammar` against
  Dark+, Dark Modern, Light+, High Contrast Black (only the
  conventional `punctuation.*` scopes fall back, same as every
  language), plus full coverage in Noctivue Dark.

## 0.0.10

### Added

- `nestpkg` language in the same extension: `nestpkg.nvpm`, `*.nvpm`,
  and `nestpkg.lock` get highlighting (new `source.nestpkg` grammar),
  LSP diagnostics from the shared `nestpkg` parser crate (one parser
  for `noct` + LSP, no drift), hover cards, completions, outline
  symbols, and canonical formatting. Single VSIX, single `noctivue-lsp`
  binary — the server routes by filename.
- Shared `nestpkg` workspace crate extracted from `noct-cli`; the CLI
  keeps working through a `crate::manifest` re-export shim.

### Updated

- The extension version is now `0.0.10`.
- The LSP server version beacon is `0.0.10-nestpkg1`.

## 0.0.9

### Fixed

- Import aliases no longer report false `unknown identifier` errors:
  `v.foo()` (for `import m as v`) and `m.foo()` (for unaliased
  `import m`) resolve through the module graph exactly like
  `noct run` does — the linker rewrite the compiler applies is now
  mirrored by diagnostic suppression in the server. Aliases of
  *unresolvable* imports still error (their `E0101` plus the
  follow-on `E0201`s stay visible), so genuinely broken imports are
  unaffected.

### Updated

- The extension version is now `0.0.9`.
- The packaged artifact is `noctivue-0.0.9.vsix`.
- The LSP server version beacon is `0.0.9-phase6`.

### Known limitations

- Hover, go-to-definition, and completions still resolve within the
  open file (plus built-ins, keywords, and prelude items); only
  diagnostics cross file boundaries via the module graph.
- Formatting is conservative and currently normalizes whitespace and the
  final newline.
- Code actions are protocol-ready but do not yet provide automatic fixes.

## 0.0.8

### Fixed

- Diagnostics are now import-aware: the server consults the compiler
  module graph for the open file, so flat uses of linked modules
  (`app_name()`, `document_title()`, …) no longer report false
  `unknown identifier` errors, and genuine import problems surface
  with their compiler codes (`E0101` unresolvable module, `E0102`
  private item, `E0103` missing item, `E0109` un-aliasable item,
  `W0101` private-use hint). Previously the server analyzed each file
  alone and disagreed with `noct run` on every multi-file program.

### Updated

- The extension version is now `0.0.8`.
- The packaged artifact is `noctivue-0.0.8.vsix`.
- The LSP server version beacon is `0.0.8-phase6`.
- Requires a freshly built server binary (see Requirements in the
  README): binaries older than this release predate import-aware
  diagnostics.

### Known limitations

- Hover, go-to-definition, and completions still resolve within the
  open file (plus built-ins, keywords, and prelude items); only
  diagnostics cross file boundaries via the module graph.
- Formatting is conservative and currently normalizes whitespace and the
  final newline.
- Code actions are protocol-ready but do not yet provide automatic fixes.

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
