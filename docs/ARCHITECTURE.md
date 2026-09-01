# ARCHITECTURE.md — Noctivue Platform Architecture Overview

**Noctivue Language Specification v0.1 — Experimental / Pre-Implementation**

This document is the entry point into the Noctivue documentation set. It
summarizes the platform at a high level and links out to the detailed specs.
Every claim here is elaborated, with rationale, in the linked documents.
Status tags (`Confirmed`, `Proposed`, `Experimental`, `Deferred`, `Open`) are
used throughout this documentation set as defined in [DECISIONS.md](./DECISIONS.md).

## 1. What Noctivue Is

Noctivue is a statically typed, memory-safe programming language and
platform intended to span systems programming, backend/server software,
CLI tooling, scripting, and UI/application development from a single
source language. It is inspired by Rust (safety, performance), Flutter
(declarative UI, developer experience), and Python (breadth,
accessibility) — without attempting to be a clone of any of them.
See [DECISIONS.md](./DECISIONS.md) for the full rationale and
[COMPARATIVE_NOTES](./ROADMAP.md#comparative-notes) for what is and isn't
borrowed from each.

**Status: Confirmed** — project identity, name, `.nv` extension, ambition
scope. **Status: Experimental** — the language does not exist yet; nothing
below should be read as a stability promise.

## 2. Core Identity

- **Name:** Noctivue
- **Source extension:** `.nv` (single developer-facing source extension — ADR-006)
- **Visual identity:** colon-first declaration syntax (ADR-007) — see [SYNTAX.md](./SYNTAX.md)
- **Guiding principle:** *Structure should communicate structure; keywords
  should communicate behavior.* Indentation-**friendly**, not
  indentation-**dependent** (braces are always a legal alternative).

## 3. The Ten Design Principles

1. **One language** — no separate UI/scripting/systems dialects.
2. **Multiple execution modes** — native ownership and managed ARC share one language.
3. **Colon-first identity** — the defining visual signature.
4. **Explicitness for experts** — `fn`, `struct`, `enum`, `trait` remain available and equivalent.
5. **Compression without a second language** — compact forms are the same AST, not a different grammar.
6. **Complexity determines verbosity** — small things stay small; large things get room.
7. **Static safety** — compile-time guarantees preferred over runtime checks.
8. **Practical ergonomics** — don't import a hard feature just because a systems language has it.
9. **Ecosystem over language bloat** — packages and FFI over core-language growth.
10. **Tooling is part of the language** — compiler, formatter, LSP, package manager, and AI interfaces are first-class.

Full discussion: [STYLE_GUIDE.md](./STYLE_GUIDE.md) §1.

## 4. System Map

```text
                         ┌─────────────────────┐
                         │   .nv source files   │
                         └──────────┬───────────┘
                                    │
                         ┌──────────▼───────────┐
                         │   Compiler Frontend   │  (Rust, ADR-002)
                         │ Lexer→Parser→AST→     │
                         │ Resolve→Typeck→HIR    │
                         └──────────┬───────────┘
                                    │
                            ┌───────▼────────┐
                            │      NIR        │  SSA, typed, mode-aware
                            └───┬─────────┬───┘
                    ┌───────────┘         └───────────┐
           ┌────────▼────────┐             ┌──────────▼─────────┐
           │  Native Backend  │             │  Managed Backend    │
           │ Cranelift→LLVM   │             │  ARC / VM (research) │
           │  (ADR-003)       │             │  (ADR-004)          │
           └────────┬────────┘             └──────────┬──────────┘
                     │                                 │
           ┌─────────▼─────────┐             ┌─────────▼──────────┐
           │  Native Runtime    │             │  Managed Runtime    │
           │  ownership, I/O,   │             │  ARC, UI runtime,   │
           │  concurrency       │             │  managed tasks      │
           └───────────────────┘             └─────────────────────┘
```

Shared across both runtimes: language syntax, type system, compiler
frontend, common IR concepts, and tooling (§9). Details:
[COMPILER_ARCHITECTURE.md](./COMPILER_ARCHITECTURE.md),
[NIR.md](./NIR.md), [RUNTIME.md](./RUNTIME.md).

## 5. Two Memory Modes, One Language (ADR-001)

| | Native mode | Managed mode |
|---|---|---|
| Strategy | Ownership + borrowing (simplified vs. Rust) | ARC |
| Target use | systems, embedded, CLI, servers, hardware | UI, app scripting, plugins |
| GC pause | none | none (ARC has no tracing pause, but has cycle risk) |
| Opt-in | default | explicit (`managed` — syntax **Proposed**, see MEMORY_MODEL.md) |

Full detail, including the cycle-mitigation strategy: [MEMORY_MODEL.md](./MEMORY_MODEL.md).

## 6. Surface Syntax at a Glance

```nv
User:
    id: Int
    name: String

calculate(x: Int) -> Int:
    x * 2

Dashboard:
    column:
        text("Analytics")
        button("Refresh"): refresh()
```

Three density levels (minimal / compact / expanded) represent the same
AST. Full grammar: [SYNTAX.md](./SYNTAX.md).

## 7. Type System, Errors, Concurrency (summary)

- Static typing, local inference, no `null` — `Option<T>` instead.
- `Result<T, E>` with `?` for recoverable errors; panics only for invariant
  violations. See [ERROR_HANDLING.md](./ERROR_HANDLING.md).
- Structured concurrency (`async`/`await`/`task`), one official runtime.
  See [CONCURRENCY.md](./CONCURRENCY.md).
- Type system detail: [TYPE_SYSTEM.md](./TYPE_SYSTEM.md).

## 8. UI Architecture (Research Track)

Flutter-style widget tree, built as an ordinary package on public
compiler/language APIs — not privileged compiler magic. UI state
management and ARC-cycle mitigation are **Open** design problems tracked
through a gated research spike (UI-S0 → UI-S1 → UI-M1). See
[UI_SPEC.md](./UI_SPEC.md).

## 9. Toolchain

One official package manager (`noct`), one official formatter, one LSP,
AI-facing machine-readable interfaces (`--json` flags) from day one. See
[TOOLCHAIN.md](./TOOLCHAIN.md) and [AI_TOOLING.md](./AI_TOOLING.md).

## 10. Repository Layout (Proposed)

```text
noctivue/
├── compiler/
│   ├── lexer/ parser/ ast/ resolver/ typeck/ hir/ nir/
│   └── backends/{cranelift,llvm}/
├── runtime-native/
├── runtime-managed/
├── stdlib/
├── lsp/
├── noct-cli/
├── docs/
└── examples/
```

No UI/resource/module project scaffolding is created until it has concrete
implementation value (§31 of the source brief).

## 11. Milestones (summary — full detail in ROADMAP.md)

`M0` tree-walking interpreter core → `M1` NIR + bytecode VM → `M2` native
compilation via Cranelift → `M3` stdlib, package manager, testing,
formatter, LSP. UI/managed work proceeds as a parallel research spike and
is **not** part of the MVP gate.

## 12. Document Index

| Doc | Covers |
|---|---|
| [LANGUAGE_SPEC.md](./LANGUAGE_SPEC.md) | Lexical foundation, keywords, normative core |
| [SYNTAX.md](./SYNTAX.md) | Full grammar (EBNF), density levels, formal parsing rules |
| [TYPE_SYSTEM.md](./TYPE_SYSTEM.md) | Types, generics, traits, inference |
| [MEMORY_MODEL.md](./MEMORY_MODEL.md) | Native ownership + managed ARC, cycle mitigation |
| [ERROR_HANDLING.md](./ERROR_HANDLING.md) | Result/Option, `?`, panics |
| [CONCURRENCY.md](./CONCURRENCY.md) | Structured concurrency, tasks, async lowering |
| [UI_SPEC.md](./UI_SPEC.md) | Widget tree, UI state, research gates |
| [MODULES.md](./MODULES.md) | Imports/exports, project/module structure |
| [FFI.md](./FFI.md) | C ABI, `unsafe`, ownership boundaries |
| [NIR.md](./NIR.md) | IR structure |
| [COMPILER_ARCHITECTURE.md](./COMPILER_ARCHITECTURE.md) | Pipeline, parsing strategy for colon-first syntax |
| [RUNTIME.md](./RUNTIME.md) | Native + managed runtime facilities |
| [TOOLCHAIN.md](./TOOLCHAIN.md) | `noct` CLI, formatter, LSP, package manager |
| [AI_TOOLING.md](./AI_TOOLING.md) | Machine-readable compiler interfaces |
| [STYLE_GUIDE.md](./STYLE_GUIDE.md) | Naming, formatting conventions, file identity/branding |
| [ROADMAP.md](./ROADMAP.md) | M0–M3, UI-S0–UI-M1, comparative notes |
| [DECISIONS.md](./DECISIONS.md) | ADR log, status taxonomy, consistency review |
| `examples/dashboard.nv` | ~100-line worked UI example |

---
*This is Noctivue Language Specification v0.1. The language is
experimental and unimplemented. Nothing here is a stability guarantee.*
