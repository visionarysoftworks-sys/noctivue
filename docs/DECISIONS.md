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
**Status:** Confirmed (model) / **Open** (exact channel/sync-primitive
API surface). One official async runtime; no competing executors in the
core ecosystem. See CONCURRENCY.md.

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
- *Status:* Proposed.

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
- *Status:* Proposed. See STYLE_GUIDE.md §2.

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
