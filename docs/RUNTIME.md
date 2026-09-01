# RUNTIME.md — Native & Managed Runtime

**Status:** Split Confirmed; managed runtime facilities Experimental
(gated by UI-S0/UI-S1, see ROADMAP.md).

## 1. Two Runtimes, Shared Foundation

```text
Shared:  language syntax, type system, compiler frontend,
         common IR concepts (NIR mode tags), tooling

Native Runtime          Managed Runtime
- allocation             - ARC (retain/release)
- ownership enforcement  - managed allocation
  (compile-time, mostly  - UI/application runtime facilities
   zero runtime cost)    - managed task execution / event loop
- I/O
- concurrency scheduler
- platform interaction
- low-level facilities
```

## 2. Native Runtime

Minimal by design: most of native mode's guarantees are enforced at
compile time (ownership/borrow checking), so the runtime component is
mainly allocation, I/O, the concurrency scheduler (CONCURRENCY.md), and
platform/OS interaction — not a large managed layer.

## 3. Managed Runtime

Provides ARC bookkeeping, managed heap allocation, and — once past the
UI-S0/UI-S1 research gate (ROADMAP.md, UI_SPEC.md §4) — the UI
event loop and widget-tree execution facilities. Until that gate is
passed, the managed runtime is prototype-grade and not a supported
production target.

## 4. Interop at the Runtime Boundary

Consistent with MEMORY_MODEL.md §7, the native and managed runtimes do
not implicitly share live object graphs; values cross the boundary
through explicit conversion, mirroring the FFI boundary discipline
(FFI.md §6). This keeps each runtime's guarantees intact independently.

## 5. Startup & Entry Point

A Noctivue binary's entry point is an ordinary top-level function
(convention: `main`); whether `main` is native-mode-only, or may be
managed-mode for UI-first applications, is **Open** pending the UI
research track's outcome.
