# SCAFFOLD.md — Repository Scaffold

**Status:** Proposed. This expands ARCHITECTURE.md §10 into a concrete,
buildable starting layout. Per design brief §31, this scaffold
deliberately does **not** create empty UI/resource/module project
conventions until they have concrete implementation value — several
directories below are marked accordingly and should not be created
until the phase that needs them (see IMPLEMENTATION_PLAN.md).

## 1. Principle

Scaffold only what the current implementation phase needs. An empty
`runtime-managed/` or `lsp/` directory sitting untouched for months is
noise, not architecture — it invites half-started code and misleads
contributors about what's actually functional. Each block below is
tagged with the phase that creates it.

## 2. Top-Level Layout

```text
noctivue/
├── Cargo.toml                  # workspace root
├── README.md
├── LICENSE
├── docs/                       # this documentation set
│   ├── ARCHITECTURE.md
│   ├── LANGUAGE_SPEC.md
│   ├── SYNTAX.md
│   ├── TYPE_SYSTEM.md
│   ├── MEMORY_MODEL.md
│   ├── ERROR_HANDLING.md
│   ├── CONCURRENCY.md
│   ├── UI_SPEC.md
│   ├── MODULES.md
│   ├── FFI.md
│   ├── NIR.md
│   ├── COMPILER_ARCHITECTURE.md
│   ├── RUNTIME.md
│   ├── TOOLCHAIN.md
│   ├── AI_TOOLING.md
│   ├── STYLE_GUIDE.md
│   ├── ROADMAP.md
│   ├── DECISIONS.md
│   ├── SCAFFOLD.md             # this file
│   └── IMPLEMENTATION_PLAN.md
│
├── compiler/                   # [Phase 0/1] Rust workspace member
│   ├── Cargo.toml
│   └── src/
│       ├── lexer/
│       │   ├── mod.rs
│       │   ├── token.rs
│       │   └── indent.rs       # offside-rule INDENT/DEDENT synthesis
│       ├── parser/
│       │   ├── mod.rs
│       │   └── grammar.rs      # hand-written recursive-descent, per SYNTAX.md §10
│       ├── ast/
│       │   └── mod.rs          # bare_decl and all node types
│       ├── resolver/
│       │   ├── mod.rs
│       │   └── classify.rs     # struct/function/component classification, COMPILER_ARCHITECTURE.md §4
│       ├── typeck/
│       │   └── mod.rs          # [Phase 1] local inference
│       ├── hir/
│       │   └── mod.rs          # [Phase 1]
│       ├── nir/                # [Phase 2] not created until M1
│       │   └── mod.rs
│       ├── diagnostics/
│       │   └── mod.rs          # structured diagnostics, shared by all stages
│       └── backends/           # [Phase 3] not created until M2
│           ├── cranelift/
│           └── llvm/           # [Phase 4+, optional]
│
├── interp/                     # [Phase 1] tree-walking interpreter, separate
│   ├── Cargo.toml               # crate from compiler/ so M0 stays runnable
│   └── src/mod.rs               # standalone while NIR doesn't exist yet
│
├── runtime-native/              # [Phase 3] not created until M2
├── runtime-managed/             # [UI research track] created only for UI-S0
│
├── stdlib/                      # [Phase 4] .nv source, not Rust
│   └── (empty until M3)
│
├── noct-cli/                    # [Phase 1] the `noct` binary crate
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── cmd_run.rs           # [Phase 1]
│       ├── cmd_test.rs          # [Phase 1]
│       ├── cmd_ast.rs           # [Phase 1] `noct ast --json`
│       ├── cmd_diagnostics.rs   # [Phase 1] `noct diagnostics --json`
│       ├── cmd_build.rs         # [Phase 3]
│       ├── cmd_fmt.rs           # [Phase 4]
│       ├── cmd_lint.rs          # [Phase 4]
│       ├── cmd_add.rs           # [Phase 4] package manager
│       ├── cmd_publish.rs       # [Phase 4, later]
│       └── cmd_doc.rs           # [Phase 4]
│
├── lsp/                         # [Phase 4] not created until M3
│
├── tests/
│   ├── fixtures/                # .nv source files used as golden tests
│   │   ├── structs/
│   │   ├── functions/
│   │   ├── enums_match/
│   │   ├── result_option/
│   │   ├── ambiguous_decls/     # Phase 0 adversarial disambiguation cases
│   │   └── ui_components/       # [UI research track only]
│   └── golden/                  # expected AST/diagnostics JSON per fixture
│
└── examples/
    └── dashboard.nv              # already exists — the ~100-line worked example
```

## 3. Cargo Workspace Shape (Phase 0/1)

```toml
# Cargo.toml (workspace root)
[workspace]
resolver = "2"
members = [
    "compiler",
    "interp",
    "noct-cli",
]
```

`interp/` is kept as its own crate, separate from `compiler/`, so M0
stays a runnable, demoable artifact (lex → parse → resolve → typecheck →
interpret) without depending on NIR or backends that don't exist yet.
When M1 introduces NIR, `interp/` either gets replaced by a NIR-based VM
crate or kept as a reference implementation for differential testing —
that decision is **Open**, tracked in IMPLEMENTATION_PLAN.md.

## 4. What Gets Created When (cross-reference)

| Directory | Created in | Reason not to create earlier |
|---|---|---|
| `compiler/src/{lexer,parser,ast,resolver}/` | Phase 0/1 | — (needed immediately) |
| `compiler/src/typeck/`, `hir/` | Phase 1 | needs AST+resolver first |
| `interp/` | Phase 1 | M0's actual deliverable |
| `noct-cli/` (run/test/ast/diagnostics only) | Phase 1 | — |
| `compiler/src/nir/` | Phase 2 | NIR.md is explicitly not frozen until M1 begins (design brief §19) |
| `compiler/src/backends/cranelift/` | Phase 3 | native codegen isn't a Phase 1/2 goal |
| `runtime-native/` | Phase 3 | native runtime is minimal until ownership enforcement is real (M2) |
| `runtime-managed/` | UI-S0 (parallel track) | gated research spike, not a committed feature |
| `stdlib/` | Phase 4 | needs a stable-enough language to write library code against |
| `lsp/` | Phase 4 | needs a stable-enough compiler API surface to bind to |
| `compiler/src/backends/llvm/` | Phase 4+, optional | ADR-003: Cranelift first, LLVM later, optional |

## 5. Fixture/Golden Test Convention (Phase 0 onward)

Every grammar feature and every disambiguation edge case
(DECISIONS.md Issue 1/2) should land as a `.nv` file under
`tests/fixtures/<category>/` with a matching `tests/golden/<category>/*.json`
capturing the expected `noct ast --json` and/or `noct diagnostics --json`
output. This exists from Phase 0 specifically because the disambiguation
algorithm is the highest-risk part of the design — regressions there
should fail a test, not surface as a confusing runtime bug three phases
later.

## 6. Naming

Crate names use the `noct-` prefix for anything published independently
(e.g., `noct-lexer`, `noct-parser`) if the compiler is later split into
publishable sub-crates; internal-only workspace members (as scaffolded
above) don't need the prefix. This is a **Proposed** convention, not
binding until the first crate actually needs to be published separately.
