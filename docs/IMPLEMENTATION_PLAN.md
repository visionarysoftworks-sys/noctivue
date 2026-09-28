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

**Progress note (Step 1 — straight-line plumbing):** the exit criteria
are NOT met yet, but the full pipeline runs end-to-end for the
straight-line category: NIR constants/`Move`/int+float arithmetic (with
checked div/rem)/comparisons/`ToString`/calls/`Print`/`Return` lower to
Cranelift IR, string literals ride in `.rodata` under a one-pointer
header model (`runtime-native/src/lib.rs`), and `noct build <file.nv>
[-o <out>] [--release]` links via a generated shim crate (cargo
piggyback — deliberately NOT raw link.exe) against `runtime-native` and
produces a runnable binary. Entry convention: source `main` becomes
`noctivue_main() -> ()`; its return value is dropped like the
interpreter drops it (exit 0 on success — a deliberate parity rule, see
`compiler/src/backends/cranelift/driver.rs`). Proofs:
`compiler/tests/native_smoke.rs` (2 object-emission cases) and
`noct-cli/tests/native_build.rs` (3 build-and-run cases incl. a
div-by-zero trap with exit-code parity against `run-vm`). Still open
(Steps 2–4): control flow, aggregates, borrow enforcement, FFI.

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
    - *Amendment 2026-09-26 (rescoped, not reordered):* the
      "executor suitable for concurrent request handling" is
      delivered as one OS thread per connection/task with
      worker-private interpreters/VMs (the `for_task` precedent),
      proven by the §5 load harness (16×128, zero failures, zero
      leak deltas). Real non-blocking sockets and `async fn` stay
      Deferred to M5 (DECISIONS.md:70-72); the blocking-threads
      floor with at-least sleep timing is the M4 runtime
      (`docs/PHASE5_PRODUCTION.md` §§2–3). Rationale: the exit
      criteria demand concurrent *correctness*, not a specific
      mechanism; the executor (with cancellation, non-blocking
      timers, and multiplexed pools) is M5 work with its own exit
      criteria (CONCURRENCY.md §4/§7), and inventing one in M4
      would strand the reference app on an interim API.
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
    - *Amendment 2026-09-26 (same session as item 1):* the reference
      database is SQLite, file-backed (ADR-019); the delivered
      interface is synchronous and blocking, and the Postgres-class
      driver plus the async query interface ride the M5 executor —
      "async" in the item above names the M5 interface, not an M4
      deliverable. Single-statement atomicity is sufficient for the
      reference app; multi-statement transactions arrive with the
      Postgres-class driver as one designed unit (PHASE5 §4).
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
- Fetched dependency content lives in a global content-addressed
  store (hash-keyed, read-only, atomic placement — TOOLCHAIN.md §3)
  instead of per-project `.noct/cache/` + `.noct/packages/`;
  in-tree `.noct/` holds only build outputs. `vendor/` still builds
  offline, and a manual `noct clean` reclaims the store.

**Status log:**
- 2026-09-26 — the content-store exit criterion above is **met**;
  the rest of Phase 6 is not. Fetched content now lives in the global
  store (`archives/` + `trees/` + `index/` + `points/` under
  `$NOCT_STORE` or the per-OS data home), keyed by the lock's
  `content: sha256:…` hashes, with both halves kept, the archive
  hashed once at fetch, the tree placed atomically and read-only, and
  in-tree `.noct/` holding build outputs only. `vendor/` still builds
  offline and precedes the store in the resolver's search order;
  `noct clean` reclaims the store and the legacy in-tree pair, and
  prints the preserved set. The lockfile format is unchanged: the
  store's own record holds the tree hash the build re-verifies, which
  is what makes a hand-edited extracted tree fail the build instead of
  shipping. Builds may auto-fetch a missing locked dependency but never
  re-resolve; `--frozen` and `--offline` are hard opt-outs. Full
  design, the per-OS root table, the measured warm-path costs and the
  legacy-migration decision are in the dated amendment under
  TOOLCHAIN.md §3. Still open in this phase: the deployment-pipeline,
  audit/signing-enforcement and second-team criteria above.

