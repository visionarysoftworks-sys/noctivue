# Noctivue Language Server

`noctivue-lsp` is the editor-facing language server for Noctivue. It
uses the compiler frontend and communicates over standard input/output
using JSON-RPC and the Language Server Protocol.

## Build and run

From the repository root:

```text
cargo build --release -p noctivue-lsp
```

The binary is written to:

```text
target/release/noctivue-lsp.exe
```

The server is normally started by the VS Code extension. For protocol
debugging, it can also be launched directly over stdio by an LSP client.

## Supported editor features

The current server advertises and handles:

- Full-document synchronization
- Diagnostics on open and change
- Hover information
- Go to definition
- Completion
- References
- Rename
- Document and workspace symbols
- Document formatting
- Semantic tokens
- Signature help
- Code-action requests

The server version beacon is emitted through `window/logMessage` when
the client finishes initialization. This makes it possible to confirm
which binary VS Code is actually running.

## Compiler integration

Analysis currently reuses the public compiler analysis API:

```text
source -> lexer -> parser -> resolver -> type checker -> analysis queries
```

The LSP should not maintain a second parser or type system. New language
features should first be represented in the compiler AST/HIR and then
exposed through `compiler::analysis`.

## Phase coverage

This server is intended to cover the editor requirements through the
Phase 5/6 development window:

- Phase 3/M2: native-aware compiler diagnostics and symbol information
- Phase 4/M3: formatter, package/project foundations, documentation
  tooling, and completion/navigation support
- Phase 5/M4: project-scale diagnostics and source navigation for
  networked/data-backed applications
- Phase 6/M5: workspace symbols, refactoring primitives, code-action
  plumbing, and production-oriented project workflows

“Phase coverage” means the protocol surface is present and can evolve
with those phases. It does not claim that every M4/M5 library, runtime,
debugger, or UI feature is already implemented.

## VS Code client

The matching client lives in `editors/vscode`. Build and install the
developer extension with:

```text
cd editors/vscode
npm install
npm run compile
npx vsce package --no-dependencies --out noctivue-0.0.7.vsix
code --install-extension noctivue-0.0.7.vsix --force
```

Set `noctivue.lsp.serverPath` when the server is not in the extension's
expected repository-relative `target/release` location.

## Current limitations

- Workspace analysis currently uses open-document state; unopened
  project files are not indexed by the server yet.
- Formatting is intentionally conservative and currently normalizes
  trailing whitespace and the final newline.
- Code-action requests are supported at the protocol boundary but do
  not yet provide automatic fixes.
- Full module-graph analysis depends on the compiler's module resolver;
  the LSP must not infer imports by text concatenation.
