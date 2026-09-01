# ROADMAP.md — Milestones & Comparative Notes

**Status:** Milestone sequencing Confirmed in shape; dates/effort
estimates intentionally omitted (v0.1 is pre-implementation).

## 1. Core Milestones

### M0 — Core Language + Interpreter
- Lexer, parser, AST
- Basic type checker
- Tree-walking interpreter
- Functions, structs, enums, traits, generics, modules
- `Result`, `Option`, basic collections, basic I/O
- `noct run`, `noct test`
- Machine-readable diagnostics, basic `noct ast --json`

**Explicitly excluded from M0:** UI, package registry, native
compilation, LLVM, Cranelift, async, ARC, full managed runtime, macros,
advanced sandboxing. This is a deliberately small MVP surface — see
design brief §35; do not silently grow it.

### M1 — NIR + Bytecode VM
Introduces NIR (NIR.md) and a bytecode VM as a faster-iterating
intermediate step before native codegen. This is also where async
lowering (CONCURRENCY.md §7) and the colon-block disambiguation
algorithm (COMPILER_ARCHITECTURE.md §4) get adversarial-test validation.

### M2 — Native Compilation via Cranelift
Native code generation lands (ADR-003). LLVM remains a later,
optional, optimized-build backend, not required for M2.

### M3 — Standard Library + Package Manager + Testing + Formatter + LSP
The production toolchain surface: `stdlib`, `noct` package manager
(manifest format finalized here — TOOLCHAIN.md §3), `noct test`,
`noct fmt`, and the LSP (STYLE_GUIDE.md §6.4 editor integrations begin
here).

**M0–M3 is the core-language MVP, not the whole plan.** It proves the
language works and is usable for non-networked, non-UI programs. It is
deliberately *not* where "build a standard or enterprise-grade app"
becomes true — that's M4/M5 below, plus UI reaching production status.
Do not read M3 as the finish line.

### M4 — Networked & Data-Backed Applications ("standard app" tier)
Everything a normal server-side or client-backend application needs
that a bare language + stdlib collections doesn't provide: production
async I/O, an HTTP client/server, TLS, JSON (de)serialization, database
connectivity with connection pooling, structured logging, and
config/env management. Full detail and exit criteria:
IMPLEMENTATION_PLAN.md Phase 5. This is the tier at which "write a
CRUD API backed by a real database" becomes a fair thing to ask
Noctivue to do.

### M5 — Production & Enterprise Hardening
What separates "an app that works" from "an app a company can run":
observability (metrics, tracing, log correlation), a first cut of
capability-based security and enforced package signing/auditing,
resilience patterns (retries, timeouts, circuit breakers, graceful
shutdown), deployment/packaging conventions, integration/mocking/
property-based testing, CI reference templates, and a formal
SemVer/LTS stability policy for the language and stdlib themselves
(enterprises adopting a language want a stability guarantee, not just
a working compiler). Full detail and exit criteria:
IMPLEMENTATION_PLAN.md Phase 6.

## 2. UI Track (parallel, gated — see UI_SPEC.md §4)

```text
UI-S0  ARC runtime prototype        — can start once Phase 1/M0 lands
UI-S1  Widget-tree prototype        — after UI-S0 validates ARC viability
UI-M1  Promotion to production      — only if all UI_SPEC.md §4 gate
                                       criteria pass
UI-M2  Enterprise/production UI     — accessibility, theming, app-scale
                                       state management, and per-platform
                                       packaging (desktop/mobile/web);
                                       targeted alongside M4/M5
```

This track runs alongside M0–M3 and does not block them. UI-M1's gate
criteria (UI_SPEC.md §4) are unchanged by this amendment — promotion is
still conditional on the research actually working, not on a calendar.
What changes: reaching "standard/enterprise-grade full-stack app"
capability is a stated goal of the M4/M5 window, not an open-ended
maybe. If UI-S0/UI-S1 fail their gates on that timeline, the fallback
is FFI-wrapped native UI toolkits (FFI.md) rather than blocking
full-stack delivery indefinitely on the in-house widget tree — the
"one language, two modes" design (ADR-001) means native-mode Noctivue
with an externally-wrapped UI layer remains a valid enterprise path
even if UI-M1/UI-M2 slip or don't pan out as designed.

## 3. Explicit Non-Requirements for MVP (M0–M3 only)

Sophisticated capability-based sandboxing (TOOLCHAIN.md §7), a finalized
macro/metaprogramming system, and a self-hosting compiler are all
out of scope for M0–M3, by design (Principle 9 — don't grow the core
prematurely). This is scoped to the core-language MVP specifically:
a first cut of capability sandboxing and enforced package
signing/auditing are picked back up in M5 (§1 above) once there's an
ecosystem worth securing — they are deferred, not abandoned. A general
macro system and self-hosting compiler remain open-ended beyond M5;
M4's serialization story (IMPLEMENTATION_PLAN.md Phase 5) uses a
narrowly-scoped derive mechanism rather than reopening general macros,
and that scoping decision should be recorded as its own ADR when M4
begins.