## 9. Phase 6.5 — Foundation Reconciliation (Gap Closure Between M5 and M6)

**Preconditions:** Phase 6/M5 content-store exit criterion met (2026-09-26). This phase exists because Phase 7's stated preconditions ("Phase 6/M5 exit criteria met," "UI-M1 gate criteria met") are not satisfiable with the following items still open — these are not "cleanup," they are blocking work.

**Work:**

1. **M5 executor (ADR-024) — resolve implementation vs. proposal gap**
   - *Current state (2026-09-26 audit, DECISIONS.md:881–938):* ADR-024 is still **Proposed**. A partial runtime-side core exists in `interp/src/lib.rs` but contradicts the ADR's migration contract.
   - *Spawn-refusal-past-worker-cap:* **Implemented and reachable** — `interp/src/lib.rs:1129–1144` refuses spawns loudly past `executor_worker_count()` (default `available_parallelism` capped at 64, `NOCT_WORKERS` override). This is a new observable loud failure **not listed in ADR-024's explicitly-breaking list** (DECISIONS.md:913–916) — the migration contract claims source/value compatibility but the cap refusal breaks it.
   - *Required:* ~~Either (a) add the spawn-refusal to the explicitly-breaking list and amend the migration contract, or (b) revert the cap~~ — **DECIDED (a), 2026-09-26.** The owner kept the cap; ADR-024's explicitly-breaking list now has item (e) for the spawn refusal, and the "Contradicts this ADR today" item (1) is marked resolved. No code change.
   - *TicklessTimerHeap:* **Deferred to Wave 2 by owner decision (2026-09-26)** — complete implementation in `interp/src/lib.rs` but no users, no tests, nothing schedules into it (DECISIONS.md "Implementation status"). The owner was asked to "surface or delete" and chose defer, which converts this from unowned debt into a **Wave 2 entry blocker**: Wave 2 may not start until the heap is surfaced (with the readiness thread) or deleted.
   - *VM/native lowering:* **Not started** — `run-vm` and Cranelift refuse executor-dependent builtins loudly (`unknown-runtime-symbol`); ADR-024 AC4 requires differential suspend/resume/phi coverage before native lowering begins.
   - *Owner action required:* ~~Reconcile `PHASE5_PRODUCTION.md §6` and `CONCURRENCY.md §4` with what actually runs~~ — **DONE (2026-09-26).** Both now carry an explicit *shipped vs. Wave-2* split: the worker cap, bounded bridge, gauges, and reachable cancellation are recorded as observable runtime behavior, and the work-stealing pool, readiness thread, timer heap, and all VM/native task support are recorded as NOT shipped. ADR-024's "Implementation status" was amended to match rather than silently rewritten.

2. **`task_cancel_builtin` — confirm reachability from `.nv`**
   - *Current state:* **Reachable.** `task_cancel_builtin` exists as a real builtin: typeck signature `(Int) -> Unit` at `compiler/src/typeck/mod.rs:113`, lowered to `Instr::TaskCancel` at `compiler/src/nir/lowering.rs:533`, implemented in interpreter at `interp/src/lib.rs:2381–2389`, wrapped as `task::task_cancel` in `stdlib/concurrency/task.nv:55–56`. Tests in `tests/task_cancel.rs` exercise the full pipeline.
   - *Gap:* ~~Document whether timer cancellation is in scope for Phase 6.5 or deferred.~~ **DECIDED: deferred, not Phase 6.5 scope (2026-09-26).** `TicklessTimerHeap` is left unreachable on purpose, and its unreachability is documented on the type itself so it reads as deliberate rather than accidental. Surfacing it honestly requires the readiness thread that *acts on* a popped id — ADR-024's "Timers" bullet, still Proposed, and ADR-024's own open-items section forbids an implementer silently picking it. The one available shortcut, routing the M4 blocking `sleep_builtin` through the heap, was rejected: it breaks the at-least timing contract the deadline-based poll deliberately provides (the regression the audit fixed) and adds process-global mutable timer state, i.e. the ambient authority ADR-020/021 forbid. A `timer_schedule`-shaped builtin whose returned ids nothing consumes would be exactly the silent no-op the invariants ban. This is Wave-2 executor work.
   - *Residual parity gap (now explicit, not a blocker for this item):* cancellation is interpreter-only. `noct run-vm` / `noct build` refuse any program containing a `task` declaration, and Cranelift rejects `Instr::TaskCancel` with a loud `UnsupportedInstr` naming the instruction — so every backend refuses identically (exit 1), but the cancellation *semantics* are exercised on the interpreter path only. Closing that needs a task registry in `runtime-native` plus the VM spawn path, and lowering only the flag-write would produce a native build that reports a cancellation that cannot happen.

