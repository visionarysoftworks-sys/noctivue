# Noctivue Language Specification v0.1

Experimental, pre-implementation. See `docs/DECISIONS.md` for the
status taxonomy (Confirmed / Proposed / Experimental / Deferred / Open)
used throughout this set.

**Scope tiers (ROADMAP.md, IMPLEMENTATION_PLAN.md):** M0–M3 is the
core-language MVP (non-UI programs only). M4 adds what a *standard*
app needs (networking, a database, logging). M5 adds what an
*enterprise-grade* app needs (observability, security auditing,
resilience, deployment/CI, a stability policy). UI reaches
production/enterprise maturity via UI-M1/UI-M2, targeted alongside
M4/M5, with an FFI-wrapped-toolkit fallback if the in-house widget-tree
research doesn't land on that schedule. M6 is where the "one language
instead of C++/Python/Rust/Flutter/Java/JS" claim (ROADMAP.md §6) gets
checkable exit criteria: a WASM/web backend, script/REPL mode, a
freestanding embedded profile, and scoped C++ interop. M7 is where
existing-ecosystem *library* breadth (PyPI/npm/Maven/crates.io) gets
addressed, via an explicitly tiered trust system rather than a single
"import anything" claim. See DECISIONS.md ADR-013 (M4/M5), ADR-014
(M6), and ADR-015 (M7) for the amendments.

## Start here

- `docs/ARCHITECTURE.md` — high-level overview and document index
- `docs/EXECUTION_GUIDE.md` — how to actually build this, phase by phase
- `docs/IMPLEMENTATION_PLAN.md` — the phased build order and exit criteria
- `docs/SCAFFOLD.md` — the concrete repo layout to start from

## Full document list

```
docs/
├── ARCHITECTURE.md           high-level overview + doc index
├── LANGUAGE_SPEC.md          lexical foundation, keywords
├── SYNTAX.md                 full grammar (EBNF), density levels
├── TYPE_SYSTEM.md            types, generics, traits, inference
├── MEMORY_MODEL.md           native ownership + managed ARC
├── ERROR_HANDLING.md         Result/Option, ?, panics
├── CONCURRENCY.md            structured concurrency
├── UI_SPEC.md                widget tree, UI state, research gates
├── MODULES.md                imports/exports
├── FFI.md                    C ABI, unsafe
├── NIR.md                    intermediate representation
├── COMPILER_ARCHITECTURE.md  pipeline, colon-first parsing strategy
├── RUNTIME.md                native + managed runtime facilities
├── TOOLCHAIN.md              noct CLI, formatter, LSP, package manager
├── AI_TOOLING.md             machine-readable compiler interfaces
├── STYLE_GUIDE.md            naming conventions, file identity/branding
├── ROADMAP.md                milestones, comparative notes
├── DECISIONS.md              ADR log, status taxonomy, consistency review
├── SCAFFOLD.md               concrete starter repo layout
├── IMPLEMENTATION_PLAN.md    phased build order + exit criteria
└── EXECUTION_GUIDE.md        care/precision guidance per phase, mega doc

examples/
└── dashboard.nv               ~100-line worked UI example
```

## Reading order for someone about to start building

1. `docs/ARCHITECTURE.md` — orient yourself
2. `docs/LANGUAGE_SPEC.md` + `docs/SYNTAX.md` — the language itself
3. `docs/DECISIONS.md` — what's actually settled vs. open, and why
4. `docs/SCAFFOLD.md` — the repo layout
5. `docs/IMPLEMENTATION_PLAN.md` — what order to build in
6. `docs/EXECUTION_GUIDE.md` — how to build each phase without the
   mistakes this design is most exposed to
