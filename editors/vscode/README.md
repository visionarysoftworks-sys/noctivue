# Noctivue for VS Code

Language support for [Noctivue](https://github.com/noctivue/noctivue) (`.nv` files)
plus `nestpkg` manifests (`nestpkg.nvpm`, `*.nvpm`, `nestpkg.lock`):
syntax highlighting, a matching dark theme, snippets, file icons, and a
Language Server Protocol (LSP) integration — all in this one extension.

## Features

- **Syntax highlighting** — TextMate grammar covering keywords, types,
  functions, variables, strings (with `{interpolation}`), numbers,
  comments, and operators. Scope names use standard TextMate prefixes,
  so the language colors correctly under any theme — not just the
  bundled one.
- **Noctivue Dark theme** — a dark color theme tuned for the grammar
  (`Ctrl+K Ctrl+T`, pick *Noctivue Dark*).
- **LSP-powered editing** (requires the server, see below):
  - Diagnostics (errors and warnings as you type, import-aware via
    the compiler module graph)
  - Hover cards — signature, inferred `Type:`, `Declared in`, and
    `///` documentation for definitions, locals, built-ins, keywords,
    and prelude items
  - Go to definition and workspace-aware completions
  - Semantic tokens, document/workspace symbols, references, rename,
    formatting, signature help, and code-action support
- **Snippets** — `fn`, `struct`, `enum`, `main` starters.
- **File icons** — `.nv` files get the Noctivue owl mark in the Explorer.
- **nestpkg manifests** — `nestpkg.nvpm` / `nestpkg.lock` get their own
  language ID in the same extension: grammar highlighting, LSP
  diagnostics (same parser as `noct`), hover cards for keys/tiers /
  version forms, completions, outline symbols, and canonical formatting.

## Requirements

The editing features (diagnostics, hover, completions) are provided by
the `noctivue-lsp` server binary, which is built from the Noctivue
repository and is **not** bundled in this extension:

```text
cargo build --release -p noctivue-lsp
```

Then point the extension at it (Settings → search `noctivue`):

```json
"noctivue.lsp.serverPath": "E:\\Projects\\Noctivue\\0.0.1\\target\\release\\noctivue-lsp.exe"
```

Syntax highlighting, the theme, snippets, and file icons work with no
setup.

## Extension Settings

| Setting | Default | Description |
|---|---|---|
| `noctivue.lsp.enabled` | `true` | Enable the Noctivue language server |
| `noctivue.lsp.serverPath` | `""` (auto-detect) | Path to the `noctivue-lsp` binary |

## Commands

| Command | Description |
|---|---|
| `Noctivue: Restart Language Server` (`noctivue.restartServer`) | Restart the LSP server |
| `Noctivue: Show Output` (`noctivue.showOutput`) | Open the language-server output channel |

## Release Notes

The detailed extension changelog is in
[`CHANGELOG.md`](./CHANGELOG.md).

### 0.0.9

- Import aliases (`v.foo()`, `m.foo()`) no longer show false
  `unknown identifier` errors — diagnostics follow the module
  graph, matching `noct run`. Requires a server binary built from
  this release or later.

### 0.0.8

- Diagnostics are now import-aware: the server reads the open file's
  module graph, so uses of linked modules no longer show false
  `unknown identifier` errors and real import problems arrive with
  their compiler codes. Requires a server binary built from this
  release or later.

### 0.0.7

- Semantic tokens now come from the compiler lexer (exact
  keyword/function/type/variable/string/number/comment spans) and the
  Noctivue Dark theme defines `semanticTokenColors` for them.

### 0.0.6

- Fixed the server's missing `textDocument/semanticTokens/range`
  handler (previously surfaced as `Method not found` in the LSP log)
  and added `shutdown`/`exit` lifecycle handling.

### 0.0.5

- Updated the LSP client/server contract for Phase 5/6 tooling:
  semantic tokens, symbols, references, rename, formatting, signature
  help, and code actions.

### 0.0.1

- Initial release: TextMate grammar (generated from the compiler's token
  definitions), Noctivue Dark theme, snippets, file icons, and LSP
  client (diagnostics, hover, go-to-definition, completions).
