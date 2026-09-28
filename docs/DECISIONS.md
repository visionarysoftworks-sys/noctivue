# DECISIONS.md — ADR Log & Technical Consistency Review

## 1. Status Taxonomy

Every design element in this documentation set is tagged with one of:

| Tag | Meaning |
|---|---|
| **Confirmed** | Locked architectural decision. Changing it requires a new ADR marked "Amendment," never a silent rewrite. |
| **Proposed** | A concrete design under active consideration, not yet load-bearing for implementation. |
| **Experimental** | Will be built and tested (e.g. behind a research spike) but may be discarded based on results. |
| **Deferred** | Explicitly out of scope for now; revisit once prerequisites land. |
| **Open** | Genuinely unresolved; multiple options on the table, no recommendation frozen. |

## 2. Architecture Decision Records

### ADR-001 — Bifurcated Memory Model
**Status:** Confirmed
Noctivue has two execution/memory modes: native (ownership + borrowing)
and managed (ARC), sharing one syntax, type system, and compiler
frontend. Rationale: systems/embedded work needs deterministic,
GC-free resource management; UI/application work benefits from
ARC's simpler mental model. See MEMORY_MODEL.md.

### ADR-002 — Compiler Implemented in Rust
**Status:** Confirmed
Rust gives memory safety for the compiler itself, a mature ecosystem
(Cranelift, LLVM bindings, parser tooling), and strong alignment with
Noctivue's own native-mode design influences.

### ADR-003 — Cranelift-First Native Backend
**Status:** Confirmed
Cranelift ships first for faster iteration and simpler embedding; LLVM
is added later as an optional high-optimization release backend. See
COMPILER_ARCHITECTURE.md §5.

### ADR-004 — ARC for Managed Mode
**Status:** Confirmed (mechanism) / **Open** (surface syntax, cycle
mitigation specifics — see MEMORY_MODEL.md §4).
ARC chosen over tracing GC for managed mode to avoid unpredictable pause
times in UI contexts, at the cost of needing explicit `weak`/`unowned`
cycle-breaking.

### ADR-005 — Flutter-Style Widget Tree
**Status:** Confirmed (model) / **Experimental** (implementation via
UI-S0/UI-S1 research spike, see ROADMAP.md).
The UI framework is an ordinary package built on public compiler APIs,
not privileged compiler functionality.

### ADR-006 — Single `.nv` Source Extension
**Status:** Confirmed
Developers work with exactly one source extension. Generated/internal
artifacts (e.g. `.nvir`) are permitted but must not compete visually or
workflow-wise with `.nv`. See STYLE_GUIDE.md §6 (Developer-Facing File
Identity).

### ADR-007 — Colon-First Declaration Syntax
**Status:** Confirmed
The primary visual identity of Noctivue. `Name:` introduces a structural
block; indentation (or braces) expresses hierarchy. See SYNTAX.md.

### ADR-008 — Expanded + Compact Syntax, Same AST
**Status:** Confirmed
Minimal/compact/expanded forms and indentation/brace forms must lower to
equivalent AST shapes. The formatter converts between them
losslessly. See SYNTAX.md §3, TOOLCHAIN.md.

### ADR-009 — Structured Concurrency
**Status:** Confirmed (model + M4 surface). One official async runtime;
no competing executors in the core ecosystem. M4 surface is `task` +
`await` + `sleep` (blocking threads, structured join at scope exit);
`async fn` and non-blocking executor are Deferred to M5. See
CONCURRENCY.md.

### ADR-010 — One Official Package Manager
**Status:** Confirmed
`noct` is the single package manager, lockfile format, and dependency
resolver. See TOOLCHAIN.md §3.

### ADR-011 — AI-Native Compiler Interfaces
**Status:** Confirmed (principle) / **Proposed** (exact command surface:
`noct ast --json`, `noct diagnostics --json`, etc.). See AI_TOOLING.md.

### ADR-012 — Developer-Facing File Identity & Branding
**Status:** Confirmed (requirement) / **Deferred** (final logo artwork).
`.nv` must have an official file icon, canonical language identifier
(`Noctivue` / `nv`), and eventual editor/OS/Git integration. The exact
visual mark is a branding task tracked separately from the grammar spec.
See STYLE_GUIDE.md §6.

### ADR-013 — Scope Expansion: Standard & Enterprise-Grade Applications (Amendment)
**Status:** Requires approval
**Conflicting requirement:** ROADMAP.md v0.1 defined "done" as M3
(stdlib + package manager + testing + formatter + LSP), with UI as a
non-blocking, unscheduled research spike. That endpoint supports
non-UI CLI/systems programs and, if UI-M1's gates pass, simple
single-process UI apps — it does not cover networked, data-backed, or
production-hardened applications, which is what "standard" and
"enterprise-grade" app development requires (auth, databases, APIs,
observability, deployment, resilience, UI at scale).
**Why the original scope no longer holds:** M0–M3 was deliberately
minimal to de-risk the core language (Principle 9: don't grow the core
prematurely) before proving out anything network- or data-facing. That
principle is still correct for M0–M3 — this amendment does not reopen
those phases — but it means the *published* roadmap stopped one to two
milestones short of "build a real app" for most real-world definitions
of that phrase. Two milestones are added after M3, and the UI track's
relationship to the overall plan is clarified, rather than expanding
M0–M3's scope.
**Amended text:** ROADMAP.md §1 gains M4 (Networked & Data-Backed
Applications) and M5 (Production & Enterprise Hardening), sequenced
after M3, detailed in IMPLEMENTATION_PLAN.md Phases 5–6. ROADMAP.md §2
(UI Research Track) is amended so that UI's promotion out of research
status is a scheduled objective of the M4/M5 window (still gated on
UI-S0/UI-S1 criteria, per UI_SPEC.md §4, unchanged) rather than an
open-ended "if reached" side track, and gains an explicit fallback
(FFI-wrapped native UI toolkits) so full-stack app delivery is not
indefinitely blocked on the in-house widget-tree research succeeding
on schedule. ROADMAP.md §3 (Explicit Non-Requirements for MVP) is
narrowed to explicitly scope to M0–M3; items resolved in M5 (a first
cut of capability sandboxing, package audit/signing enforcement) are
called out as moved, not silently dropped from "non-requirements."
No previously Confirmed ADR in the M0–M3 core (memory model, syntax,
concurrency model, toolchain principles) is altered by this amendment;
M4/M5 are additive milestones built on top of them.

