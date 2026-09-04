# IMPLEMENTATION_PLAN.md — Implementation Plan

**Status:** Proposed sequencing. This is the actionable build order
referenced from ROADMAP.md's milestone *definitions* — ROADMAP.md says
*what* M0–M3 and the UI track contain; this document says *in what
order, with what exit criteria, starting from nothing*.

## 1. Sequencing Principle

Each phase below must produce something runnable/testable before the
next phase begins. No phase should be started because it's
"architecturally next" if the previous phase's exit criteria aren't
met — this plan exists specifically to stop scope creep back into
excluded M0 features (async, ARC, native codegen, macros — ROADMAP.md
§1) before the foundation is solid.

## 2. Phase 0 — De-risk the Core Syntax Question

**Goal:** validate the colon-block disambiguation algorithm
(COMPILER_ARCHITECTURE.md §4) before committing the parser to it.

**Work:**
- Write the adversarial test cases already flagged in DECISIONS.md
  Issue 1/2 as `.nv` fixtures (`tests/fixtures/ambiguous_decls/`),
  including the `Splash: loading_screen()`-style cycle case.
- Hand-simulate (or throwaway-prototype) the parser-produces-`bare_decl`
  → resolver-classifies-by-shape algorithm against every fixture.
- Confirm the "declaration classification cycle" compile-error path
  (COMPILER_ARCHITECTURE.md §4) is actually reachable and reasonable,
  not just theoretical.

**Exit criteria:** every fixture in `ambiguous_decls/` either
classifies unambiguously or produces the defined cycle error — no case
falls through to undefined behavior. If a fixture reveals the algorithm
doesn't work, that's a DECISIONS.md amendment *now*, not a Phase 1 bug.

**Does not require:** any actual lexer/parser code — this can be done
as a design exercise or a minimal standalone prototype.

## 3. Phase 1 — M0: Core Language + Interpreter

Matches ROADMAP.md's M0 definition exactly; this section is the
internal build order within it.

**Work, in order (SCAFFOLD.md §2/§4 for directory mapping):**

1. **Lexer** (`compiler/src/lexer/`) — UTF-8, tokens, INDENT/DEDENT
   offside-rule pass (SYNTAX.md §10.1). Test against files with mixed
   valid/invalid indentation to confirm the "hard compiler error, never
   a guess" rule (LANGUAGE_SPEC.md §2) actually holds.
2. **Parser** (`compiler/src/parser/`) — the EBNF in SYNTAX.md §10,
   producing `bare_decl` nodes per the Phase 0 result. Keep it strictly
   context-free; if you find yourself wanting to peek at identifier
   casing to resolve a grammar decision, that's a signal Phase 0's
   algorithm needs revisiting, not a shortcut to take.
3. **AST + Name Resolution** (`compiler/src/ast/`, `resolver/`) —
   struct/function/component classification lands here
   (`resolver/classify.rs`).
4. **Basic type checker** (`compiler/src/typeck/`) — local inference
   only (TYPE_SYSTEM.md §13); function signatures fully annotated,
   no whole-program inference.
5. **Tree-walking interpreter** (`interp/`) — operates on typed
   AST/HIR directly; no NIR yet.
6. **CLI wiring** (`noct-cli/`) — `noct run`, `noct test`.
7. **Structured diagnostics + AST dump** — `noct diagnostics --json`,
   `noct ast --json` (AI_TOOLING.md §2), built alongside each stage
   above, not retrofitted (COMPILER_ARCHITECTURE.md §6).

**Exit criteria:**
- `examples/dashboard.nv`'s non-UI subset (structs, functions, `enum`,
  `match`, `Result`/`Option`, `?`) lexes, parses, resolves, type-checks,
  and interprets correctly.
- `tests/fixtures/` covers structs, functions, enums+match,
  result/option, and the Phase 0 ambiguous-declaration cases, each with
  a golden AST/diagnostics JSON.
- `noct ast --json` and `noct diagnostics --json` are stable enough that
  a second tool (even a throwaway script) can consume them without
  parsing human-readable text.

**Explicitly excluded** (ROADMAP.md §1): UI, package registry, native
compilation, LLVM, Cranelift, async, ARC, full managed runtime, macros,
advanced sandboxing.

## 4. Phase 2 — M1: NIR + Bytecode VM

**Preconditions:** Phase 1 exit criteria met.