3. **Managed mode / ARC runtime — name the blocker explicitly**
   - *Current state:* **Zero runtime integration.** ARC runtime exists as `interp/src/arc.rs` (HeapRegistry, ArcValue, WeakValue, UnownedValue, ManagedError) with unit tests — but it is **not wired into any backend**. The interpreter has `arc` module imported but no managed-mode execution path; NIR has `Mode::Managed` and managed instructions (`heap_alloc`, `arc_retain`, `arc_release`, `weak_new`, `weak_upgrade`, `unowned_new`) but lowering for them is stubbed; Cranelift backend explicitly marks managed-mode as Step 4/out of scope (`compiler/src/backends/cranelift/lower.rs:8`, `driver.rs:12`).
   - *UI-M1 impact:* UI-S0 (ARC runtime prototype) cannot validate ARC viability without a managed-mode execution path. This **blocks UI-M1 gate criterion (a)** (UI_SPEC.md:106: "ARC behavior is viable under real workloads") and therefore blocks Phase 7's UI-M1 precondition.
   - *Required:* Explicitly name "managed-mode execution path (interpreter + VM + native)" as a Phase 6.5 blocker. Do not carry forward silently.
   - *Verified 2026-09-26 (gate run, not a report):* **still blocked, and
     the gap is now measured rather than assumed.** A managed-mode
     program cannot call any ARC operation:
     - `interp/src/lib.rs` has `eval_builtin` arms for all seven
       (`heap_alloc_builtin`, `arc_retain_builtin`, `arc_release_builtin`,
       `weak_create_builtin`, `weak_load_builtin`, `unowned_create_builtin`,
       `unowned_load_builtin`) and `interp/src/arc.rs` has the runtime —
       but **none of the seven is in `is_builtin`** (`interp/src/lib.rs:1354`),
       and **`compiler/src/typeck/mod.rs` has zero references to them**. So
       a program naming one gets `E0201 unknown identifier`, the arms are
       unreachable, and the only builtins that work are the ones threaded
       through all layers. That is precisely the interp-only violation the
       mirror rule forbids, so the work is not "complete" in any usable
       sense.
     - Real: the `managed` keyword (lexer `token.rs:54` + `mod.rs:981`, parser
       `grammar.rs:311-400`, HIR mode tag) and the NIR variants
       (`instr.rs:78-84`: `HeapAlloc`, `ArcRetain`, `WeakLoad`, …).
   - *UI-S1 does not use ARC at all:* `libs/ui/lib/*.nv` contains **zero**
     ARC/managed calls — its only two hits for `managed`/ARC are comments.
     `cycle_mitigation.nv` is honest pure-value code whose own header says
     the checks "are checks on values and a heap object needs different ones"
     once real ARC lands. `register_child`, `validate_widget_tree`, and
     `make_unowned_parent` (named as delivered) do not exist; only
     `diff_widgets`/`diff_widgets_depth` do. `libs/ui` also has no `tests/`,
     no `README.md`, and no `docs/overview.md` — although `cycle_mitigation.nv:47`
     points readers at that last file.
   - *The demo does not run:* `noct run lib/main.nv` in `examples/ui_demo`
     fails `E0101 cannot resolve imported module 'ui'`, cascading to ~20
     unknown-identifier errors. `compiler/src/modules.rs` never reads
     `nestpkg.nvpm`, so a manifest `path:` dependency cannot resolve; the
     resolver only searches sibling dirs, `vendor/`, and store roots.
     (`examples/reference-crud` works only because it declares no
     dependencies.)
   - *Consequence for the gate:* criteria (a) real-workload ARC,
     (b) weak/unowned noise, and (c) re-render performance are all
     **unevaluable** — not failing, not passing. Nothing has been measured.
     Criterion (d) (dogfooding) is likewise unassessable while the demo
     will not build.
   - *Measured 2026-09-26, UI-M1 gate criteria (a)-(c):*
     - **(a) REAL WORKLOAD: PASS.** `libs/ui/lib/ownership.nv` puts the
       widget tree on the managed heap (see below) and
       `libs/ui/tests/ownership_test.nv` exercises it: root with no
       parent, child→unowned→parent traversal resolving to the parent's
       tag, `Weak` observing a live parent (`Some`), and the same `Weak`
       yielding `None` once the only strong reference is released (which
       proves the release actually freed the object). All five pass on
       the interpreter. ARC is no longer a prototype — it is the
       ownership model of a real package.
     - **The shape that works is a flat arena, and that is a finding, not
       a shortcut.** The managed heap is write-once from `.nv`: you can
       allocate a value and read it back, but you cannot store into an
       object you already hold. So a parent created complete can never
       afterwards gain a strong `kids` list, and a nested `parent ->
       children` owning tree cannot be built after the fact. What builds
       honestly is the direction that needs no mutation: every child is
       allocated with an `unowned` back-reference to a parent that
       already exists, while the application holds every node strongly.
       No strong cycle is possible by construction, which is exactly what
       makes the `unowned` back-reference sound.
     - **(b) NON-OWNING RATIO: PASS, at half the bar.** The gate's bar
       (owner-set) is at most 2 non-owning references per node. The
       steady state uses exactly 1 per child (its parent back-ref) and 0
       per root; a `Weak` appears only as a transient observer, never as
       structure. `ownership_test.nv` proves each child's single
       back-ref resolves, so the count is measured, not asserted.
     - **(c) RE-RENDER PERFORMANCE: FAIL.** A 100-node
       `column(text, ...)` re-render (`diff_widgets`, identical trees,
       nothing to find) costs **1,907 ms**; at 250 nodes it costs
       **7,665 ms**; a 1,000-node build+diff measured **227 s** wall.
       Build is quadratic (`list_append_builtin` copies the list per
       element) and the diff is superlinear (the `Mutation` chain is
       rebuilt by copying through `graft`/`push`/`reverse`). The value
       model copies everywhere, so no interactive budget — 16 ms, 100
       ms, even 1 s — is reachable at any reasonable scale with this
       implementation. An `Arc`-identity diff (comparing heap ids instead
       of deep values) would change the asymptotics, but that is a
       redesign of `cycle_mitigation`, not a tuning pass. Recorded as a
       fail with data rather than a smaller certified size, because a
       smaller number would certify a UI framework that cannot render a
       UI.