### ADR-014 — Multi-Paradigm, Multi-Target Coverage (Amendment)
**Status:** Requires approval
**Conflicting requirement:** ROADMAP.md's comparative notes (§4, now
§6) position Noctivue as drawing from Rust, Swift, Dart/Flutter,
Python, Kotlin, Go, Zig, C++, and TypeScript — implying it should be
viable where each of those is used today: systems/embedded (C++,
Rust), rapid scripting (Python), enterprise backends at scale (Java-
adjacent, via Kotlin's lineage), UI/mobile (Flutter), and the web
(TypeScript/JS). ADR-013 (M4/M5) covers the *enterprise backend +
UI* slice of that list. It does not cover systems/embedded parity,
scripting/REPL ergonomics, or a web/WASM target — without those,
"one language for everything" is aspirational text, not a plan.
**Why the original scope no longer holds:** M0–M3 built the core
language; M4/M5 built the server-side production story. Neither
touches compilation *targets* beyond native + VM, nor the ergonomic
"just run this like a script" mode Python users expect, nor a
scoped answer for interop with existing C++ codebases (as opposed to
plain C, which FFI.md already covers). These are gaps in target/
paradigm coverage, not gaps in production-readiness, so they're
tracked as a distinct milestone rather than folded into M4/M5.
**Amended text:** ROADMAP.md gains a Domain & Platform Coverage
section (§6) mapping each reference language to what Noctivue offers
and where, plus milestone **M6 — Multi-Target Compilation & Interop
Expansion**, detailed in IMPLEMENTATION_PLAN.md Phase 7.
COMPILER_ARCHITECTURE.md §5's backend strategy gains a WASM backend
(NIR was already designed mode-tagged and backend-agnostic — NIR.md
§2 — so this is additive, not a redesign). FFI.md gains a scoped,
gated tier for C++ interop distinct from the already-Confirmed C ABI
tier, since C++'s lack of a stable ABI makes it a materially different
problem, not a variation on FFI.md's existing content. No Confirmed
ADR governing M0–M5 is altered.

### ADR-015 — Tiered Cross-Ecosystem Package Interop (Amendment)
**Status:** Requires approval
**Conflicting requirement:** TOOLCHAIN.md §1's "one official package
manager" principle and ROADMAP.md §6's Python/JS/Java coverage row are
currently satisfied only by native `.nv` packages (post-M4) plus the
gated C/C++ FFI tiers (FFI.md). That leaves the actual breadth
advantage of Python/JS/Java — decades of existing libraries on
PyPI/npm/Maven/crates.io — untouched. ROADMAP.md §6 itself flagged
"ecosystem breadth" as something M0–M6 does *not* provide.
**Why the original scope no longer holds:** it doesn't — this isn't a
correction, it's new scope. The design brief's "does it all without
breaking a sweat" ambition (README.md, ROADMAP.md §4/§6) is
under-served without *some* answer to existing-ecosystem breadth, and
that answer needs to be structured as explicit trust/fidelity tiers
rather than one feature, because "import a foreign package" means
something very different depending on the source ecosystem (a
C-ABI-exporting Rust crate vs. an arbitrary PyPI package are not the
same problem, and conflating them would misrepresent what's actually
guaranteed).
**Amended text:** ROADMAP.md gains milestone **M7 — Cross-Ecosystem
Package Interop**, detailed in IMPLEMENTATION_PLAN.md Phase 8, and
TOOLCHAIN.md §3 gains a per-dependency trust-tier field in the package
manifest. Five tiers, decreasing in fidelity/trust:
1. **Native** — ordinary `.nv` packages (M4 baseline); full M5 audit/
   signing trust applies.
2. **C-shim** — C ABI or crates/libraries exporting one (FFI.md §1–8);
   same trust tier as native, since the boundary is fully specified.
3. **C++-shim** — the gated shim-generator tier (FFI.md §9); Experimental
   status carries through to any package using it.
4. **WASM-component** — typed interop via the WASM Component Model
   (WIT interfaces), reusing the M6 WASM backend; no embedded foreign
   runtime, so this stays close to tier 1/2 trust, gated only on
   upstream packages actually shipping a WASM component.
5. **Foreign-runtime bridge** — an embedded JVM/CPython/JS runtime with
   a marshaling boundary, for ecosystems with no C-shaped or
   WASM-component export. **Explicitly excluded from the default M5
   trust tier**: opt-in per dependency, flagged in the manifest and in
   `noct audit` output, not covered by the same signing/audit
   guarantees as tiers 1–4, and documented as shipping the foreign
   runtime's packaging/performance/security cost, not translating the
   package into `.nv`.
No tier claims to convert foreign source into `.nv` source
automatically — general source-to-source transpilation from
dynamically-typed languages is noted as **not attempted** (infeasible
at ecosystem scale given reflection/`eval`/dynamic-typing semantics),
distinguishing this ADR's actual scope from that stronger, unsupported
claim.

### ADR-016 — NIR Phi-Completeness Is a Trap, Not a Default (Amendment)
**Status:** Accepted (retroactively documents a change already made in
NIR.md §4.1 item 10 and `compiler/src/nir/vm.rs`'s `VmError::MalformedCfg`
on 2026-09-04; recorded here per the Amendment process below, which that
change should have gone through at the time instead of landing as a
NIR.md-only edit.)
**Conflicting requirement:** NIR.md §4.1 rule 1 ("every reachable block
is terminated... terminate dead blocks anyway") and the VM's general
"no silent recovery" discipline (rule 9, `try_unwrap`'s trap-on-None/Err)
did not, until this amendment, cover the case of a `Phi` reached via a
predecessor block with no corresponding `incoming` entry. The VM's
original `Phi` handler defaulted to `VmValue::Unit` in that case,
producing a wrong-but-non-erroring result — precisely the failure
signature §4.1's header already warns about ("most of them execute
without errors") but that this specific case wasn't yet listed against.
**Why the original scope no longer holds:** a lowering bug that
silently substitutes `Unit` for a real value is strictly worse than one
that traps, because it can propagate through arithmetic/printing before
surfacing (if it surfaces at all), and the differential suite only
catches it as *wrong output*, not as a distinguishable failure. Any
future control-flow construct that adds a new edge into an existing
merge block (this session's `break`/`continue` lowering is the first
one) needs a loud failure mode here, not a silent one, to be safe to
implement incrementally.
**Amended text:** NIR.md §4.1 gains rule 10 (already present in the
current doc): every Phi's `incoming` list must cover every actual
predecessor that can reach it; a Phi reached via an unrecorded
predecessor — or with no predecessor recorded at all — traps as
`VmError::MalformedCfg` rather than defaulting to `Unit`. No ISA change
(no new `Instr` variant); this is a VM-behavior and documentation
amendment only.

**Amendment process:** Any change to a Confirmed ADR must (1) name the
conflicting requirement, (2) explain why the original decision no longer
holds, (3) propose the amended text, and (4) be marked "Requires
approval" until accepted — never silently rewritten in place.

### ADR-017 — Project-Adjacent File Types: `nestpkg.nvpm`, `nestpkg.lock`, `*.nv.env`
**Status:** Proposed.
**Decides what was Open:** TOOLCHAIN.md §3 left the manifest format open
(TOML-like vs. Noctivue-native) with no filename; no env/config file was
specified anywhere (config loading itself is scheduled, Phase 5/M4,
without a file); the lockfile had a mandate (one canonical, checked-in
format) but no name.
**Decision:**
- Project manifest: a file named **`nestpkg.nvpm`** (`nvpm` = Noctivue
  package manifest — unique stem and extension, no collision with any
  existing ecosystem). Custom Noctivue-flavored declarative syntax
  (colon blocks + significant indentation, per SYNTAX.md §§6–7 —
  *not* YAML/TOML), read by a dedicated hand-written line-oriented
  parser (no expressions, no interpolation), homed as a `manifest`
  module inside `noct-cli` for M3 and split to its own crate if/when
  the registry needs it. The full compiler frontend is deliberately not
  involved: manifest parsing must work before/without a compilable
  project. v1 fields: package name, version, description,
  dependencies (with ADR-015 trust-tier annotations), dev-dependencies.
- Lockfile: **`nestpkg.lock`**, same stem, generated by `noct add`/
  `noct build`, checked in. Format decided with the manifest parser
  (machine-generated, human-readable).
- Env file: **`*.nv.env`** (e.g. `app.nv.env`), dotenv-compatible
  `KEY=VALUE` syntax (no invented format — compatibility wins),
  `noct run` auto-loads `./*.nv.env` (explicit `--env-file` overrides),
  process environment always wins over file values, secrets never
  committed (`.gitignore` guidance ships with the feature).
- Editor rule (STYLE_GUIDE.md §6.7 applies): `nestpkg.nvpm` gets language
  association by exact filename; generated files (`nestpkg.lock`) get
  none of the `.nv` identity (no icon, no source-view surfacing).
**Milestones:** manifest + lockfile land in Phase 4 (M3) with the
package manager as already planned; env loading lands in Phase 5 (M4)
with config support as already planned. No roadmap surgery — these are
naming/format decisions inside existing scheduled work.
**Amendment (2026-09-06, Phase 4 execution — P-003 §§1–8 as built):**
grammar, caret/exact requirements, tier + opt-in (incl. the
transitivity rule), canonical lock, and the TOFU-with-paper-trail
Ed25519 design are implemented as specified (`noct-cli/src/manifest.rs`
+ `registry.rs`). Implementation choices the proposal left open,
recorded here rather than silently assumed: the registry backend is a
file-backed directory index (`<root>/<name>/<version>/` with
`manifest.nvpm`, `pkg.bin`, `pkg.hash`, `pkg.sig`, `pkg.key`, plus
`<root>/_keys/`; the HTTP API stays deferred); resolution is highest-
satisfying-version over the closure by fixpoint (constraints only
accumulate — no backtracking false-conflicts), tier conflicts and
transitive-only foreign-runtime fail loud with chains named; tarball
framing is a minimal length-prefixed v1 (fixture interchange, not a
wire format); fetched packages unpack under `.noct/` (cache +
unpacked tree); key material lives per-OS config home with a
`NOCT_KEYS` override for tests/hermetic use. `noct build`/`run`/`test`
enforce manifest-vs-lock-vs-cache consistency (the lock is the build
input); `noct audit` prints the trust columns from lock data alone;
`noct publish --dry-run` validates locally while real publish stays
refused (no registry, no verification path yet). Still deferred:
HTTP registry API, compound version ranges (v2), yank/revoke flows,
vulnerability database (M5).

### ADR-018 — Narrowly-Scoped Derive for Serialization (Phase 5/M4)

**Status:** Confirmed (implemented: `derive` keyword, resolver
expansion to `to_json_<T>` / `from_json_<T>` + marker impl,
`doc_get_char` / `doc_get_index` / `list_append_builtin`,
E0320–E0328 diagnostics; `enum` targets and user-defined derives
still deferred per the non-goals).

**The constraint.** ROADMAP keeps general macros deferred, and the
language has no attribute syntax, no method-call syntax, no Map
values, and no trait dispatch (C1/C4). A general derive/macro
system is out of the question for M4. What the reference
application actually needs is narrower: turn request structs into
JSON text and back, without hand-writing (and hand-rotting) the
field lists.

**Decision.** Exactly one new declaration form, exactly two
derivable traits, expanding to ordinary items at resolve time —
never a macro system, never user-extensible in M4:

```nv
derive Serialize for User:
derive Deserialize for User:
```

- `derive` becomes a keyword (no corpus use as an identifier — safe).
- Only `Serialize` and `Deserialize` (both marker traits declared in
  `encoding/json/serialize.nv`) may follow it; anything else is a
  loud error naming the closed set. Only `struct` targets are
  accepted in v1 (`enum` targets are a loud "not yet" — their tagged
  representation is its own future ADR).
- Expansion is desugaring, not code generation into text: the
  resolver appends synthesized `FunctionDecl`s plus a marker
  `ImplBlock` to the program, and every downstream stage (typeck,
  HIR, VM, native) treats them as hand-written items. No backend
  work, no hygiene questions (fixed expansion), no new name
  resolution rules (synthesized names are fixed and documented).
- Synthesized surface (the contract — rename only via a new ADR):
  `to_json_<Type>(v: Type) -> String` and
  `from_json_<Type>(doc: JsonDoc) -> Result<Type, String>`, built
  from `string_concat`, `json_quote`, and the `JsonDoc` accessors.
  Field coverage v1: `Int`, `Float`, `Bool`, `Char`, `String`,
  nested derived structs (missing derive on the field type is a
  loud error naming it), `Option<T>` of those (absent key or
  explicit `null` both decode to `None`), `List<T>` of those
  (emitted via loops over `string_concat`). Anything else in field
  position (`Result`, tuples, function types, maps) is a loud
  "unsupported derive field" error, never a silent skip.
- Missing JSON fields decode-error naming type and field; extra
  fields are ignored (documented); whole-number floats cross as
  integers on the wire and coerce back on decode (documented
  `doc_get_float` behavior, not a derive special case).
- `JsonDoc` itself (opaque handle over a Rust-side parsed document)
  is specified alongside: no `Map` value, no `Ty`/`Value` surgery —
  revisit if `Map` ever lands.

**Non-goals (still deferred, explicitly):** general macros,
user-defined derives, derive for enums, `impl`-block method
synthesis beyond the marker, reflection of any kind.

### ADR-019 — SQLite File-Backed Reference Database (Phase 5/M4)

**Status:** Confirmed (implemented: `db/sqlite.nv` + `db/pool.nv`
over bundled rusqlite, binding-only JSON interchange, task-local
pools, 16×128 two-cycle load harness in `tests/http_load.rs`).
Filed 2026-09-26 to repair the dangling `docs/PHASE5_PRODUCTION.md`
reference — the decision predates this entry, which records rather
than re-decides it.

**The constraint.** The M4 reference app needs a production
database with zero system dependencies and zero network services
in test. A client-server engine fails that bar; SQLite (bundled
via rusqlite, no system library) passes it.

**Decision.** SQLite is the reference engine: file-backed temp
databases under load (`PRAGMA integrity_check` + spot reads),
`:memory:` for unit smokes, WAL journaling + 5 s busy timeout for
concurrent handlers, single-statement atomicity as sufficient
(the reference CRUD is expressible without multi-statement
transactions). Values bind from JSON scalar arrays only —
interpolation into SQL text is a convention + self-grep test
failure, never a runtime guess.

**Non-goals (still deferred, explicitly):** replication/failover,
a client-server (Postgres-class) driver, multi-statement
transactions (one designed unit with that driver, Phase 6),
full ORM (never core — ecosystem).

### ADR-020 — Monotonic Clock Source (Phase 6/Wave 0)

**Status:** Implemented (2026-09-26, Phase 6/Wave 0 — `time_mono_ms_builtin`
in the interpreter + NIR VM, Cranelift loud-reject, `stdlib/time/instant.nv`
promoted to runnable, `tests/clock_rng.rs` 10/10). The Proposed rationale
below is retained verbatim per §1 (no silent rewrites); the
implementation-outcome amendment is recorded at the end of this entry.

**The constraint.** Nothing in the tree can observe time passing:
`stdlib/time/clock.nv` is byte-empty and no doc mentions time at
all. Wall-clock dates need a timezone database (M6+ weight), but
every Phase 6 timing feature only needs *durations between
instants* — a far smaller surface.

**Decision.** Monotonic milliseconds first: a `time_mono_ms_builtin`
(interp-only initially, VM/`build` fail loud per the host-IO
precedent), with `stdlib/time/instant.nv` promoted to runnable
carrying the mono-clock wrapper only. Semantics: arbitrary epoch,
`u64` millis, never goes backward within a process; no date/time
interpretation at the builtin layer. The `resilience` lib's `Int`
millis integrate onto it unchanged, and its fake-clock test
pattern becomes the conformance oracle (real clock must agree
with simulated clock on policy math).

**Non-goals (still deferred, explicitly):** wall-clock dates,
timezones, calendars, NTP, sleep precision beyond at-least,
deadline objects (built in libraries on top, not in the builtin).

**Implementation amendment — 2026-09-26 (Wave 0, as built).** One
correction to the Decision's mechanism, recorded rather than assumed:
the builtin shipped on BOTH runtimes from day one, not interp-only.
The host-IO precedent (VM/build fail loud) would have made
`time_mono_ms_builtin` interp-only; but it is a pure scalar `Int`
computation over a process anchor, so lowering needed no new `Instr`
— `Instr::TimeMonoMs` was added to the existing VM-executed set
(`compiler/src/nir/lowering.rs`, `compiler/src/nir/vm.rs`) and
Cranelift rejects it through its normal `is_supported` pre-check, so
`noct build` still fails loud. Every Decision semantic is honored:
arbitrary process epoch (`LazyLock<Instant>` in both runtimes),
millisecond `Int`, monotonic within a process, and no date/time
interpretation at the builtin layer. `stdlib/time/clock.nv` remains a
byte-empty reserved stub — the wall-clock non-goal is untouched. The
Decision's "resilience fake-clock as conformance oracle" clause is
NOT yet discharged: `libs/resilience` still has no test binding real
`instant_now_ms()` against its simulated clock (see that lib's
`docs/overview.md`).

### ADR-021 — Explicit-Seed RNG, No Global Entropy (Phase 6/Wave 0)

**Status:** Implemented (2026-09-26, Phase 6/Wave 0 — `rng_seed_builtin` +
`rng_next_builtin` in the interpreter + NIR VM, Cranelift loud-reject,
`stdlib/numbers/random.nv` promoted to runnable, `tests/clock_rng.rs`
10/10). The Proposed rationale below is retained verbatim per §1 (no
silent rewrites); the implementation-outcome amendment is recorded at
the end of this entry.

**The constraint.** Jitter and property tests need randomness, but
a global entropy source is untestable (unreplayable failures) and
unauditable (unpredictable consumption). The `testing_ext` lib
already proved the alternative shape in pure `.nv` (seeded LCG,
threaded state, deterministic replay) — this ADR ratifies that
shape at the builtin layer.

**Decision.** Values, not ambient authority: `rng_seed_u64(seed)
-> Rng` plus `rng_next(Rng) -> RngOut { value, state }`, threaded
explicitly like pool/breaker handles. Deterministic replay is the
acceptance bar (same seed → same sequence, pinned in tests). OS
entropy arrives no earlier than M6 with its own audit note, and
cryptographic randomness is never promised from this surface
(crypto RNG belongs to the Phase 6 crypto work, audited
separately).

**Non-goals (still deferred, explicitly):** a global `random()`,
float distributions, crypto-grade output, OS-entropy seeding.

**Implementation amendment — 2026-09-26 (Wave 0, as built).** The
Decision's named surface (`rng_seed_u64(seed) -> Rng`,
`rng_next(Rng) -> RngOut { value, state }`) was NOT built as written;
what landed is a flatter, still-explicit shape, recorded here because a
surface rename is exactly the kind of silent divergence the amendment
process exists to prevent:
- `rng_seed_builtin(seed: Int) -> Int` allocates an opaque handle from
  a per-run registry (the `DbRegistry` handle shape), and
  `rng_next_builtin(handle: Int) -> Int` advances it. The
  `{ value, state }` record was dropped: SplitMix64 keeps the state in
  the registry, so a returned struct would have been a second copy of
  state that could drift from the registry. The `Rng` *struct* still
  exists at the `.nv` layer (`stdlib/numbers/random.nv`) so the
  "values, not ambient authority" threading reads the same in user
  code — but it wraps only the `Int` handle, so the VM's
  struct-returning-call gap (FuncId::UNRESOLVED, noted in
  `tests/clock_rng.rs`) keeps the `Rng`-struct path interp-only. That
  is a documented test-scoping boundary, not a mirrored surface, and it
  is called out in the test file rather than hidden.
- Both runtimes implement one byte-identical SplitMix64 step
  (`interp::rng_next_u64` / `compiler::nir::vm::vm_rng_next_u64`),
  pinned by `tests/clock_rng.rs`; same seed → same sequence across
  `run` and `run-vm` is the acceptance bar and it is green.
- All non-goals hold: there is no global `random()`, no float
  distribution, no OS-entropy seeding, and no crypto claim anywhere in
  the surface or its docs. Unknown handles fail loudly in both
  runtimes (`unknown rng handle ...`).

### ADR-022 — Minimal String-Keyed Map Values (Phase 6/Wave 0)

**Status:** Proposed (paper first — unblocks route tables,
catalogs, headers, and JSON objects as values).

**The constraint.** `Map` values do not exist (SPEC.md C6); the
parallel-list workaround in libraries does not scale to route
tables or i18n catalogs, and the ownership story gates only
*in-place* mutation — not values (the `FieldSet` functional-update
precedent from P-001 applies: clone-on-write needs no borrowck).

**Decision.** A closed, minimal surface in the ADR-018 spirit:
string keys only (covers catalogs, headers, route tables, and
JSON objects — the actual M4/M5 use cases), functional update
semantics (`map_set` returns the new map, the input is observably
unchanged), builtins `map_new`/`map_get`/`map_set`/`map_len`/
`map_keys` over an insertion-ordered pair store. The surface reads
`Map<String, T>` with `T` monomorphic per map until generics land;
general key types and in-place mutation are deferred, never
precluded. Iteration order is insertion order, documented, not
guaranteed across versions.

**Non-goals (still deferred, explicitly):** non-String keys,
in-place mutation, a hashing API, concurrent maps, `Map` in
`derive` field position (revisit with ADR-018's successor).

### ADR-023 — TLS/Driver Vehicle Selection Criteria (Phase 6/Wave 0)

**Status:** Proposed as *criteria + shortlist* — the pick itself
locks via amendment here after a time-boxed Wave 1 spike, before
main implementation (hybrid per the Wave-planning record: decide
enough up front that parallel tracks stop guessing, learn enough
in the spike that the pick survives contact with packaging).

**What gets judged.** TLS: static-link story, license, client +
server API coverage, auditability of the binding, packaging
weight. Driver: wire-protocol completeness, transaction +
savepoint support, fit with the M5 executor story, packaging
weight. Both: the `unsafe` boundary review plan and the tier
labels they will carry in `noct audit`.

**Explicitly on the table.** The plan's "FFI-bound driver"
wording is an assumption, not a ruling: a pure-Rust crate
competitor is judged by the same criteria (no C toolchain and no
`unsafe` boundary to audit are real points in its favor). Same
for TLS (vetted-C vs audited-Rust). The spike must include a
static-binary packaging probe, since Wave 4 deployment is where
a C dependency would hurt most — that failure mode is what the
up-front criteria exist to catch early.

**Lock rule.** No implementation past the spike until the pick is
amended into this entry with the measured rationale. Unknowns
that reopen it: throughput shortfall under §5-style load,
static-link failure, license conflict.

**Lock amendment — 2026-09-26 (Wave 1 lock run): DEFERRED, with
sealed sub-decisions.** Verdict: **DEFERRED** — the measurements
below decide the rustls crypto provider and confirm offline
reproducibility, but the driver pick cannot lock without an
authenticated Postgres conformance run, and TLS cannot lock
without the handshake matrix. Nothing here is fabricated: every
cell is `[measured]` (observed in this run on Windows/MSVC,
scratch in `Temp\opencode\adr23-lock`, never in the repo) or
names its explicit blocker.

**Sealed (no re-measurement needed at the lock run):**
1. Criteria + shortlist stand as written above (TLS: rustls /
   mbedTLS-TF-PSA / OpenSSL / LibreSSL; driver: pure-Rust
   `postgres` blocking facade / libpq-FFI). Raw
   `tokio-postgres` async core stays rejected — see the ADR-009
   note.
2. Provider pick — **`ring`** (sealed, statically decided).
   `[measured]` `rustls 0.23.45` + `ring 0.17.14` clean-builds
   in ~146 s, links statically by default, and runs (9 cipher
   suites enumerated) via the already-proven `cc` path with
   `cl.exe` NOT on `PATH`. `[measured]` `aws-lc-rs 1.18.1`
   configures (finds only the VS-bundled CMake, which is NOT on
   `PATH`) but its full C++ source compile exceeded a 15-min
   budget without producing the lib; no cmake/perl/nasm on
   `PATH` in a plain shell. License is not the decider
   (`[measured]` all clean and MIT-compatible, no election:
   ring `Apache-2.0 AND ISC`, rustls `Apache-2.0 OR ISC OR
   MIT`, aws-lc-rs `ISC AND (Apache-2.0 OR ISC)`). Reopen only:
   a future aws-lc-rs recipe that builds hermetically inside the
   offline-build proof AND beats ring on binary size —
   packaging evidence, not opinion.
3. Offline reproducibility confirmed. `[measured]` this machine
   is ONLINE (`cargo search`/`fetch` succeed); every candidate
   resolves (`rustls 0.23.45`, `ring 0.17.14`, `aws-lc-rs
   1.18.1`, `postgres 0.19.14` over `tokio-postgres 0.7.18` +
   `tokio 1.53.1`) and is now in the local cargo cache, so the
   lock run re-verifies hermetically with `cargo build
   --offline`.
4. Driver build. `[measured]` `postgres 0.19.14` clean-builds
   (5m23s, pure Rust, zero system deps, zero new build tools)
   and links; licenses `MIT OR Apache-2.0` (both crates). Live
   queries NOT run — see shopping list item 1.
5. Postgres hunt. `[measured]` a test Postgres EXISTS
   (PostgreSQL 18, `localhost:64534` via `PGPORT`, `pg_isready`
   accepting, `scram-sha-256` everywhere, role `postgres`
   exists) but NO credentials are available in this session (no
   `PGUSER`/`PGPASSWORD`, password auth fails) — conformance is
   blocked on access, not existence. No simulation performed,
   no config touched.

**ADR-009 interpretation note (sealed).** The `postgres`
blocking facade's hidden internal tokio driver does NOT violate
"one official async runtime": `[knowledge]` it exposes no Tokio
types across the `.nv` boundary and every call joins on the
calling OS thread (the M4 blocking-threads floor) — structured
join at scope exit per PHASE5_PRODUCTION.md §3, no different in
kind from a C library spawning worker threads. Conditions:
(i) no Tokio handle/future crosses into `.nv` user code;
(ii) no background work outlives the call (no detach);
(iii) M5 re-opens only whether a native async surface is
added, never the M4 blocking use.

**Shopping list for the lock run (each names its blocker):**
1. Driver conformance vs a real Postgres (SCRAM, extended
   protocol, prepared statements, transactions + savepoints,
   pooled concurrent use, one `COPY` + one `LISTEN`/`NOTIFY`
   leg). Blocker: authenticated access to the PG18 instance
   above (or any test Postgres).
2. TLS handshake interop matrix (rustls+ring client+server
   against each other + one external peer) + Windows
   trust-store decision (bundled roots vs platform verifier)
   with `noct audit` tier/label treatment. Blocker: design
   time + verifier-crate API read.
3. Release binary-size deltas (rustls+ring vs any revived
   challenger). Blocker: items 1–2 first; mbedTLS still needs
   network + its own CMake proof.
4. M5-executor integration sketch for the facade (per the
   ADR-009 note §iii). Blocker: design time.
5. License texts pinned into `noct audit` rows (texts verified
   above; recording is lock-run paperwork).

**Hybrid rule (what Wave 1 implementation may start NOW).**
Stdlib scaffolding may proceed against `rustls 0.23 + ring`
(client+server API shape, error-to-`Result` mapping,
`native`-tier audit rows) and against the `postgres` blocking
facade (pool/borrow discipline per PHASE5_PRODUCTION.md §1,
transaction API as one designed unit) — both picks' build and
packaging evidence is in. What reopens: ANY shopping-list
failure, ANY static-link/hermetic-build failure, ANY license
conflict or unrecorded election, throughput shortfall under a
§5-style load harness, or any M5-executor integration needing
a second runtime (revives libpq and forces the ADR-009 note
open).

**Lock amendment — 2026-09-26 (Wave 1 lock-run follow-up): LOCKED.**
Verdict: **LOCKED** — every shopping-list leg measured PASS on
Windows/MSVC in `Temp\opencode\adr23-lockrun` (never in the repo),
instance `localhost:64535` from datadir `Temp\opencode\pglock`
(PostgreSQL 18.3, initdb UTF8/scram-sha-256, role `postgres`),
scratch DB `adr23lock`. Port 64534 untouched throughout. Sealed
sub-decisions stand (`ring` provider, ADR-009 note, offline cache).
`[measured]` below = observed this run; versions pinned:
`rustls 0.23.45`, `ring 0.17.14`, `postgres 0.19.14`
(over `tokio-postgres 0.7.18`), `rcgen 0.14.10`,
`webpki-roots 1.0.9`, `rustls-platform-verifier 0.7.1`.
1. Driver conformance (`postgres` sync facade, `pgconf` bin,
   start→all-legs→stop in one call, SCRAM auth): `[measured]`
   scram-connect PASS 550 ms; prepared-extended PASS 326 ms;
   txn-savepoint PASS 749 ms (ids=[1,2], rollback leg clean);
   concurrent-16 PASS 2708 ms, method=one-client-per-thread
   (16 independent connects, 16/16 ok — no hand-rolled pool);
   copy PASS 349 ms (count=2, out_bytes=16);
   listen-notify PASS 393 ms (adr23chan/ping123).
   TOTAL 5089 ms, failures=0.
2. TLS interop (`tlsmat`, ring-only
   `rustls = { version = "0.23.45", default-features = false,
   features = ["std","tls12","ring"] }`): `[measured]` cert-gen
   PASS 3 ms via `rcgen 0.14 generate_simple_self_signed`
   (localhost; cert_der 354 B, key_der 138 B; openssl CLI absent,
   not used); config-build PASS 1 ms; 9 cipher suites enumerated
   (TLS13_AES_256_GCM_SHA384, TLS13_AES_128_GCM_SHA256,
   TLS13_CHACHA20_POLY1305_SHA256 + 6 ECDHE ECDSA/RSA);
   handshake PASS 13 ms over 127.0.0.1 TCP, ping-tls/pong-tls
   both ways, negotiated TLS13_AES_256_GCM_SHA384 / TLSv1_3
   both ends. TOTAL 22 ms.
3. Trust-store INPUTS (`trustmat`): `[measured]` bundled API
   `webpki_roots::TLS_SERVER_ROOTS` len=121, deps only
   `rustls-pki-types` (lightest); platform-verifier APIs
   `ClientConfig::with_platform_verifier()` OK (9 suites) and
   `BuilderVerifierExt::with_platform_verifier()` with explicit
   ring provider OK; full `cargo tree -p
   rustls-platform-verifier` = log + rustls(ring, no aws-lc
   pulled) + windows-sys/windows-link, 27-line trustmat tree.
   Weight: trustmat 875008 B vs tls-only 698880 B (+176128 B
   for roots+verifier+windows-sys). Recommendation: platform
   verifier as the client default (OS store, live trust,
   enterprise CA/proxy, Windows-API revocation; rustls-team
   best-default opinion; deployed by 1Password/Bitwarden/Signal/
   rustup; works ring-only), bundled `webpki-roots` as opt-in
   for hermetic/container (deterministic, no OS dependency).
   Both `native` tier; `webpki-roots` license CDLA-Permissive-2.0
   recorded. Server side needs no store (presents cert).
4. Release size deltas (always-linked scratch bins, `x86_64-pc-
   windows-msvc` .exe): `[measured]` base 131072 B; tls-only
   698880 B (+567808 B); pg-only 1225216 B (+1094144 B);
   both 1788928 B (+1657856 B; shared-dep saving 4096 B vs sum
   of deltas). Full-harness binaries for reference: pgconf
   1556480 B, tlsmat 1877504 B (incl. rcgen), trustmat 875008 B.
5. Licenses (texts verified in cargo cache, `LICENSE*`
   present): `[measured]` ring 0.17.14 `Apache-2.0 AND ISC`;
   rustls 0.23.45 `Apache-2.0 OR ISC OR MIT`; postgres 0.19.14
   `MIT OR Apache-2.0` (tokio-postgres 0.7.18 same). No
   election needed. Exact `noct audit` rows (code untouched —
   text for implementation; `signed_by` = registry key at add
   time; format per `cmd_audit.rs` name/version/tier/signed_by/
   audit): `ring 0.17.14 native <key> signed (no vuln
   database) // license Apache-2.0 AND ISC`; `rustls 0.23.45
   native <key> signed (no vuln database) // license
   Apache-2.0 OR ISC OR MIT`; `postgres 0.19.14 native <key>
   signed (no vuln database) // license MIT OR Apache-2.0`.
   Trust-store rows when added: `webpki-roots 1.0.9 native
   <key> signed (no vuln database) // license
   CDLA-Permissive-2.0`; `rustls-platform-verifier 0.7.1 native
   <key> signed (no vuln database) // license MIT OR Apache-2.0`.
Reopen conditions (carry the hybrid list + spike §6
counter-argument): throughput shortfall under §5-style load;
ANY static-link/hermetic failure; ANY license conflict or
unrecorded election (incl. CDLA scope for bundled roots);
ANY protocol-conformance failure the C reference passes; ANY
M5-executor integration needing a second runtime (revives libpq,
forces ADR-009 note open); any enterprise mandate the pure-Rust
pair cannot meet (new PG auth method, FIPS module, OS TLS-policy
— the reference-lag hedge was correctly priced).
**Hybrid-rule revision.** The hybrid rule above is superseded:
full implementation is authorized against `rustls 0.23 + ring`
and the `postgres` blocking facade with the trust-store
recommendation in (3); scaffolding constraints become build
constraints (hermetic offline build from committed lockfile, no
new build tools, `native`-tier rows as in (5)). Nothing else in
this entry changes.

### ADR-024 — M5 Non-Blocking Executor (Phase 6)

**Status:** Proposed (merges `docs/EXECUTOR_DESIGN_DRAFT.md` §§1–5
as the normative executor record and resolves-or-carries its §6
items below; the draft itself stays untouched and non-normative).
On acceptance, ADR-009's "async fn and non-blocking executor are
Deferred to M5" resolves to this entry; ADR-009's own text is
amended separately, never silently rewritten here.
**The executor is NOT implemented.** A partial *runtime-side core* has
landed in `interp/src/lib.rs` ahead of acceptance — see
"Implementation status" at the end of this entry, which records exactly
what runs, what is orphaned, and which of this ADR's compatibility
promises it already contradicts.

**The constraint.** The M4 floor (CONCURRENCY.md §4,
PHASE5_PRODUCTION.md §§2–3) is one OS thread per task/connection:
correct under the 16×128 load harness but bounded by the OS, not
the runtime — the unboundedness M5 exists to remove. ADR-009 locks
one official runtime, structured concurrency with no
fire-and-forget, and defers `async fn` plus the non-blocking
executor to M5. The executor must therefore retire
thread-per-connection *without* a dual-runtime interregnum, reuse
the already-validated NIR suspend/resume shape (NIR.md §5: block
splits, tag dispatch, `phi`-carried state, `early_return` — no new
`Instr`), keep every M4 program compiling with identical values,
and add cooperative cancellation plus non-blocking timers without
inventing preemptive kills or ambient authority (ADR-020/021).

**Decision.** Option A of the draft (fixed work-stealing pool +
single readiness event loop, std-only, zero new dependencies
under the ADR-023 bar), with the draft's §6 open items resolved
as follows (rationale per item; what is not resolved here is
carried explicitly below — never silently picked):

- Workers: default `available_parallelism` capped at 64
  (INITIAL default — Wave 2 measures under §5-style load and amends
  this number with evidence; the cap shape, not the value, is what
  is pinned here); override via `NOCT_WORKERS` env (positive int,
  loud error otherwise); a manifest key is deferred to the config
  track. Rationale: the 16×128 mix binds on SQLite-serialized
  writes long before 64 workers matter, so the ceiling only
  prevents oversubscription pathology on many-core CI; worker
  count stays recorded-not-thresholded engineering, never a gate.
- Blocking bridge + spill rule: fixed bridge pool, default cap =
  worker count (same initial-default status — measured in Wave 2);
  queue-full is a loud `Err` (never silent growth,
  never an implicit wait). Sizing rule (documented, not coded):
  keep bridge cap >= pool max so pool-exhaustion stays the binding
  backpressure signal. The bound is also the anti-deadlock story:
  N sync-awaits against a smaller bridge fail loud instead of
  wedging.
- Sync `await` stays a blocking-bridge join (draft R2 confirmed —
  CONCURRENCY.md §2's flagship awaits from sync `main`, so a
  loud-error redirect would break source compatibility).
  Anti-masking guardrail: bridge joins are bounded (previous
  bullet), bridge depth is a test-visible gauge, and the 30 s
  watchdog plus drain reporting still turn lifetime bugs into loud
  failures.
- Timers: tickless heap owned by the readiness thread (wake at
  next expiry, no fixed tick); `sleep(0)` yields — re-queues
  behind and is a cancellation checkpoint. Rationale: at-least
  admits both, but yield makes `sleep(0)` a usable cooperative
  yield with deterministic cancel semantics instead of a
  potential busy-spin.
- `select` v1: DEFERRED — timeout/hedge compose from oneshot +
  timers (AC1's hedge leg proves adequacy); reopened only on
  measured evidence, owned by the resilience-library track.
- Pool: `pool_checkout_wait(pool, timeout_ms)` added as the
  explicit opt-in waiting variant (suspends; timeout is
  `Err("pool checkout timed out")`-class, exact string pinned in
  tests; negative timeout is a loud `Err`); default
  `pool_checkout` and both pinned strings stay byte-for-byte.
- Handle ids: monotonic within a run, NEVER reused; stale,
  unknown, or double await/cancel is a loud error. Rationale:
  reuse would alias a new task onto a dead scope's id — silent
  wrong-task values, the exact failure the loud-errors discipline
  forbids. u64 space makes exhaustion a non-issue.
- RNG in executor: jitter for timeout/hedge MUST thread ADR-021
  explicit-seed `Rng` handles — no ambient source. Tests pin
  per-sequence replay (same seed → same jitter sequence, agreeing
  with the fake-clock oracle); exact cross-task interleaving order
  is NOT pinned (pinning it would overspecify the scheduler).

Preserved (argued, not assumed): one official runtime (no
`--executor=` flag period — a gated dual runtime is two
schedulers, rejected under R8); structured concurrency +
join-on-cancel (cancel sets flags; scope exit still joins every
child; a cancelled child reports a `cancelled`-class value
composable with `?`); task-local handle rules (`for_task` fresh
registries; bare `Db`/`PoolCheckout` across tasks stays a loud
error; cross-task sharing only via explicit sync types);
cancellation composes with join (no detach-by-cancel — a cancel
that detaches would be fire-and-forget under another name).

**Migration / compatibility contract.** Existing `task`/`await`
programs are SOURCE- and VALUE-compatible (same handles, same
single-use/unknown-handle errors, same drain-at-exit).
`tests/http_load.rs` is EXTENDED, not replaced: the M4 legs keep
passing except the two implementation-naming assertions
re-baselined to executor gauges (live/parked tasks, bridge depth,
multiplexed connections); timeout + hedge legs are added (AC1).
`for_task` registries are PRESERVED in phase 1; relaxation arrives
only through sync types. Explicitly breaking — and nothing else:
(a) thread-count observability re-baselined to task gauges;
(b) `sleep` overlap constants recalibrated (same structure);
(c) pool struct shape may gain queue-depth fields behind unchanged
strings; (d) `http_server_serve_loop` goes readiness-driven behind
an unchanged route-table surface. No syntax, manifest, lockfile,
registry, or other error-string change.

**Amendment 2026-09-26 (owner decision): the worker cap stays, and
spawn refusal joins the explicitly-breaking list.** Item (e) is added:
(e) `spawn_task` **refuses** past `executor_worker_count()` (default
`available_parallelism` capped at 64, `NOCT_WORKERS` override, loud on
a malformed value) with `task spawn refused: worker pool exhausted`.
A program that spawns more concurrent tasks than the machine has
workers therefore fails loudly where it previously ran. This is an
error-string change and is scoped as one, narrowly: the *only* new
error string in the migration is the spawn refusal, and it fires only
past the cap. The alternative — reverting the cap to preserve the
promise literally — was considered and rejected, because the cap is
what bounds the migration this ADR exists to perform. Recorded rather
than left as a silent contradiction between the cap and the
source/value-compatibility promise (see "Contradicts this ADR today"
below, item (1), now resolved).

**Acceptance criteria.** AC1 extended harness (16×128 floor +
timeout leg + hedge-with-proven-join leg; latency
recorded-not-thresholded; PHASE5 triple-zero: pool delta zero,
gauges to baseline, second cycle no growth). AC2 cancellation
tests (sleep-parked, channel-parked, scope-cancel-joins-all,
idempotent cancel-after-completion, unknown/double loud,
drain-reported un-awaited, sibling-unaffected). AC3
no-regression list (drain, pool strings + `Ok(1)`/`Ok(0)`,
loud-failure discipline, at-least sleep + negative panics,
binding-only SQL grep, file-backed integrity + `:memory:`
smoke). AC4 backend parity (loud `unknown-runtime-symbol` on
`run-vm`/`build` until lowered; differential
suspend/resume/phi coverage before native). AC5
zero-new-dependency audit. New pins: `cancelled`-class strings,
checkout-timeout strings, id monotonicity (no reuse observable
in a soak), `sleep(0)`-yields (returns control + observes a
pending cancel).

**Non-goals (still deferred, explicitly):** the draft §6
non-goal list in full — work-stealing tuning as a guarantee;
TLS/auth/rate-limit/HTTP-2/status-aware codes/ORM/metrics
export/`noct audit` enforcement/replication/driver pick (Phase 6
owners); preemptive kills, builtin deadlines, priorities,
pinning API, unbounded channels, reentrant mutexes, RwLock v1,
`select` v1; wall-clock/timezone/NTP/precision; VM/native
executor lowering in v1; cross native/managed sharing;
multi-statement transactions + eviction policy; timeouts/hedging
as anything but blessed libraries.

**Open items carried (owner decides; implementers MUST NOT
silently pick):** (i) native-backend port mechanism — link the
same Rust executor vs. port the protocol in native codegen —
owner: backend (`runtime-native`/Cranelift) track; constraint:
identical observable protocol (cancel strings, join-on-cancel,
gauge semantics once frozen) proven by the AC4 differential
suite; WASM (M6) assumes whatever the port settles. (ii) OTel
gauge names/timing — gauges stay test-only until Phase 6
observability lands — owner: Wave 4 observability track. (iii)
UI/readiness-loop convergence — owner: UI track (MEMORY_MODEL
RUNTIME owners); constraint: nothing in this ADR precludes
convergence. (iv) `select` reopen + worker-override manifest key
— owners: resilience-library / config tracks respectively.

**Implementation status (recorded 2026-09-26, integration audit).**
An audit of the Phase 6 waves found a partial M5 executor core already
in `interp/src/lib.rs` even though this entry is still Proposed and
PHASE5_PRODUCTION.md §6 lists "cancellation, spawn limits" as
deferred. Recording it here so the record is not silent:

- **Runs today (reachable from `.nv`):** the worker cap. `spawn_task`
  refuses loudly past `executor_worker_count()` (default
  `available_parallelism` capped at 64, `NOCT_WORKERS` override,
  loud on malformed values), and `join_task` admits through the
  bounded blocking bridge (`blocking_bridge_acquire`, cap = worker
  count, loud `Err` when full, 30 s watchdog). Gauges
  (`live_executor_tasks`, `peak_executor_tasks`, `bridge_depth`,
  `bridge_peak`) are `pub` with no test coverage.
- **Runs:** cooperative cancellation. The per-task
  `AtomicBool` flag, the `sleep` cancel checkpoint (including
  `sleep(0)`  `yield_now` + checkpoint), the pinned
  `task {id} cancelled` value, and the drain's cancelled-report path
  are all live, and the flag is now reachable from a user program:
  `task_cancel_builtin(id)` is a real builtin (typeck
  `(Int) -> Unit`, lowered to `Instr::TaskCancel`, implemented in the
  interpreter, and wrapped as `task::task_cancel` in
  `stdlib/concurrency/task.nv`). `await` on a cancelled task yields the
  same `Err(task {id} cancelled)` a failing body would, so `?`
  composes it unchanged. Cancelling a task that already finished is a
  no-op success; cancelling an unknown handle is a loud `E1002`.
  `TicklessTimerHeap` remains orphaned, and is deferred to Wave 2 by
  owner decision (below).
- **Deferred to Wave 2 by decision (2026-09-26), orphaned until then (no
  users, no tests):** `TicklessTimerHeap` and `timer_now_ms`. The heap
  is a complete, correct, FIFO-ordered tickless structure with an
  injectable clock - and nothing schedules into it. `timer_now_ms` is a
  process-anchored `Instant` millis clock that the heap was meant to be
  ordered on. The owner was asked to choose "surface or delete" and
  chose **defer**, so this is now a committed Wave 2 *entry* item
  rather than unowned debt: Wave 2 may not start until the heap is
  surfaced (with the readiness thread) or deleted. Surfacing it is not
  a small job - the honest version needs the single readiness thread
  that acts on a popped id, i.e. this ADR's "Timers" bullet, still
  Proposed. The shortcut of routing the M4 blocking `sleep_builtin`
  through the heap was rejected: it breaks the at-least timing contract
  the deadline-based poll deliberately provides, and adds
  process-global mutable timer state, which ADR-020/021 forbid. A
  `timer_schedule`-shaped builtin whose returned ids nothing consumes
  is exactly the silent no-op the invariants ban.
- **Contradicts this ADR today:** (1) ~~the migration contract's
  "existing `task`/`await` programs are SOURCE- and VALUE-compatible"
  does not cover a spawn *refusal* past the cap~~ — **RESOLVED
  2026-09-26:** the owner kept the cap and added the refusal to the
  explicitly-breaking list as item (e). The contract is amended, not
  the cap;
  (2) ~~`docs/PHASE5_PRODUCTION.md` §6 and `docs/CONCURRENCY.md` §4
  describe the cap/bridge as M5 design, not as shipped behavior~~ —
  **RESOLVED 2026-09-26:** both now carry an explicit shipped-vs-Wave-2
  split;
  (3) `sleep_builtin` gained cancel polling, which is an M4 timing
  path — the audit fixed a regression there (a naive elapsed counter
  accumulated the OS timer's overshoot and broke
  `tests/async_test.rs::tasks_overlap_in_wall_clock`), and the
  cancellation *mechanism* is reachable but not native-parity yet
  (the VM spawns no tasks and Cranelift refuses the instruction
  loudly), so the risk it added
  bought nothing.
- **Wave 2 entry checklist (was "owner action required"; the heap
   choice is now made — 2026-09-26):**
   1. ~~Cancellation surface~~ — **DONE** (`task_cancel_builtin` +
      typeck + VM arm + `.nv` wrapper, per the mirror rule; see the
      "Runs" entry above).
   2. ~~Reconcile the docs with what runs~~ — **DONE**;
      `PHASE5_PRODUCTION.md` §6 and `CONCURRENCY.md` §4 now carry an
      explicit shipped-vs-Wave-2 split (see below).
   3. `TicklessTimerHeap` — **decided: defer.** Carried into Wave 2 as
      a committed entry item (surfaced with the readiness thread, or
      deleted), not as silent debt.
   4. **Spawn-refusal-past-worker-cap resolved:** **Added to explicitly-breaking list.** The migration contract's "existing `task`/`await` programs are SOURCE- and VALUE-compatible" is amended: the worker-cap spawn refusal (`task spawn refused: worker pool exhausted`) is a new observable loud failure that M4 programs did not have. This is now documented in the explicitly-breaking list. The cap stays (default `available_parallelism` capped at 64, `NOCT_WORKERS` override). The explicitly-breaking list in this ADR is updated to include this item.

**Doc reconciliation (2026-09-26):** `PHASE5_PRODUCTION.md` §6 and `CONCURRENCY.md` §4 updated to carry an explicit "shipped vs. Wave 2" split. They now describe the cap/bridge as *shipped behavior* (not "M5 design") with a note that Wave 2 will add the readiness thread and timer heap. The `TicklessTimerHeap` remains orphaned (no users, no tests) and is carried into Wave 2 as a committed entry item.

### ADR-025 — `.nvir` / `.nvc` Artifact Extensions

**Status:** Proposed. Stage 1 (`.nvir` dumps) is **implemented**;
Stage 2 (compiled interface) and the `.nvc` producer are specified, not
built. This entry exists so the rationale has a home in the log, not
only in prose: as of this writing the two extensions appeared in
`STYLE_GUIDE.md` §6.7, `NIR.md` §7, and `DEPLOYMENT.md` §8 with no ADR
behind them, which is exactly the drift §1's "Confirmed requires an
ADR, never a silent rewrite" rule exists to prevent.

**Decides what was Open:** STYLE_GUIDE.md §6.7 named `.nvir` and
`.nvc` as reserved extensions and said what they must *not* do (not
compete visually with `.nv`, no editing affordances), but never said
what they *are*. `.gitignore` listed both under "compiler artifacts
(future)". Nothing owned them.

**Decision:**

- **`.nvc` — linkable compiled-package artifact.** The unit a
  dependency ships when its `.nv` stays private. Executables remain
  the only runnable product: `noct build` keeps emitting the platform
  binary, and `.nvc` is linked into it, never substituted for it. The
  working parallel is Flutter's `app.so` — one compiled module per
  target, found in build outputs, consumed at package/run time rather
  than compiled from source at use time. Proposed location
  `.noct/build/<target-triple>/<name>.nvc`, which is toolchain-owned,
  gitignored, and structurally unable to leak into a published source
  tarball (`noct publish` collects `*.nv` plus the manifest).
- **`.nvir` — portable interface + IR, text first.** Stage 1 is the
  human-inspectable dump and build cache; `--emit-nir` ships it. Stage
  2 is the public-API-only compiled interface (signatures, exported
  types, mode tags, docs), hash-bound to the `.nvc` it was extracted
  with. Stage 2 is not optional garnish: consumers cannot compile
  against a closed package they cannot see into, so the interface half
  is what makes the artifact half usable — the `.cmi`/`.swiftmodule`/
  `.d.ts` role.
- **Both ride the `noctivue` language outright** (same grammar, same
  file identity, `.nv` analysis path) with exactly one server-side
  tweak: **no diagnostics**, because running `.nv` analysis over
  generated text yields false positives. This is how §6.7's "MUST NOT
  visually compete with `.nv`" is satisfied — by posture, not by a
  separate brand.
- **Emission is a leaf:** `--emit-nir` writes the dump and stops before
  staging, codegen, and linking, so IR inspection never depends on a
  native toolchain.

**Security posture, stated honestly.** Shipping compiled output instead
of source raises the bar against casual copying and gives integrity
hashes and signatures something to attach to. It is **deterrence, not
immunity**: native code disassembles and bytecode decompiles. No
document may promise reverse-engineering-proof output, and obfuscation
is explicitly out of scope for v1. A security-relevant claim that
cannot survive contact with a determined reverse engineer is worse than
no claim, because it is relied upon.

**Explicit non-conflation.** VM bytecode encoding is not NIR
serialization; a compact bytecode form is a separate format decision
with its own extension, never a second meaning loaded onto `.nvir`.
`.nvir` is never the distribution artifact.

**Cross-references:** STYLE_GUIDE.md §6.7 (roles + editor identity),
NIR.md §7 (the format and its two stages), DEPLOYMENT.md §8
(container sketch, producer, non-goals), TOOLCHAIN.md §3 (the store
this rides in), IMPLEMENTATION_PLAN.md Phase 7/M6 (the per-target
output matrix that governs `.nvc`).

**Milestones:** Stage 1 shipped with `noct build --emit-nir`. The
`.nvc` producer rides the direct Cranelift backend (M5 at the earliest,
DEPLOYMENT.md §8); the interface half rides with it, because shipping
one without the other delivers a package nobody can consume. No roadmap
surgery — both are naming/format decisions inside already-scheduled
backend work.

### ADR-026 — Dependency Content Store: Derived In-Project, Fetched Global

**Status:** Proposed. The end-state is decided; the move is scheduled
work, not a rewrite of the present layout.

**Decides what was Open:** TOOLCHAIN.md §3 fixed the manifest, lockfile,
and tiers, and ADR-017 fixed the *names*, but nothing said where
fetched dependency content lives. The implemented answer is
per-project: `.noct/cache/<name>-<version>.pkg` (raw archive bytes,
kept for hash re-verification) plus `.noct/packages/<name>-<version>/`
(the unpacked tree the compiler reads). So every dependency is stored
twice per project, and again in every other project that uses it.

**Decision — one sentence to settle every future "where does X go":**
*derived lives with the project, fetched lives global, and the lock is
the mapping between them.*

- **In-tree, permanently:** `nestpkg.nvpm` + `nestpkg.lock` (the
  mapping — the role `.dart_tool/package_config.json` plays in Flutter,
  except human-readable and checked in), `.noct/build/` outputs
  (including `.nvc`), and any resolution metadata.
- **Global, content-addressed:** `~/.noctivue/store/sha256/<ab>/<cdef…>/`
  for archive bytes and a parallel `extracted/` tree, keyed by the
  `content: sha256:…` hashes the lockfile already records. Identical
  bytes are stored once across projects *and* across versions. XDG-aware
  on Linux, per-OS homes elsewhere, with a `NOCT_STORE` env override
  following the existing `NOCT_KEYS` precedent.
- **Both halves are kept, shared rather than collapsed.** The archive is
  the unit of verification and transfer (one streaming hash pass); the
  extracted tree is the unit of compilation and human inspection.
  Every mature ecosystem keeps the pair (Cargo `.crate` + `src/`, Go
  zips + extracted trees, pub archives + extracted dirs). The waste was
  never archive-plus-extracted — it was per-project. Collapsing to one
  half would save a fraction of that while destroying either cheap
  verification or direct readability.
- **Cadence:** hash the archive once at fetch; extract atomically
  (temp, verify, move into place — pub's discipline) and mark
  read-only (Go's `-modcacherw` exists precisely because read-only is
  the default there, and it is what makes silent drift from the lock
  impossible). An immutable store has no repair path by design:
  corruption is answered by re-fetching loudly, not by patching bytes.
  `--frozen` and the manifest gate stay metadata-only (lock-vs-mapping),
  never a per-build re-hash.
- **`vendor/` remains** the committed, offline, air-gapped escape hatch
  and bypasses the store entirely. A missing store entry with no network
  fails loudly naming the package.
- **No reference counting, no auto-GC.** A manual `noct clean` first;
  LRU pruning later if measured. Cargo only recently grew cache GC after
  a decade without it.

**Milestones:** M5 (same window as signing/audit enforcement returns —
TOOLCHAIN.md §3, and an M5 exit criterion in IMPLEMENTATION_PLAN.md).
The move is deliberately gated on scale, not principle: while the
registry is young the duplication is unnoticeable and hermetic
per-project trees are genuinely easier to debug. Two cheap integrity
wins that require no relocation may land earlier — read-only unpack,
and editor excludes so the LSP does not index fetched trees.

**Cross-references:** TOOLCHAIN.md §3 (the end-state bullet), STYLE_GUIDE.md
§6.7 (why `.nvc` outputs stay in-tree while fetched bytes do not),
DEPLOYMENT.md §§1–2 (reproducible builds; the runtime image carries no
`.noct/`), IMPLEMENTATION_PLAN.md Phase 6/M5 (exit criterion).

## 3. Technical Consistency Review

This section is a deliberately critical pass over the design as
specified in v0.1. It is not exhaustive, and the "Recommended solution"
column reflects a working proposal, not a final ruling.

---

**Issue 1 — Colon-first parsing ambiguity between declarations and control flow**
- *Problem:* `Name:` and `if condition:` / `match x:` both use a
  trailing colon to introduce an indented block. The parser must not
  rely on capitalization or naming convention to disambiguate
  declaration-introducing colons from control-flow colons, since that
  would make casing load-bearing for parsing rather than for style.
- *Why it matters:* If disambiguation silently depends on identifier
  casing, then a lowercase type name or an uppercase local binding
  becomes a parse-breaking change disguised as a style violation.
- *Severity:* High — this is foundational to the entire grammar.
- *Possible solutions:* (a) reserve control-flow keywords (`if`, `for`,
  `while`, `match`, etc.) so any bare leading keyword unambiguously
  starts a control construct, and treat *every other* `Identifier(...)? :`
  at statement/declaration position as a declaration — a struct,
  function, component, or module, disambiguated later during name
  resolution, not during parsing; (b) require an explicit keyword
  (`struct`, `fn`) whenever ambiguity could arise. (a) preserves the
  concise style; (b) undermines ADR-007.
- *Recommended solution:* (a). The **parser** only needs to distinguish
  "reserved control-flow keyword at head of statement" from "everything
  else," which is a purely syntactic (keyword-table) check, not a
  semantic guess. Whether a non-keyword-headed colon block is a struct,
  function, or component is resolved during name resolution using
  arity/shape (parameter list vs. field list vs. nested-call body), not
  during parsing. See COMPILER_ARCHITECTURE.md §4 for the full worked
  algorithm.
- *Status:* Proposed — Phase 0 hand-simulation complete (see Phase 0
  Validation Note below Issue 2). No remaining unhandled ambiguous cases
  found. Ready for Phase 1 implementation. Promotion to Confirmed pending
  a running prototype (Phase 1 exit criterion).

---

**Issue 2 — `struct`-less type declarations vs. UI components look identical**
- *Problem:* `User:` (a struct) and `Dashboard:` (a UI component) are
  syntactically identical at the grammar level. UI components are not a
  core-language concept (ADR: UI is "an ordinary package," §22 of the
  design brief) — so the compiler frontend cannot know `Dashboard:` is
  special.
- *Why it matters:* If the framework needs compiler cooperation to treat
  component bodies differently (e.g., implicit widget-tree construction
  from nested calls), that contradicts "ordinary package, not privileged
  compiler functionality" (ADR-005).
  a plain struct-with-body).
- *Severity:* Medium — affects the UI framework's implementability as a
  pure library.
- *Possible solutions:* (a) Components genuinely are just structs/values
  built via ordinary struct-literal-like syntax, and "the widget tree"
  is a runtime data structure assembled by ordinary function calls
  inside the body — no special compiler case needed; (b) give the
  compiler a narrow, general "trailing block is sugar for a builder
  closure" desugaring that any library can hook, not just UI.
- *Recommended solution:* (b), generalized as a language-level rule
  (not UI-specific): a colon-block whose declared name resolves to a
  *function or macro* rather than a *type* desugars to "call this
  function, passing a closure that emits successive statements in the
  block as calls into an implicit builder." This keeps the UI framework
  an ordinary package while explaining the desugaring generally. This
  needs a working prototype before being promoted from Proposed to
  Confirmed.
- *Status:* Open — flagged as the single highest-risk syntax question
  in the spec; must be resolved before M1.

---

**Phase 0 Validation Note — Issues 1 & 2 (recorded 2026-08-31)**

The disambiguation algorithm from COMPILER_ARCHITECTURE.md §4 was
hand-simulated against the full `tests/fixtures/ambiguous_decls/`
fixture set during Phase 0. Findings:

**The algorithm holds**, with one important clarification and one
decided edge case.

*Clarification — what creates a classification dependency:*

Field type references (`identifier: type_expr` in a struct body, e.g.
`next: Node`) do **NOT** create a classification dependency on the
referenced type. They merely look up the name as a type during name
resolution — the referenced declaration's *kind* (struct vs function vs
component) is irrelevant to classifying the current declaration. This
means the original `cycle_self`, `cycle_direct`, and `cycle_indirect`
fixtures were wrong: they used mutual field-type references, which
produce two or three independent struct classifications, not a
classification cycle.

A genuine classification dependency exists **only** when a bare
declaration's body contains a **call expression** whose callee's kind is
not yet known — specifically, the component rule requires knowing whether
the callee resolves to a function/macro or a type, and that requires
classifying the callee. The three cycle fixtures were corrected to use
mutual call expressions (e.g., `Ping: { Pong() }` / `Pong: { Ping() }`)
which do create the mutual dependency the algorithm is designed to detect.
All three cycle shapes (self-reference A→A, direct A→B→A, indirect
A→B→C→A) produce E0010 as specified.

*Edge case decided — empty body:*

A bare declaration with an empty body (no fields, no statements, no
return type, no parameters) vacuously satisfies the struct rule
("exclusively field declarations with no calls" — zero fields is still
exclusively fields). **Decision: classify as struct.**

Rationale: consistent with ADR-008 (concise and explicit forms produce
the same AST — `Empty:` and `struct Empty:` must be equivalent). A
zero-field struct is a valid unit-like type. The alternative (error,
require explicit keyword) adds friction with no safety benefit.

*`component_vs_function` golden corrected:*

The original golden for `component_vs_function.nv` incorrectly said
`Splash` classifies as a function. Correct outcome: `Splash`'s body
contains `loading_screen()`, a call. `loading_screen` has a return type
(`-> Screen`) and classifies as a function independently. Therefore
`Splash`'s body-call resolves to a function/macro, triggering the
component rule. `Splash` is a **component**, not a function.

*New fixture added:*

`function_no_return_type.nv` was added to cover the body-statement
classification path (no `->` annotation, classified function by presence
of `let`/`return`/`var`/expression statements). The original
`function_minimal.nv` only covered the return-type-present path.

*Algorithm status after Phase 0:* **Proposed → ready for Phase 1
implementation.** No case falls through to undefined behaviour. Issue 1's
recommended solution (a) holds. Issue 2 still requires a working
prototype (the component/builder desugaring rule) before promotion to
Confirmed — that remains the highest-risk open question for Phase 1.

---

**Issue 3 — Memory-mode boundary crossing (native ↔ managed)**
- *Problem:* The spec confirms two modes sharing one type system, but
  does not yet define what happens when a native-mode (owned/borrowed)
  value is referenced from managed (ARC) code, or vice versa — e.g.
  passing a native `struct` into a managed UI component's state.
- *Why it matters:* Without a defined boundary, either mode's safety
  guarantees can leak or break at the seam (e.g., a borrowed reference
  outliving its native scope while held by an ARC-managed closure).
- *Severity:* High — a memory-safety-relevant gap, not just an ergonomic one.
- *Possible solutions:* (a) require explicit conversion/wrapping at the
  boundary (e.g., a native value must be copied or explicitly "adopted"
  into managed mode); (b) disallow direct sharing — native and managed
  values are always distinct types, with FFI-like boundary functions;
  (c) allow read-only borrowing across the boundary with compiler-enforced
  scoping.
- *Recommended solution:* (b) for v0.1 — the two modes' value graphs stay
  disjoint, communicating only through explicit conversion APIs (analogous
  to the C-ABI boundary already required for FFI). This is the safest
  starting point and can be relaxed later once (a)/(c) are prototyped.
- *Status:* Open — no implementation should begin on cross-mode data
  sharing until this is promoted out of Open.

---

**Issue 4 — ARC reference cycles in UI state (weak-by-default risk)**
- *Problem:* The brief asks to "explore weak-by-default closure capture"
  for UI. Weak-by-default capture is a footgun: values can be
  deallocated out from under a closure unexpectedly, producing
  `None`/null-like failures at a distance.
- *Why it matters:* This would reintroduce a null-like failure mode into
  a language whose stated goal is "no ordinary null."
- *Severity:* Medium-High, ergonomics/safety tradeoff.
- *Possible solutions:* (a) weak-by-default in closures; (b)
  strong-by-default with a dedicated, compiler-recognized "view/state"
  ownership pattern (parent strongly owns children, children hold weak
  parent pointers, enforced by the widget-tree runtime rather than by
  a general language rule); (c) require explicit `weak`/`unowned` always,
  no default.
- *Recommended solution:* (b), scoped narrowly to the UI runtime's
  parent/child ownership discipline rather than a general language
  default — avoids the null-like footgun of (a) while not burdening all
  managed-mode code with (c)'s verbosity.
- *Status:* Open — explicitly tracked as the primary UI-S0/UI-S1 research
  question (ROADMAP.md).

---

**Issue 5 — Semicolons as "compression only" vs. formatter round-tripping**
- *Problem:* Semicolons are optional and meant purely as a compaction
  device, but the formatter is also required to losslessly convert
  between compact and expanded forms. If the formatter always expands
  semicolon-joined statements onto separate lines, semicolons become
  purely transient/never-persisted — which is fine, but must be stated
  explicitly or authors will treat semicolons as meaningful style.
- *Why it matters:* Ambiguous formatter behavior around semicolons could
  fragment style the way the spec explicitly tries to avoid (one
  official formatter, ADR-010's sibling goal).
- *Severity:* Low.
- *Possible solutions:* (a) formatter always removes semicolons except
  where line-length/density heuristics choose to compact; (b) formatter
  never removes user-chosen semicolons.
- *Recommended solution:* (a) — the formatter treats semicolons as a
  formatting decision it owns, consistent with "formatting belongs to
  the formatter" (design brief §8).
- *Status:* Confirmed.
- **Confirmation (2026-09-17):** Recommended solution (a) confirmed —
  the formatter owns semicolon placement; semicolons are a formatting
  decision, never persisted as author style. Original Proposed text
  retained above per §1 (no silent rewrites).

---

**Issue 6 — NIR must carry memory-mode metadata without becoming two IRs**
- *Problem:* Preserving native vs. managed information "in the IR
  rather than erased prematurely" risks a de facto IR fork if not
  designed carefully (native lowering rules vs. ARC-insertion rules
  diverge quickly).
- *Why it matters:* An accidental two-IR system reintroduces the
  "two languages" problem ADR-001 is explicitly trying to avoid, one
  level down the stack.
- *Severity:* Medium — an implementation-engineering risk, not a
  user-facing one, but expensive to fix late.
- *Possible solutions:* (a) single NIR type/instruction set with a
  per-value/per-function mode tag consulted only during lowering; (b)
  fully separate NIR dialects per mode.
- *Recommended solution:* (a). See NIR.md §2.
- *Status:* Proposed — to be validated once M1 begins.

---

**Issue 7 — Ecosystem fragmentation risk in "explicit keyword" equivalence**
- *Problem:* `User:` and `struct User:` being fully equivalent means
  linters/style guides across teams could diverge (some codebases ban
  concise form, others ban explicit form), which is exactly the kind of
  fragmentation Principle 9 warns against — just moved from the
  language into linter configuration.
- *Why it matters:* Undermines "recognizable immediately from its source
  code" if large codebases look inconsistent from project to project.
- *Severity:* Low-Medium, ecosystem/social risk rather than technical.
- *Possible solutions:* (a) leave it to team style guides, no
  opinion; (b) official style guide states a house recommendation
  (concise by default, explicit only when disambiguation genuinely
  helps) that `noct fmt`/`noct lint` can optionally enforce.
- *Recommended solution:* (b) — ship a *default-off* lint rule so teams
  can opt in to consistency without the core language mandating it.
- *Status:* Confirmed. See STYLE_GUIDE.md §2.
- **Confirmation (2026-09-17):** Recommended solution (b) confirmed —
  the official house recommendation (concise by default, explicit only
  when it aids disambiguation) is enforced ONLY via a default-off lint
  rule, never by the core language or default tooling. Original
  Proposed text retained above per §1 (no silent rewrites).

---

## 4. Open Design Questions (rollup)

- Exact managed-mode opt-in syntax (`managed Name:` or otherwise)
- Exact ARC-cycle mitigation mechanism for UI state
- Precise UI state/ownership model
- Exact NIR instruction set
- Native-mode lifetime/borrow syntax
- Package manifest file format
- Macro/metaprogramming system (whether one exists at all)
- Capability-based security model
- Optional chaining semantics
- Reflection support, if any
- Self-hosting strategy (whether the compiler is ever rewritten in Noctivue)
- Mobile/web rendering architecture for the managed/UI runtime
- Cross-mode (native↔managed) value sharing (Issue 3 above)
- Colon-block declaration-kind resolution algorithm validation (Issue 1/2 above)

None of the above should be treated as decided by omission elsewhere in
this documentation set.

---

### ADR-026 — Panic/Unwind Semantics for Native Mode

**Status:** Accepted (2026-09-26).

**Problem:** ERROR_HANDLING.md §4 and NIR.md §6 left native-mode panic behavior as "Deferred" — specifically, whether a panic aborts the process or unwinds to a catch boundary. This became load-bearing once Phase 6 shipped real I/O (Postgres, TLS) with FFI boundaries that could panic.

**Decision:**
- **Native mode: abort.** A panic in native mode calls `std::process::abort` (or platform equivalent). No unwinding, no cross-FFI exception propagation, no hidden control flow. This matches the "no hidden control flow" principle (Principle 9) and avoids the complexity of unwinding across FFI boundaries.
- **Managed mode: catch at boundary.** The UI runtime's event loop catches panics and surfaces them as structured errors (e.g., `task {id} panicked`). This is an implementation detail of the managed runtime, not a language feature.

**Rationale:** Unwinding across FFI (especially C/Rust/C++ boundaries) is fragile and platform-dependent. Aborting is deterministic, simple, and forces the programmer to use `Result` for recoverable errors. The "unwind to a defined boundary" option was considered but rejected because it implies a runtime mechanism that doesn't exist in native mode and adds complexity without clear benefit.

**Impact:** 
- `panic` builtin in native mode → process abort.
- NIR `Panic` instruction lowers to abort in Cranelift.
- `panic`/`assert`/`to_int` builtins in NIR VM → trap (abort).
- No try/catch syntax added to the language.
- ERROR_HANDLING.md §4 updated to reflect this decision.

**Related:** NIR.md §6 (panic/assert not representable in NIR yet — now explicitly lowers to abort), FFI.md §6 (FFI boundary discipline: no exceptions crossing).
