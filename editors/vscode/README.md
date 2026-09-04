# Noctivue for VS Code

Language support for [Noctivue](https://github.com/noctivue/noctivue) (`.nv` files):
syntax highlighting, a matching dark theme, snippets, file icons, and a
Language Server Protocol (LSP) integration.

## Features

- **Syntax highlighting** — TextMate grammar covering keywords, types,
  functions, variables, strings (with `{interpolation}`), numbers,
  comments, and operators. Scope names use standard TextMate prefixes,
  so the language colors correctly under any theme — not just the
  bundled one.
- **Noctivue Dark theme** — a dark color theme tuned for the grammar
  (`Ctrl+K Ctrl+T`, pick *Noctivue Dark*).
- **LSP-powered editing** (requires the server, see below):
  - Diagnostics (errors and warnings as you type)
  - Hover cards — signature, inferred `Type:`, `Declared in`, and
    `///` documentation for definitions, locals, built-ins, keywords,
    and prelude items
  - Go to definition and workspace-aware completions
- **Snippets** — `fn`, `struct`, `enum`, `main` starters.
- **File icons** — `.nv` files get the Noctivue owl mark in the Explorer.

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

### 0.0.1

- Initial release: TextMate grammar (generated from the compiler's token
  definitions), Noctivue Dark theme, snippets, file icons, and LSP
  client (diagnostics, hover, go-to-definition, completions).