4. **Standard Error trait — decide, or document every distinct shape**
   - *Current state:* **Not decided.** `stdlib/error/traits.nv:5–10` explicitly reserves trait-based error handling for M4+ (requires trait/impl support, SPEC.md C4). `ERROR_HANDLING.md:62–66` confirms: "`E` in `Result<T, E>` is an ordinary Noctivue type… standard-library error trait/convention is Deferred."
   - *Distinct error shapes introduced so far (each module invents its own):*
     - `Result<T, String>` — foundation convention (E3), used by `db/sqlite.nv`, `net/http/*.nv`, `concurrency/task.nv`, `fs/*.nv`, `process/command.nv`, `result/result.nv`, `error/ffi.nv`, `error/macros.nv`
     - `ErrorInfo { code: String, message: String, source: String }` — structured record in `stdlib/error/types.nv:12–16` with free functions `error_info`, `error_info_display`, `error_info_context`
     - `enum ErrorCode { BadRequest, Unauthorized, Forbidden, NotFound, Conflict, RateLimited, Internal }` — in `examples/noctivue-expert-server-service.nv:55–63`
     - `struct AppError { message: String }` — in `examples/nightshade/lib/core/errors/exceptions.nv:1–5`
     - `Result<T, String>` with pinned string literals for pool (`"pool max_size must be at least 1"`, `"pool exhausted (max_size reached)"`) — `stdlib/db/pool.nv:36,57`
   - *Required:* Explicit go/no-go on a standard `Error` trait for M5. If deferred again, document the convention (e.g., "all stdlib fallible fns return `Result<T, String>`; structured records use `ErrorInfo`; application enums are free") and add a linter rule to flag new ad-hoc shapes.