**Work:**
1. Design NIR's concrete instruction set (NIR.md §4) — this is where
   NIR.md stops being deliberately underspecified and gets frozen
   enough to implement against.
2. Lower typed AST/HIR → NIR.
3. Build a bytecode VM consuming NIR (faster iteration than jumping
   straight to Cranelift).
4. Validate the single-IR, mode-tagged approach (NIR.md §2,
   DECISIONS.md Issue 6) — confirm native vs. managed lowering rules
   diverge only in *lowering*, not in instruction shape, before this is
   load-bearing for the rest of the compiler.
5. Begin async lowering design (CONCURRENCY.md §7) against real NIR,
   now that SSA exists to lower into.
6. Decide `interp/`'s fate (SCAFFOLD.md §3, Open) — retire it in favor
   of the NIR-based VM, or keep it as a differential-testing oracle.

**Exit criteria:**
- Same Phase 1 test fixtures pass through NIR + VM with identical
  observable behavior to the Phase 1 interpreter (differential test).
- Adversarial disambiguation fixtures from Phase 0 still hold under
  NIR lowering (no new ambiguity introduced by mode-tagging).

**Completion record (M1):** met, with receipts.
- Differential coverage: `noct-cli/tests/differential.rs` (22 CLI
  cases — byte-identical stdout, exit codes, stderr over `run` vs
  `run-vm`, incl. print-ordering, `??` short-circuit observability,
  `?`/`Err` propagation, match dispatch, loops, fixtures) and
  `compiler/src/nir/vm_tests.rs` (10 unit cases incl. the single-IR
  mode-tag shape test). All `#[ignore]`d repros resolved and un-ignored.
- `interp/` fate: **kept as differential-testing oracle** (see
  `interp/src/lib.rs` header) — closes the Open item, retires nothing.
- Instruction set frozen in NIR.md §4 (with the §4.1 lowering
  discipline and §5 async-validation outcome recorded there).
- Known boundaries carried forward, not hidden: runtime-failure stderr
  *text* parity (HIR has no spans for the VM to cite — needs unified
  runtime diagnostics); builtins beyond `print` unrepresentable (loud
  `UNRESOLVED`, never silent); nested-variant/literal match
  subpatterns rejected loudly; `break`/`continue` lowered (E0205 outside
  loops, value-discard warning; labeled breaks still open);
  match-guard `Bool` unchecked by typeck (all in NIR.md §6).

## 5. Phase 3 — M2: Native Compilation (Cranelift)

**Preconditions:** Phase 2 exit criteria met.

**Work:**
1. Real ownership/borrow enforcement (MEMORY_MODEL.md §2) — this is
   where "mostly compile-time, minimal runtime cost" native mode
   actually needs to exist, not just be type-checked loosely as in M0.
2. NIR → Cranelift lowering (`compiler/src/backends/cranelift/`).
3. `runtime-native/` — allocation, I/O, concurrency scheduler
   (RUNTIME.md §2).
4. `noct build` in `noct-cli/`.

**Exit criteria:**
- `examples/dashboard.nv`'s non-UI subset compiles to a native binary
  and produces identical output to the Phase 1/2 interpreter/VM paths.
- A basic FFI round-trip (FFI.md) — call one C stdlib function
  (e.g., `strlen`) — works inside `unsafe`.

## 6. Phase 4 — M3: Stdlib + Package Manager + Testing + Formatter + LSP

**Preconditions:** Phase 3 exit criteria met.

**Work:**
1. `stdlib/` — collections, string/FFI boundary helpers (FFI.md §7),
   error-handling conventions (ERROR_HANDLING.md §6).
2. Package manifest format finalized (TOOLCHAIN.md §3, currently Open)
   and `noct add`/lockfile generation implemented.
3. `noct fmt` — must losslessly round-trip density levels and block
   forms (SYNTAX.md §§3, 6–7); semicolon-placement behavior per
   DECISIONS.md Issue 5.
4. `noct lint` — default-on structural rules; style-preference rules
   (e.g., concise-vs-explicit) ship default-off (DECISIONS.md Issue 7).
5. `lsp/` and the VS Code integration first (STYLE_GUIDE.md §6.4).
6. `noct doc`.

**Exit criteria:**
- A second developer (not the implementer) can `noct create`, write a
  small non-UI program, `noct test`, `noct fmt`, and `noct build` it
  using only the public toolchain — no direct compiler-crate access.