## 4. Comparative Notes

These comparisons are **lessons to draw from**, not "Noctivue beats X"
claims — Noctivue does not exist yet as a working implementation.

| Language | What Noctivue draws from it | What Noctivue deliberately avoids |
|---|---|---|
| **Rust** | Ownership/borrowing model, memory safety, Cargo's "one tool" ergonomics | Cloning Rust's lifetime *syntax* verbatim; steep learning curve as an unavoidable cost |
| **Swift** | ARC as a viable managed-memory strategy; approachable optional-type ergonomics | — |
| **Dart / Flutter** | Declarative widget-tree UI model | Requiring a second, UI-specific language/DSL layered on top |
| **Python** | Breadth via ecosystem/stdlib rather than core-language bloat; approachability | Fragmented packaging ecosystem (Noctivue mandates one package manager) |
| **Kotlin** | Pragmatic type-system choices, null-safety-by-default philosophy | — |
| **Go** | Simplicity as a first-class value; fast compilation as a goal | Overly minimal generics/error-handling ergonomics |
| **Zig** | Comptime-adjacent thinking around explicitness; no hidden control flow | — |
| **C++** | (cautionary) what unconstrained feature accretion costs long-term | Multiple overlapping ways to do the same low-level thing |
| **TypeScript** | Gradual, inference-friendly static typing ergonomics | — |

## 5. Versioning

This documentation set is **Noctivue Language Specification v0.1**.
Everything in it is experimental. Future changes to Confirmed decisions
are recorded as ADR amendments (DECISIONS.md §2), never silent rewrites.

## 6. Domain & Platform Coverage (ADR-014)

§4's comparative table is aspirational unless the plan actually reaches
each domain those languages own today. This maps domain → what covers
it → where. "Covered" means a milestone exists with exit criteria, not
that it's implemented yet.

| Domain (owned today by) | Noctivue's answer | Where |
|---|---|---|
| Systems/embedded (C++, Rust) | Native mode: ownership/borrowing, zero-cost-ish abstractions, C ABI FFI | M0–M2 (ADR-001), FFI.md §1–8 |
| Freestanding/embedded targets, no managed runtime | Freestanding native profile (no ARC, minimal/no heap assumptions) | M6, Phase 7 below |
| Existing C++ codebase interop | Scoped, gated interop tier distinct from C ABI (no stable C++ ABI to target directly) | M6, Phase 7 below; FFI.md new §9 |
| Rapid scripting/glue (Python) | `noct run` on a single file already works from M0; M6 adds a real REPL and script-mode ergonomics (no mandatory ceremony for one-off scripts) | M0 (existing), M6 |
| Enterprise backends at scale (Java/Kotlin-adjacent) | Production hardening (M5) + workspace/multi-package project support | M5, M6 |
| UI/mobile (Flutter) | Widget-tree model, promoted to production and per-platform (desktop/mobile) packaging | ADR-005, UI-M1/UI-M2 |
| Web (TypeScript/JS) | WASM compilation target as a third NIR backend alongside Cranelift/LLVM; UI-M2's "web" packaging target targets it | M6, Phase 7 below; COMPILER_ARCHITECTURE.md §5 |
| Ecosystem breadth (PyPI/npm/Maven/crates.io) | Tiered package interop — native/C-shim/C++-shim carry full trust; WASM-component interop reuses the M6 backend; a foreign-runtime bridge tier covers what's left, at explicitly lower trust and different cost, not translation into `.nv` | M7, ADR-015, IMPLEMENTATION_PLAN.md Phase 8 |

### M6 — Multi-Target Compilation & Interop Expansion
Closes the remaining gaps between "Noctivue can build enterprise
backends and UI apps" (M4/M5, UI-M1/UI-M2) and "Noctivue is a credible
one-language alternative to C++/Python/Rust/Java/JS across the domains
those languages own." Concretely: a WASM backend (web target, and a
path to the same widget-tree UI running in a browser), a scoped C++
interop tier (separate from the Confirmed C-ABI FFI tier, since C++
has no stable ABI to target), a real REPL/script-mode for Python-style
rapid iteration, a freestanding/no-managed-runtime native profile for
embedded targets, and multi-package workspace support for large
codebases. Full detail: IMPLEMENTATION_PLAN.md Phase 7.

### M7 — Cross-Ecosystem Package Interop (ADR-015)
M0–M6 make Noctivue *capable* across systems/backend/UI/web/scripting
domains, but every package in those domains still has to be written in
`.nv` (or be a C/C++ library via FFI). That leaves the actual
day-one advantage of Python/JS/Java untouched: decades of existing
libraries on PyPI/npm/Maven/crates.io. M7 adds a five-tier package
interop system — native `.nv`, C-shim, C++-shim, WASM-component, and
an explicitly lower-trust foreign-runtime bridge tier — rather than one
"import anything" feature, because those tiers do not carry the same
fidelity, performance, or trust guarantees, and conflating them would
overstate what's actually delivered. Full detail:
IMPLEMENTATION_PLAN.md Phase 8, TOOLCHAIN.md §3.