5. **Panic/unwind semantics for native mode — decide**
   - *Current state:* **Deferred.** `ERROR_HANDLING.md:44–46`: "A panic in **native mode** aborts the current process (or, where the platform/runtime configuration allows, unwinds to a defined boundary — exact unwind-vs-abort configurability is **Deferred**)." `NIR.md:92,190,200,203`: `panic`/`assert`/`to_int` builtins not representable in NIR yet; `CondBranch` truthiness vs interpreter strict-Bool panic divergence confined to ill-typed programs.
   - *Phase 6 impact:* Real I/O shipped (Postgres-class driver authorized per ADR-024 hybrid revision DECISIONS.md:720–726, TLS via rustls+ring authorized same entry) — but panic/unwind contract for native FFI boundaries is undecided.
   - *Required:* Decide unwind-vs-abort for native mode before Phase 7's multi-target work (WASM, C++ interop) makes it load-bearing. Record as ADR amendment.

6. **Imports/exports — settle as decided, not proposed**
   - *Whole-module import (`import event + event::kind_closed()`):* **Already legal** per MODULES.md §2 (item import `import path::Item` works). The fix is a style-guide + linter rule ("prefer whole-module import over 3+ per-item imports from one module"), not new grammar.
   - *Re-exports (`export import ...`):* **Proposed** in MODULES.md §5 — must be implemented. Implement `export import resilience::prelude::*` (or `export import resilience::prelude`) so consumers see one import, not nine, without weakening per-declaration `export` as the visibility default. Do NOT make whole-file export the default.
   - *Diagnostic gaps in MODULES.md §4.1 (must fix before/during re-exports, since re-exports increase surface area):*
     - **Gap 1:** `pkg::Type` qualifier not stripped — unaliased module names (`import semver::version` registers `version`, never `semver`) leave qualifier in place, causing type mismatch pointing at type not missing import (MODULES.md:95–102). Fix: register full module path as alias or strip unaliased qualifiers in rewriter.
     - **Gap 2:** Private-access errors blame the alias — `p.private_one()` (where `private_one` lacks `export`) emits `E0201 unknown identifier p` instead of naming the private item and the `export` fix (MODULES.md:104–109). Flat use gets `W0101` hint; qualified form does not. Fix: qualified private access should emit the same hint or a dedicated diagnostic.
   - *Update MODULES.md:* Mark all of the above as **Decided**, not "Proposed."

**Exit criteria:**
- ADR-024 status changed from **Proposed** to **Accepted** (or explicitly rejected with migration path), with spawn-refusal contradiction resolved and `TicklessTimerHeap` fate decided.
- `PHASE5_PRODUCTION.md §6` and `CONCURRENCY.md §4` reconciled with shipped behavior (no "M5 design" language for what runs today).
- Managed-mode execution path has a concrete plan with owner and timeline, or is explicitly deferred with UI-M1 impact acknowledged in writing (not silent).
- Standard Error trait: go/no-go recorded as ADR; if deferred, convention documented and linter rule added for ad-hoc shapes.
- Panic/unwind semantics for native mode: go/no-go recorded as ADR amendment.
- Re-export syntax (`export import ...`) implemented and working; whole-module import style rule in linter (default-on or default-off per style-preference convention).
- Both diagnostic gaps in MODULES.md §4.1 fixed (unaliased `pkg::Type` stripped; qualified private access emits actionable hint).
- MODULES.md updated: §1 directory-module nesting rules, §5 re-exports, and §4.1 diagnostic gaps all marked **Decided**.