- Package signing/integrity hashing in place *before* any public
  registry launch (TOOLCHAIN.md §3 — this is a hard precondition, not
  a nice-to-have).

## 7. Phase 5 — M4: Networked & Data-Backed Applications

**Preconditions:** Phase 4/M3 exit criteria met — stable stdlib,
package manager, `noct test`/`noct fmt`, and LSP, per a second
developer using only public tooling.

**Work:**
1. **Production async I/O** — harden the async runtime from Phase 2/
   CONCURRENCY.md §7 for real non-blocking sockets and an executor
   suitable for concurrent request handling, not just single-task
   await chains.
2. **`net.http`** — HTTP client and server in stdlib (HTTP/1.1 first;
   HTTP/2 as stretch), plus TLS via an FFI binding to a vetted C
   library (FFI.md §6 boundary discipline applies: the `unsafe`
   surface is isolated and audited, not scattered through user code).
3. **Serialization** — JSON as a first-class stdlib type, with a
   `Serialize`/`Deserialize` derive story. This needs a narrowly-scoped
   derive mechanism; record the scoping decision as its own ADR rather
   than reopening ROADMAP.md's general "macros deferred" stance
   (DECISIONS.md Issue-style entry, amendment process per §2).
4. **Database connectivity** — an FFI-bound driver for at least one
   production database (e.g., a Postgres client library) behind an
   isolated `unsafe` boundary, a thin async query interface, and
   connection pooling. Explicitly not a full ORM at this phase — that's
   ecosystem territory, revisited no earlier than M5.
5. **Config & structured logging** — env var/file-based config loading,
   and a leveled, structured `log` facade with a pluggable sink, both
   in stdlib.

**Exit criteria:**
- A reference application — a CRUD REST API backed by a real database —
  is built end-to-end using only public stdlib and the package manager.
- The reference app handles concurrent requests correctly under a basic
  load test (many concurrent connections) with no crashes, deadlocks,
  or resource leaks.
- Native-mode ownership discipline holds for connection/socket/file
  lifetimes under that load test (verified with a sanitizer or
  equivalent leak-detection pass) — this phase must not quietly
  reintroduce manual-lifetime bugs into what was a compile-time-checked
  guarantee through M0–M3.

## 8. Phase 6 — M5: Production & Enterprise Hardening

**Preconditions:** Phase 5/M4 exit criteria met.

**Work:**
1. **Observability** — metrics (counters/histograms) and distributed
   tracing hooks in stdlib, with correlation IDs threaded through logs;
   export in an OpenTelemetry-compatible format via a blessed package
   rather than mandating a specific vendor in core.
2. **Security hardening** — crypto primitives (hashing, key handling)
   in stdlib alongside M4's TLS; a first cut of capability-based
   sandboxing (TOOLCHAIN.md §7, previously Deferred wholesale — now
   picked up, not necessarily finished); `noct audit` for dependency
   vulnerability scanning; package integrity hashing *and* signing
   (TOOLCHAIN.md §3) fully enforced, not just specified.
3. **Resilience patterns** — retries, timeouts, circuit breakers,
   graceful shutdown/signal handling, and backpressure, as stdlib or
   blessed packages rather than language features (keeps the core
   language minimal, per Principle 9 — these are library concerns).
4. **Deployment** — reproducible cross-compilation targets, static/
   minimal-footprint binaries, official container base-image guidance,
   and health-check/readiness conventions for the reference app.
5. **Testing at scale** — an integration-test harness capable of
   standing up dependent services, mocking/fakes support, a
   property-based testing library, and `noct create --template ci`
   reference CI pipelines.
6. **Stability policy** — a formal SemVer commitment for the language
   and stdlib, plus an LTS/stable-channel policy, recorded as its own
   ADR; this is a governance decision as much as a technical one, and
   is frequently a hard adoption precondition for enterprises.

**Exit criteria:**
- The M4 reference app is deployed through the documented pipeline
  (build → test → containerize → deploy) with metrics and traces
  visible in a standard backend (e.g., an OTel collector plus
  Prometheus/Grafana).
- The app and its dependencies pass `noct audit` cleanly, and package
  signing is enforced end-to-end (not optional) for anything pulled
  from the registry.
