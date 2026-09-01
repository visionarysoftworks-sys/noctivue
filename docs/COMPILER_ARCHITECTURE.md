# COMPILER_ARCHITECTURE.md — Compiler Pipeline & Parsing Strategy

**Status:** Pipeline shape Confirmed; disambiguation algorithm in §4 is
Proposed, pending prototype validation (this is the single most
important open implementation question in the project, per design
brief §39).

## 1. Implementation Language

Rust (ADR-002).

## 2. Pipeline

```text
Source (.nv)
  → Lexer            (tokens + INDENT/DEDENT synthesis, SYNTAX.md §10.1)
  → Parser            (produces AST per SYNTAX.md grammar; purely syntactic)
  → AST
  → Name Resolution    (resolves imports, binds identifiers, disambiguates
                        declaration KIND — see §4)
  → Type Checking      (local inference, trait resolution, mode checking)
  → HIR / Typed AST
  → NIR                (SSA lowering, see NIR.md)
  → Backend            (Cranelift native, or bytecode VM for M1, or LLVM
                        for optimized release builds — see §5)
```

## 3. Frontend Responsibilities

| Stage | Owns |
|---|---|
| Lexer | UTF-8 handling, tokenization, indentation-sensitivity (offside rule), comment/doc-comment extraction |
| Parser | Pure grammar per SYNTAX.md §10 — produces a syntax tree with **zero** semantic guessing (see §4) |
| Name Resolution | Scope construction, import resolution, and the declaration-kind disambiguation that the parser deliberately does not attempt |
| Type Checker | Local inference, trait bound checking, `Option`/`Result` flow, mode consistency (native vs. managed boundary checks per MEMORY_MODEL.md §7) |

## 4. Disambiguating Colon-First Declarations (the core parsing question)

The parser **MUST NOT** guess whether `Foo:` is a struct, a function, a
UI component, or a namespace based on identifier casing or naming
convention — casing is a style convention (LANGUAGE_SPEC.md §4), never
load-bearing for parsing (DECISIONS.md Issue 1).

**Parser-level rule (purely syntactic, no semantic knowledge required):**

1. If the statement begins with a reserved control-flow keyword
   (`if`, `for`, `while`, `match`, `return`, `break`, `continue`,
   `async`, `await`, `import`, `export`, ...), it parses as that
   construct. Unambiguous — keywords are reserved (LANGUAGE_SPEC.md §7).
2. If the statement begins with an explicit declaration keyword
   (`fn`, `struct`, `enum`, `trait`, `impl`, `const`, `let`, `var`,
   `state`), it parses as that declaration kind directly. Unambiguous.
3. Otherwise — a bare `Identifier [ ( params ) ] [ -> Type ] :` — the
   parser produces a single generic node, tentatively called a
   **`bare_decl`**, capturing: the identifier, an optional parameter
   list, an optional return type, and a block body. The parser commits
   to this shape without deciding whether it's a struct, function, or
   component. This is what SYNTAX.md §11 calls out as intentionally
   deferred past parsing.

**Resolver-level rule (semantic, after parsing):**

A `bare_decl` is classified as one of:

- **struct** — if the body consists exclusively of `field_decl`-shaped
  lines (`identifier: type_expr`) with no calls.
- **function** — if a return type (`-> Type`) is present, or the body is
  a single expression / contains `return`, `let`, or other statement
  forms rather than field declarations.
- **component (builder call)** — if the body consists of nested calls
  (`identifier(...): ...` / `identifier(...)` patterns) that resolve,
  via name resolution, to functions or macros rather than types — i.e.,
  the declaration is itself sugar for "define a function that builds a
  tree of calls," per the desugaring rule proposed in DECISIONS.md
  Issue 2.

This classification is **structural** (shape of the body), not based on
naming convention, and is intentionally deferred to name resolution
(where full symbol information is available) rather than attempted
during parsing (where it is not). This keeps the grammar itself
context-free and avoids a PEG-style backtracking parser, while still
achieving the "no naming-convention-dependent parsing" requirement.

**Known remaining ambiguity (flagged, not yet resolved):** a
zero-argument, zero-field bare declaration with an empty or
single-nested-call body — e.g. `Splash: loading_screen()` — is
structurally compatible with both "component calling one builder" and
"a function returning the result of `loading_screen()`" if
`loading_screen` could be either a function or a component-builder
itself. Current mitigation proposal: resolution is transitive — the
classification of `loading_screen` itself must already be known
(components/functions can't be mutually deduced in a cycle without a
defined resolution order), enforced as a compile error
("declaration classification cycle") rather than a silent guess if a
genuine cycle occurs. **Status: Proposed**, needs adversarial test
cases before promotion to Confirmed.

## 5. Backend Strategy

Cranelift first (ADR-003): faster development cycle, easier embedding,
useful early native codegen, lower initial implementation complexity
than standing up LLVM integration from day one. LLVM is added later as
an optional high-optimization backend for release builds. A WASM
backend is added at M6 (ROADMAP.md §6, ADR-014) for the web target;
NIR's mode-tagged design (NIR.md §2) was deliberately kept
backend-agnostic so this is a new lowering target, not a NIR redesign:

```text
NIR
 ├── Cranelift → native/debug builds (default)
 ├── LLVM      → optional optimized/release builds (later milestone)
 └── WASM      → web target (M6)
```

## 6. Diagnostics

All compiler stages **MUST** be able to emit structured diagnostics
(not just human-readable text) consumable by `noct diagnostics --json`
(AI_TOOLING.md). This is a pipeline-wide requirement, not bolted on
after the fact, because retrofitting structured diagnostics onto a
text-first diagnostic system is historically expensive.