**Go/No-Go for Phase 7 (M6):**
- **Phase 6/M5 exit criteria met?** **NO.** Phase 6 exit criteria (IMPLEMENTATION_PLAN.md §8 lines 284–300) require: deployed reference app with metrics/traces, `noct audit` clean with signing enforced, second-team onboarding, content-store met. Only content-store is met (2026-09-26). The rest are open.
- **UI-M1 gate criteria met?** **NO.** UI-SPEC.md §4 requires: (a) ARC viable under real workloads — blocked by zero managed-mode execution path; (b) cycle-mitigation strategy holds — unvalidated; (c) re-render performance acceptable — unmeasured; (d) ergonomics compelling in dogfooding — untested.
- **FFI-wrapped toolkit fallback genuinely in place?** **NO.** No native-mode FFI-wrapped UI toolkit exists; ADR-001 fallback is architectural intent, not implemented code.
- **Conclusion:** **NO-GO for Phase 7.** Phase 6.5 is not "informal cleanup that can run in parallel" — it is the actual blocking work. Phase 7 must not start until Phase 6.5 exit criteria are met.

## 10. Phase 7 — M6: Multi-Target Compilation & Interop Expansion

**Preconditions:** Phase 6.5 exit criteria met (which subsumes Phase 6/M5 exit criteria for the backend-app side); UI-M1 gate criteria (UI_SPEC.md §4) met or the FFI-wrapped-toolkit fallback in use for the UI side — this phase's WASM/mobile work assumes *a* UI story exists, not necessarily the in-house one.

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

## 11. Phase 8 — M7: Cross-Ecosystem Package Interop

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

## 12. Parallel Track — UI (research spike through UI-M1, hardening through UI-M2)

Runs alongside Phases 1–7, does not gate Phases 1–4 (UI_SPEC.md §4).
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
                                       during the Phase 5/6/6.5 window
```

If UI-S0/UI-S1 fail their gates on that timeline, Phases 5–7 (and thus
"build a full-stack app") do **not** block indefinitely on it: the
fallback is native-mode Noctivue with a UI layer built by FFI-wrapping
an existing native UI toolkit, per the "one language, two modes" design
(ADR-001). UI-M1/UI-M2 stay the preferred path; the fallback exists so
enterprise-app delivery has a floor that doesn't depend on the widget-
tree research succeeding on schedule.

## 13. What "Start Building a Real Project" Means at Each Phase

| After phase | What you can actually build |
|---|---|
| 1 (M0) | Non-UI scripts/programs run via `noct run`; good for validating language ergonomics, not for shipping anything |
| 2 (M1) | Same, faster iteration via VM; async programs once lowering lands |
| 3 (M2) | Native binaries — CLI tools, servers, systems-ish code become realistic |
| 4 (M3) | Real multi-file projects with dependencies, tests, and formatting — the first point where "start a project with Noctivue" means something close to what it means in an established language |
| 5 (M4) | **Standard apps**: networked, data-backed services — REST APIs against a real database, with logging and config — the "build a to-do app with a real backend" tier |
| 6 (M5) | **Enterprise-grade apps**: the M4 app hardened with observability, security auditing, resilience patterns, and a deployment/CI pipeline a company can actually run in production |
| 6.5 (Foundation Reconciliation) | **Unblocked M6**: M5 gaps closed (executor contract, ARC runtime path, error convention, panic semantics, module system settled) — Phase 7 can now start |
| UI-M1 (if reached) | UI applications (single-app/prototype maturity) |
| UI-M2 (targeted alongside M5) | Production/enterprise UI — accessible, themeable, packaged per platform, usable as the front end of the M5-tier backend for genuine full-stack delivery |
| 10 (M6) | **One-language coverage**: the same source targets native (Cranelift/LLVM), WASM/web, script/REPL use, embedded (freestanding profile), and can call into existing C++ libraries — the "C++/Python/Rust/Flutter/Java/JS, one language" claim becomes checkable rather than aspirational |
| 11 (M7) | **Ecosystem breadth**: real PyPI/npm/Maven/crates.io libraries usable as dependencies at an explicitly labeled trust tier per package, closing the gap between "the language is capable" and "the language has libraries" |

## 14. Amendment Note

Like DECISIONS.md, changes to this plan's phase ordering are amendments,
not silent rewrites — if a phase's exit criteria turn out to be wrong or
a dependency was missed, record why here rather than quietly
reordering.