- A second team — not the implementers — can onboard onto the
  reference app using only public docs and `noct create` templates,
  mirroring Phase 4's "second developer" bar but for a production
  service rather than a single package.

## 9. Phase 7 — M6: Multi-Target Compilation & Interop Expansion

**Preconditions:** Phase 6/M5 exit criteria met for the backend-app
side; UI-M1 gate criteria (UI_SPEC.md §4) met or the FFI-wrapped-
toolkit fallback in use for the UI side — this phase's WASM/mobile
work assumes *a* UI story exists, not necessarily the in-house one.

**Work:**
1. **WASM backend** — a third NIR lowering target alongside Cranelift
   and LLVM (COMPILER_ARCHITECTURE.md §5); validate that mode-tagging
   (NIR.md §2) holds under a genuinely different target (no native
   pointers/syscalls the way Cranelift assumes) rather than only
   under native-vs-native variation as M2 validated.
2. **Web UI target** — the widget-tree runtime (UI-M1/UI-M2) rendering
   through the WASM backend, so the same Noctivue UI code targets
   desktop/mobile/web rather than needing a separate web framework.
3. **REPL & script mode** — `noct repl` on top of the existing
   interpreter/VM (Phase 1/2 already built these; this is exposing
   them ergonomically, not new execution machinery), plus a script
   mode that runs a single `.nv` file without requiring project
   scaffolding or a `main` boilerplate ceremony, for Python-style
   one-off use.
4. **Freestanding native profile** — a native-mode build profile with
   no managed runtime assumptions and minimal/no heap allocation
   requirement, for embedded targets (MEMORY_MODEL.md's native mode
   already supports this in principle; this phase makes it a
   supported, documented profile rather than an implicit possibility).
5. **C++ interop shim generator** — per FFI.md §9, scoped to plain
   functions and simple (non-template, single-inheritance) classes.
6. **Workspace/multi-package support** — `noct` gains multi-package
   workspace resolution (shared lockfile across packages in one repo),
   for large-codebase ergonomics closer to enterprise monorepo norms.

**Exit criteria:**
- A Noctivue program compiles and runs correctly through all three
  backends (Cranelift, LLVM, WASM) from the same source, with
  differential testing against the Phase 1/2 interpreter/VM oracle
  (mirroring the Phase 2 exit-criteria pattern).
- The M4 reference app's UI (if UI-M1 landed) or a comparable widget
  demo runs in a browser via the WASM backend.
- `noct repl` supports interactive evaluation with the same semantics
  as `noct run`; a script-mode `.nv` file runs with zero project
  scaffolding.
- The FFI.md §9 exit criterion (C++ shim round-trip under a sanitizer)
  is met.
- A multi-package workspace builds, tests, and locks dependencies
  consistently across its member packages via `noct`.

## 10. Phase 8 — M7: Cross-Ecosystem Package Interop

**Preconditions:** Phase 7/M6 exit criteria met — the WASM backend and
C++ shim tier both need to exist before this phase can build on them.

**Work, by tier (ADR-015):**
1. **Native (`.nv`)** — already the baseline from M4 onward; this phase
   only adds the manifest trust-tier field itself (TOOLCHAIN.md §3) so
   every dependency, regardless of tier, declares which one it's in.
2. **C-shim** — extend `noct add` to fetch and build plain C libraries
   and any Rust crate that exports a `cdylib`/`extern "C"` surface
   (whether hand-written or `cbindgen`-generated); no new trust
   handling needed, this reuses FFI.md §1–8 as-is.
3. **C++-shim** — wire `noct add` to the FFI.md §9 shim generator for
   packages that opt into it; Experimental status propagates into
   `noct audit` output so it's visibly distinct from tier 1/2
   dependencies, not silently equivalent.
4. **WASM-component** — resolve and link WIT-typed WASM components as
   dependencies through the M6 WASM backend; no embedded foreign
   runtime, so this tier's trust handling matches tiers 1–2 once the
   component's interface is verified against its WIT signature.
5. **Foreign-runtime bridge** — embed a JVM/CPython/JS runtime behind a
   marshaling boundary for ecosystems with no C-shaped or
   WASM-component export. Per dependency, this requires: (a) an
   explicit opt-in in the manifest (never a transitive default), (b) a
   declared error/nullability translation (foreign exceptions/
   `undefined` mapped to `Result`/`Option` at the boundary, not left
   as an unhandled foreign failure mode), and (c) `noct audit` flagging
   it as outside the default signing/audit trust tier, with its
   packaging-size and startup-cost impact surfaced, not hidden.
6. **Documentation of the non-goal** — record explicitly (STYLE_GUIDE.md
   or this plan) that no tier performs automatic source-to-source
   translation of foreign packages into `.nv`; tiers 2–4 call
   already-compiled/typed artifacts, tier 5 calls a foreign runtime.
   This keeps M7's actual guarantee distinct from the stronger,
   unsupported "everything becomes `.nv`" claim.

**Exit criteria:**
- One real dependency is successfully pulled in through each of tiers
  2–5 into the M4 reference app (or a comparable demo), each visibly
  labeled by tier in `noct audit`/`noct doc` output.
- The foreign-runtime bridge's error/nullability translation is
  exercised by a test that triggers a foreign-side failure and confirms
  it surfaces as a normal `Result::Err`, not an unhandled foreign
  exception crossing into Noctivue code.
- A dependency audit report distinguishes tier 1–4 (default trust) from
  tier 5 (flagged, opt-in) dependencies for a project mixing all five,
  so the M5 audit story (Phase 6) isn't silently weakened by M7.

## 11. Parallel Track — UI (research spike through UI-M1, hardening through UI-M2)

Runs alongside Phases 1–6, does not gate Phases 1–4 (UI_SPEC.md §4).
Earliest reasonable start for UI-S0: end of Phase 1 / during Phase 2,
once there's a stable enough language to build a managed-mode prototype
against.

```text
UI-S0  ARC runtime prototype        — can start once Phase 1 lands
UI-S1  Widget-tree prototype        — after UI-S0 validates ARC viability
UI-M1  Promotion to production      — only if all UI_SPEC.md §4 gate
                                       criteria pass
UI-M2  Enterprise/production UI     — accessibility, theming/design
                                       tokens, app-scale state management,
                                       per-platform packaging
                                       (desktop/mobile/web); targeted
                                       during the Phase 5/6 window
```

If UI-S0/UI-S1 fail their gates on that timeline, Phases 5–6 (and thus
"build a full-stack app") do **not** block indefinitely on it: the
fallback is native-mode Noctivue with a UI layer built by FFI-wrapping
an existing native UI toolkit, per the "one language, two modes" design
(ADR-001). UI-M1/UI-M2 stay the preferred path; the fallback exists so
enterprise-app delivery has a floor that doesn't depend on the widget-
tree research succeeding on schedule.

## 12. What "Start Building a Real Project" Means at Each Phase

| After phase | What you can actually build |
|---|---|
| 1 (M0) | Non-UI scripts/programs run via `noct run`; good for validating language ergonomics, not for shipping anything |
| 2 (M1) | Same, faster iteration via VM; async programs once lowering lands |
| 3 (M2) | Native binaries — CLI tools, servers, systems-ish code become realistic |
| 4 (M3) | Real multi-file projects with dependencies, tests, and formatting — the first point where "start a project with Noctivue" means something close to what it means in an established language |
| 5 (M4) | **Standard apps**: networked, data-backed services — REST APIs against a real database, with logging and config — the "build a to-do app with a real backend" tier |
| 6 (M5) | **Enterprise-grade apps**: the M4 app hardened with observability, security auditing, resilience patterns, and a deployment/CI pipeline a company can actually run in production |
| UI-M1 (if reached) | UI applications (single-app/prototype maturity) |
| UI-M2 (targeted alongside M5) | Production/enterprise UI — accessible, themeable, packaged per platform, usable as the front end of the M5-tier backend for genuine full-stack delivery |
| 9 (M6) | **One-language coverage**: the same source targets native (Cranelift/LLVM), WASM/web, script/REPL use, embedded (freestanding profile), and can call into existing C++ libraries — the "C++/Python/Rust/Flutter/Java/JS, one language" claim becomes checkable rather than aspirational |
| 10 (M7) | **Ecosystem breadth**: real PyPI/npm/Maven/crates.io libraries usable as dependencies at an explicitly labeled trust tier per package, closing the gap between "the language is capable" and "the language has libraries" |

## 13. Amendment Note

Like DECISIONS.md, changes to this plan's phase ordering are amendments,
not silent rewrites — if a phase's exit criteria turn out to be wrong or
a dependency was missed, record why here rather than quietly
reordering.
