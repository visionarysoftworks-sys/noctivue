# EXECUTION_GUIDE.md — Phase-by-Phase Execution Guide

**Status:** Proposed. This is the operational companion to
IMPLEMENTATION_PLAN.md. That document says *what order* and *what exit
criteria*; this document says *how to execute each phase without
introducing the specific mistakes this project is most exposed to*, and
*what concrete artifact "done" looks like*. Read it phase by phase, in
order — each section assumes the previous phase's outcome exists.

Every phase section below has the same four parts:

- **Purpose** — why this phase exists, in one paragraph.
- **Precision Points** — the specific things that are easy to get
  subtly wrong, and what wrong looks like if you don't catch it.
- **Common Mistakes to Avoid** — failure modes seen in comparable
  language projects, named explicitly so you recognize them if you
  start drifting toward one.
- **Definition of Done (Outcome)** — the concrete, checkable artifact
  that phase must produce. Not a feeling of readiness — an artifact.

---

## Phase 0 — De-risking the Core Syntax Question

### Purpose
Everything downstream — the parser, the resolver, the AI-tooling JSON
schemas, even the formatter — depends on one unproven assumption: that
a bare `Foo:` declaration can be classified as struct/function/
component *after* parsing, using only body shape, without ever
consulting naming convention. If this assumption is wrong, it's wrong
in a way that's expensive to discover in Phase 1, because by then a
parser and resolver already exist that assume it's true.

### Precision Points
- **Write the fixtures before touching the algorithm, not after.**
  It's tempting to write the classification algorithm first and then
  invent test cases that confirm it works. Write the adversarial cases
  cold, from the grammar alone, before you've committed to an
  implementation approach — otherwise you'll unconsciously avoid cases
  that break your preferred design.
- **The transitive-resolution-order problem is real, not decorative.**
  COMPILER_ARCHITECTURE.md §4 proposes a "declaration classification
  cycle" compile error for cases like mutually-referential bare
  declarations. Don't treat this as an edge case to handle later —
  construct at least three genuinely different cycle shapes (direct
  A→B→A, indirect A→B→C→A, and self-reference A→A) and confirm each
  produces the *same* well-defined error, not three different failure
  modes.
- **Distinguish "the algorithm rejects this input" from "the algorithm
  crashes/loops on this input."** A classification cycle should be
  *detected* and reported, never cause unbounded recursion in the
  resolver. If your prototype infinite-loops on a cycle case, that's
  not a minor bug — it means the resolution order isn't actually
  well-founded, and the algorithm needs redesign, not a guard clause
  bolted on.
- **Test the boundary between "structurally a struct" and "structurally
  a function" carefully.** A struct with zero fields and a function
  with zero statements can look identical after whitespace is
  stripped. Confirm your classification rule handles the *empty body*
  case explicitly, not by falling through to a default.

### Common Mistakes to Avoid
- **Silently reaching for identifier casing as a tiebreaker "just this
  once."** The moment casing becomes load-bearing for even one
  disambiguation case, LANGUAGE_SPEC.md §4's promise that casing is
  "a formatter/linter recommendation, not a compiler-enforced rule"
  is broken, and it will be broken in a way that's invisible until
  someone names a type in lowercase and gets a confusing error.
- **Treating this phase as "design on paper, move on."** The exit
  criterion is fixtures that pass through an actual (even throwaway)
  implementation of the algorithm — not a design document asserting
  the algorithm would probably work.
- **Scoping the fixture set too narrowly to the dashboard example.**
  `examples/dashboard.nv` was written to demonstrate style, not to
  stress-test disambiguation. Don't let it stand in for adversarial
  coverage.

### Definition of Done (Outcome)
A `tests/fixtures/ambiguous_decls/` directory containing:
1. At least one fixture per classification outcome (struct, function,
   component) using the *minimal* syntax that could plausibly be
   confused with another outcome.
2. At least three distinct cycle-shape fixtures, each producing the
   defined "declaration classification cycle" diagnostic — verified
   against a real (even if throwaway) implementation, not asserted by
   hand.
3. A short written note (append to DECISIONS.md Issue 1/2, don't create
   a new file) recording whether the algorithm as specified held up,
   or what had to change. If it changed, that's an ADR amendment, not
   a silent edit.

**You may not proceed to Phase 1 without this artifact existing.** This
is the one phase in the whole plan where skipping ahead is explicitly
disallowed, because the cost of discovering a flaw here is one afternoon;
the cost of discovering it after Phase 1's parser is built around the
wrong assumption is a parser rewrite.

---

## Phase 1 — M0: Core Language + Interpreter

### Purpose
Prove the language exists as a runnable thing, end to end, on the
smallest possible feature surface. Everything excluded here (ROADMAP.md
§1: UI, async, ARC, native codegen, macros) is excluded specifically so
that the first full pipeline pass — lex → parse → resolve → typecheck →
interpret — happens against a small enough surface that bugs are
attributable to a specific stage, not lost in feature interaction.

### 1.1 Lexer

**Precision Points**
- The INDENT/DEDENT offside-rule pass (SYNTAX.md §10.1) is the single
  highest-leverage piece of correctness in the entire frontend, because
  every later stage trusts its output implicitly. Write fixtures for:
  consistent 4-space indentation, tab/space mixing *within one block*
  (must be a hard error, LANGUAGE_SPEC.md §2), a dedent that doesn't
  match any enclosing level (must be a hard error, not a best-effort
  snap-to-nearest), and blank lines / comment-only lines inside an
  indented block (must not affect indentation tracking).
- Confirm brace-delimited blocks genuinely suspend indentation
  sensitivity for their *entire* contents, including nested indented
  code inside a brace block that itself contains a colon-block
  (SYNTAX.md §6/§7 interaction — this nesting case is not explicitly
  worked out in SYNTAX.md and deserves its own fixture).
- String interpolation (`"{expr}"`) requires the lexer to re-enter
  expression tokenization mid-string. Get this working with a nested
  interpolated expression containing a string literal itself
  (`"outer {f("inner")} done"`) before considering the lexer done —
  this is a classic place for a hand-rolled lexer to silently break.

**Common Mistakes to Avoid**
- Treating indentation errors as warnings or auto-correcting them
  ("assume the user meant 4 spaces"). LANGUAGE_SPEC.md §2 is explicit:
  inconsistent indentation **MUST** produce a compiler error. A
  forgiving lexer here creates programs whose meaning depends on which
  version of the toolchain compiled them.
- Under-testing Unicode identifiers early and discovering XID_Start/
  XID_Continue edge cases (e.g., emoji, combining characters) only once
  real users hit them. Test at least one non-Latin identifier
  (LANGUAGE_SPEC.md §1 gives you `café`/`привет` for free) in Phase 1,
  not later.

**Definition of Done (Outcome)**
A lexer crate/module that: tokenizes every fixture in
`tests/fixtures/` correctly, including the Phase 0 ambiguous-declaration
fixtures at the token level; rejects every deliberately-malformed
indentation fixture with a specific, non-generic diagnostic; and has a
golden test for the nested-interpolation and brace-suspends-indentation
cases called out above.

### 1.2 Parser

**Precision Points**
- Build the parser to produce `bare_decl` nodes exactly as
  COMPILER_ARCHITECTURE.md §4 describes — capturing identifier,
  optional params, optional return type, and block body, *without*
  attempting classification. Resist adding a "shortcut" where the
  parser classifies obvious cases (e.g., "a body of pure field
  declarations is obviously a struct, just tag it now") — this creates
  two classification code paths (one in the parser, one in the
  resolver) that can silently drift apart. One classifier, one place,
  per Phase 0's validated algorithm.
- Validate the grammar against every operator-precedence case in
  SYNTAX.md §9 explicitly, especially the interaction between `?`
  (postfix, precedence 11) and `??` (infix, precedence 10) — these are
  adjacent precedence levels with different fixities and are a common
  source of parser bugs (e.g., `a?？b` parsing ambiguity in the
  written grammar should be checked by hand against a worked example).
- Named arguments (`arg = [ identifier , ":" , expression ]`) share
  syntax with field declarations and struct-literal-style init. Confirm
  the parser distinguishes call-site named arguments from block-body
  field/state declarations using position (inside `(...)` vs. inside an
  indented/braced block), not by any semantic lookahead.

**Common Mistakes to Avoid**
- Writing a PEG-with-backtracking parser "just to get past" a
  seemingly-ambiguous case instead of confirming the grammar in
  SYNTAX.md §10 is genuinely context-free as claimed. If you find
  yourself reaching for backtracking, that's a signal the grammar
  itself needs revision (an ADR-worthy change), not that backtracking
  is an acceptable implementation detail to paper over it.
- Skipping error recovery ("just stop at the first parse error").
  Even a minimal recovery strategy (e.g., skip to next newline/DEDENT
  at the current indentation level) is worth having by the end of
  Phase 1, because `noct diagnostics --json` is expected to report
  more than one error per run in real usage, and retrofitting recovery
  later touches every parser function.

**Definition of Done (Outcome)**
A parser producing a documented AST (including un-classified
`bare_decl` nodes) for every fixture in `tests/fixtures/`, with a
golden AST dump per fixture under `tests/golden/`. At least one
multi-error fixture demonstrating basic recovery (two independent
syntax errors in one file both reported, not just the first).

### 1.3 AST + Name Resolution

**Precision Points**
- This is where the Phase 0-validated classification algorithm gets
  its real implementation (`resolver/classify.rs`). Re-run every Phase
  0 fixture against this real implementation as a regression suite —
  don't treat Phase 0's throwaway prototype as sufficient forever.
- Scope construction must correctly handle the fact that `export`ed
  top-level declarations are visible to importers while everything else
  is module-private by default (MODULES.md §3). Test a fixture that
  deliberately imports a non-exported item and confirms a specific
  "not exported" diagnostic, distinct from a generic "not found" one —
  these are different problems for a user and deserve different
  messages.
- Confirm `import path::Item` vs. `import path` populate the resolver's
  scope table differently (item-level binding vs. module-namespace
  binding) and that a subsequent unqualified use of `Item` only
  resolves correctly in the former case.

**Common Mistakes to Avoid**
- Resolving names and classifying declarations in two separate passes
  that can observe each other's *partial* state inconsistently (e.g.,
  classification depends on a symbol whose own classification hasn't
  completed yet, but the resolver doesn't detect this as the "cycle"
  case from Phase 0 — it just reads stale/default state). If
  classification and resolution are interdependent, make the
  dependency explicit and detect cycles deliberately; don't let it
  happen by accident of pass ordering.
- Silently allowing shadowing that the spec doesn't explicitly bless.
  Decide and document (even informally, in a code comment referencing
  this section) whether inner-scope shadowing of an outer binding is
  allowed before it comes up in a real fixture and gets decided
  by accident.

**Definition of Done (Outcome)**
Every fixture resolves with correct scope/export visibility; the full
Phase 0 fixture set passes through the real classifier with identical
results to the Phase 0 prototype (any divergence is investigated and
recorded, not silently accepted because "the real one is probably
right").

### 1.4 Basic Type Checker

**Precision Points**
- Confirm local-only inference genuinely stops at function boundaries
  (TYPE_SYSTEM.md §13) — construct a fixture where two functions each
  have unannotated-would-be-inferrable-together types and confirm the
  checker requires the boundary annotation rather than opportunistically
  inferring across it. This is easy to get "accidentally right" with a
  simple checker and then accidentally wrong later when someone
  optimizes it.
- `Option<T>`/`Result<T, E>` and `?` propagation (ERROR_HANDLING.md §2)
  need their own careful fixture: a function using `?` whose return
  type is *not* a compatible `Result`/`Option` must be a clear, specific
  type error at the `?` site, not a generic type-mismatch error
  reported somewhere else in the function.
- `match` exhaustiveness (TYPE_SYSTEM.md §4) needs a fixture per enum
  shape: a `match` missing one variant (must error), a `match` missing
  a variant but with a `_` arm (must not error), and a `match` with an
  unreachable arm after a catch-all (should ideally warn, though this
  can be deferred — decide explicitly and record which).

**Common Mistakes to Avoid**
- Implementing exhaustiveness checking as "does every variant *name*
  appear somewhere," which breaks the moment nested/guarded patterns
  are involved (`match_arm` includes an optional `if` guard per
  SYNTAX.md §10 — a guarded arm does not make a variant exhaustively
  covered). Get this right in Phase 1 with the small pattern grammar
  that exists now; it only gets harder to retrofit as patterns grow.
- Treating type-check errors and borrow/ownership errors as the same
  category. They aren't (native-mode ownership enforcement is
  explicitly deferred to Phase 3 per MEMORY_MODEL.md/IMPLEMENTATION_PLAN.md
  §5) — keep the diagnostic categories cleanly separated from the start
  so Phase 3 can add a new category without restructuring existing
  ones.

**Definition of Done (Outcome)**
A type checker that accepts every valid fixture and rejects every
deliberately type-invalid fixture with a diagnostic that names the
specific rule violated (return-type mismatch, non-exhaustive match,
invalid `?` usage, etc.) — not a generic "type error."

### 1.5 Tree-Walking Interpreter

**Precision Points**
- Since native-mode ownership enforcement is deferred to Phase 3, the
  Phase 1 interpreter is not yet enforcing move/borrow semantics at
  runtime. Be explicit (in code comments and in a short note in this
  document's fixture set) about which memory-safety guarantees are
  *not yet real* in M0, so nobody mistakes "the interpreter didn't
  crash" for "ownership is enforced."
- Confirm `Result`/`Option` values interpret correctly through nested
  `match` and `?` combinations — this is the most-exercised control-flow
  path in real Noctivue code per the dashboard example, and subtle
  interpreter bugs here (e.g., `?` inside a `match` arm inside a loop)
  are easy to miss with shallow test coverage.

**Common Mistakes to Avoid**
- Building the interpreter directly against the untyped AST instead of
  the typed AST/HIR that Phase 1.4 produces. Interpreting pre-type-
  check would let type errors surface as confusing runtime failures
  instead of compile-time diagnostics, undermining the entire
  "diagnostics are structured and precise" goal (AI_TOOLING.md).
- Over-investing in interpreter performance. M0's interpreter is
  explicitly a correctness vehicle, not a performance deliverable —
  time spent optimizing it is time not spent validating the language
  design, and it will likely be replaced or demoted to an oracle in
  Phase 2 anyway (IMPLEMENTATION_PLAN.md §4).

**Definition of Done (Outcome)**
Every fixture that should run produces the expected output; every
fixture that should panic (ERROR_HANDLING.md §4) panics with a
specific, attributable message, not a generic crash.

### 1.6 CLI + Structured Diagnostics

**Precision Points**
- `noct diagnostics --json` and `noct ast --json` must be built
  incrementally as each stage above lands, carrying real source spans
  (file/line/column/byte-offset, AI_TOOLING.md §4) from day one. Verify
  this concretely: write one throwaway external script that consumes
  the JSON output and locates an error in the original source file
  using *only* the JSON — if that script needs to also parse the
  human-readable message to work, the JSON schema is incomplete.
- Confirm diagnostics from different pipeline stages (lexer, parser,
  resolver, type checker) are distinguishable in the JSON output (a
  `stage` or equivalent field) — an AI agent or IDE consuming this
  needs to know whether a fix belongs at the syntax or semantic level.

**Common Mistakes to Avoid**
- Wrapping the human-readable diagnostic string in JSON and calling it
  "structured" (explicitly warned against in AI_TOOLING.md §4 — "not a
  JSON-wrapped string blob of the human text"). This is the single
  most common shortcut taken under time pressure and the most expensive
  to unwind later, since external tooling will have already been built
  against the wrong schema.

**Definition of Done (Outcome)**
`noct run` and `noct test` work end to end on the fixture set;
`noct ast --json` and `noct diagnostics --json` produce schema-
documented, span-accurate output verified by at least one external
consumer script as described above.

### Phase 1 Overall Definition of Done

`examples/dashboard.nv`'s non-UI subset (structs, functions, enums,
`match`, `Result`/`Option`, `?`) runs correctly end-to-end via
`noct run`. The full fixture set from Phase 0 plus every fixture
introduced in 1.1–1.6 passes as an automated regression suite. A second
person (not the implementer) can read `noct diagnostics --json` output
for a deliberately broken fixture and locate the bug without reading
the compiler source.

---

## Phase 2 — M1: NIR + Bytecode VM

### Purpose
Move off the ad hoc tree-walking interpreter onto a real intermediate
representation, both because native codegen (Phase 3) needs one and
because this is the natural point to validate that memory-mode
information can live in *one* IR without forking it into two
(DECISIONS.md Issue 6).

### Precision Points
- **Freeze the NIR instruction set deliberately, not accidentally.**
  NIR.md is explicitly underspecified pending this phase (design brief
  §19). Write the instruction set as a numbered, reviewed document
  update to NIR.md §4 *before* implementing the lowering pass — an IR
  designed implicitly, instruction-by-instruction as lowering code is
  written, tends to accumulate special cases that are hard to see as a
  whole.
- **Prove the single-IR claim with a concrete test, not an assertion.**
  Take one function, lower it once with a `native` mode tag and once
  with a `managed` mode tag (even synthetically, before managed mode
  is otherwise implemented), and confirm the *instruction sequence
  shape* is identical except for the memory-operation instructions
  (`stack_alloc`/`move`/`borrow` vs. `heap_alloc`/`arc_retain`/
  `arc_release`). If anything else differs, the mode tag is leaking
  into places NIR.md §2 says it shouldn't.
- **Differential-test against the Phase 1 interpreter before trusting
  the VM.** Every Phase 1 fixture must produce byte-identical
  observable output (stdout, return codes, panic messages) through the
  new NIR+VM path as it did through the Phase 1 interpreter. Any
  divergence is a bug in one of the two — find out which before
  proceeding, don't assume the newer one is correct by default.
- **Design async lowering against the real NIR you just built, not the
  placeholder description in CONCURRENCY.md §7.** That section was
  deliberately deferred; treat it as a starting hypothesis (state-
  machine lowering) to validate, not a finished design to transcribe.

### Common Mistakes to Avoid
- Retrofitting mode-tagging onto an IR that was designed without it in
  mind "temporarily," planning to generalize later. Build the tag in
  from the first instruction definition — this is exactly the kind of
  decision that's cheap now and expensive after the VM and Cranelift
  backend both depend on the IR's shape.
- Letting the bytecode VM's instruction encoding and NIR's logical
  instruction set become the same artifact out of convenience. NIR is
  meant to feed *both* a VM and a native backend (NIR.md §1) — if VM
  bytecode encoding details (stack layout, opcode numbering) leak into
  NIR itself, the native backend inherits VM-specific baggage it
  doesn't need.
- Deciding `interp/`'s fate implicitly by just stopping maintenance of
  it. IMPLEMENTATION_PLAN.md §4 flags this as an open decision —
  resolve it explicitly (retire it, or keep it as a differential-testing
  oracle) and record the decision, rather than letting it silently bit-
  rot into an untrustworthy leftover.

### Definition of Done (Outcome)
A documented, frozen-for-now NIR instruction set (NIR.md §4 updated
with real content, no longer marked "illustrative"). A lowering pass
from typed AST/HIR to NIR. A bytecode VM executing NIR. Full Phase 1
fixture-set parity confirmed by differential test. A written record of
the `interp/` decision and the async-lowering validation outcome.

---

## Phase 3 — M2: Native Compilation (Cranelift)

### Purpose
Produce real native binaries, and — critically — make native-mode
ownership/borrow enforcement *real* rather than loosely type-checked.
This is the phase where MEMORY_MODEL.md §2's promises stop being
design intent and start being compiler behavior someone can violate
and get caught doing so.

### Precision Points
- **Implement move/borrow checking as its own diagnostic category from
  day one** (per the Phase 1.4 note above) — don't bolt it onto the
  existing type checker's error type. Borrow errors need different
  information (the conflicting borrow's location, the point of
  invalidation) than type errors do, and AI_TOOLING.md's suggested-fix
  requirement (§4) is much harder to retrofit if the diagnostic shape
  wasn't designed for it.
- **Validate "safe defaults" and "value semantics for Copy-eligible
  types" (MEMORY_MODEL.md §2) with fixtures that would be genuinely
  annoying under a stricter model** — e.g., passing a small struct of
  primitives to multiple functions in sequence without explicit
  cloning. If this requires explicit `.clone()`-equivalent ceremony,
  the "reduced Rust learning curve" goal isn't actually being met and
  that's worth flagging as a design gap now, not discovering via user
  complaints later.
- **Cross-check the Cranelift lowering against the VM's behavior on
  every Phase 1/2 fixture**, the same differential-testing discipline
  as Phase 2, now three-way (interpreter-or-retired, VM, native).
  Numeric edge cases (integer overflow behavior, float comparison) are
  a classic place native codegen diverges from an interpreter/VM if
  not checked explicitly.
- **Do the FFI round-trip early, not last.** A single `unsafe extern
  "C" fn strlen(...)` call (FFI.md) exercises the ABI boundary, pointer
  types, and the ownership-handoff discipline (FFI.md §6) all at once.
  Treat it as a smoke test you run early in this phase, not a final
  checkbox.

### Common Mistakes to Avoid
- Treating borrow-checker false positives (rejecting valid programs) as
  lower priority than false negatives (accepting invalid ones), on the
  logic that false positives are "just" an ergonomics problem. Given
  MEMORY_MODEL.md's explicit goal of reducing Rust's learning curve,
  false positives directly undermine the core value proposition of
  native mode and deserve equal weight during this phase.
- Reaching for LLVM prematurely because Cranelift's optimizer is
  weaker. ADR-003 confirms Cranelift-first specifically to keep this
  phase's scope bounded; LLVM is Phase 4+ and optional
  (IMPLEMENTATION_PLAN.md §6).
- Building `runtime-native/`'s concurrency scheduler speculatively
  ahead of CONCURRENCY.md's still-Open channel/sync-primitive API
  surface (§6). Build only what native-mode Phase 3 fixtures actually
  need; don't pre-build a scheduler for an API that isn't designed yet.

### Definition of Done (Outcome)
`examples/dashboard.nv`'s non-UI subset compiles to a native binary via
`noct build`, with output identical to the VM/interpreter paths on the
full regression fixture set. At least one fixture demonstrating a
borrow-checker rejection with a diagnostic that includes a suggested
fix. A working FFI call to a real C stdlib function inside `unsafe`.

---

## Phase 4 — M3: Stdlib + Package Manager + Testing + Formatter + LSP

### Purpose
Turn a working compiler into a usable toolchain — the phase where
"start a project with Noctivue" starts meaning something close to what
it means in an established language (IMPLEMENTATION_PLAN.md §8).

### Precision Points
- **Finalize the package manifest format (TOOLCHAIN.md §3) as its own
  small design pass before writing `noct add`/lockfile code**, the same
  discipline as freezing NIR before implementing its lowering in Phase
  2. A manifest format designed implicitly while writing the resolver
  tends to bake in accidental limitations (e.g., assuming a single
  registry, when multiple/private registries may matter later).
- **Test the formatter's round-trip property exhaustively, not
  spot-checked.** SYNTAX.md §7 requires `noct fmt` to transform between
  compact and expanded forms *without changing semantics*. The concrete
  test: for every fixture, format it, re-parse the formatted output,
  and confirm the resulting AST is identical to the AST from the
  original source — an automated property, not a manual read-through.
- **Resolve the semicolon-formatting behavior (DECISIONS.md Issue 5)
  as actual formatter code, then update Issue 5's status from Proposed
  to Confirmed** (or record why it changed) — don't leave the spec and
  the implementation disagreeing silently.
- **Ship the default-off style lint rules as genuinely off by
  default**, verified by running `noct lint` on a fixture using
  explicit (`struct Foo:`) style and confirming no warning fires
  without opt-in configuration (DECISIONS.md Issue 7,
  STYLE_GUIDE.md §2/TOOLCHAIN.md §5).
- **Build the LSP against the same `noct diagnostics --json` /
  `noct ast --json` schemas already validated in Phase 1**, not a
  separate ad hoc protocol — this is the actual payoff of having
  invested in real schemas that early, and a divergent LSP-specific
  format would waste that investment.

### Common Mistakes to Avoid
- Writing `stdlib/` code that quietly depends on compiler behavior not
  otherwise exposed to ordinary packages (a "privileged" stdlib). This
  directly contradicts Principle 9 (ecosystem over language bloat) and
  the UI framework's stated goal of being "an ordinary package"
  (UI_SPEC.md §2) — if the stdlib needs something a third-party package
  couldn't have, that's a sign a language feature is missing, not that
  the stdlib gets a special exemption.
- Launching package publishing before signing/integrity hashing is
  actually implemented, on the logic that it can be "added before the
  *real* public launch." TOOLCHAIN.md §3 treats this as a hard
  precondition, not a fast-follow.
- Building the VS Code integration (STYLE_GUIDE.md §6.4) against a
  compiler API surface that isn't the stable public one, because it's
  more convenient during development. This creates a maintenance trap
  where the editor extension breaks on every internal refactor.

### Definition of Done (Outcome)
A second developer, using only the public toolchain (no direct access
to compiler-crate internals), can `noct create` a new project, write a
small non-UI program with at least one external dependency, run
`noct test`, `noct fmt`, and `noct build` it successfully, and get
useful diagnostics and hover information from the LSP in an editor.
Package signing is implemented and verified before any registry is
made publicly reachable.

---

## Parallel Track — UI Research Spike (UI-S0 → UI-S1 → UI-M1)

### Purpose
Answer MEMORY_MODEL.md §4 / UI_SPEC.md §3's open question — whether
ARC plus a workable cycle-mitigation strategy can support a real
widget-tree UI — without that answer blocking native-mode progress.

### Precision Points
- **UI-S0 must specifically stress-test cycles, not just confirm ARC
  works in the easy case.** A prototype that only exercises acyclic
  reference graphs proves nothing about the actual risk
  (DECISIONS.md Issue 4). Deliberately construct parent↔child and
  widget↔event-closure cycles and confirm the chosen mitigation
  (leading candidate: framework-enforced ownership discipline,
  MEMORY_MODEL.md §4 option 3) actually prevents the leak, measured,
  not assumed.
- **UI-S1's widget-tree prototype should be built against a
  *realistic* app shape**, not a toy counter — use
  `examples/dashboard.nv`'s UI portions (the parts Phase 1 explicitly
  skipped) as the target, since it already exercises state, events,
  async loading, conditional rendering, and iteration together.
- **Evaluate all four UI-M1 gate criteria (UI_SPEC.md §4) independently
  and honestly** — ARC viability, cycle-mitigation robustness,
  re-render performance, and developer ergonomics are different axes,
  and a prototype can pass three and fail one. Don't let a strong
  result on the easy criteria (e.g., ARC viability) paper over a weak
  one (e.g., ergonomics requiring excessive `weak`/`unowned` noise).

### Common Mistakes to Avoid
- Letting UI research schedule pressure pull native-mode engineers off
  Phase 2/3 work. The whole point of gating this as a parallel,
  non-blocking track (UI_SPEC.md §4, IMPLEMENTATION_PLAN.md §7) is that
  native-mode Noctivue remains fully usable even if this track fails
  its gates — protect that property operationally, not just on paper.
- Declaring UI-M1 "mostly passed" and promoting to production track
  anyway. The gate is a conjunction ("only if all of...", UI_SPEC.md
  §4) — a partial pass is a reason to iterate UI-S1 further or
  reconsider the mitigation strategy (MEMORY_MODEL.md §4 lists three
  other options), not a reason to relax the gate.

### Definition of Done (Outcome)
Either: (a) a written UI-M1 promotion record showing all four gate
criteria met, with the evidence for each (measured cycle behavior,
render-performance numbers, an ergonomics assessment from dogfooding
the dashboard example) — at which point UI_SPEC.md and MEMORY_MODEL.md
§4 get updated from Open to Confirmed for the chosen approach; or (b) a
written record of which gate criteria failed and why, feeding a
DECISIONS.md amendment considering the remaining mitigation options,
without this record blocking any native-mode phase's progress.

---

## Cross-Phase Discipline (applies throughout)

- **Never let a phase's "Definition of Done" be asserted without the
  artifact it names actually existing and being checkable by someone
  other than the person who built it.** Every "Done" section above
  names a concrete thing (a fixture set, a differential test result, a
  second developer's successful run) — treat these as literal
  acceptance tests, not prose describing a vibe.
- **Every place this guide says "record the decision"** means: update
  the relevant spec doc's status tag (Proposed → Confirmed, or Open →
  a chosen option) and, if it reverses or narrows something already
  marked Confirmed elsewhere, follow the DECISIONS.md §2 amendment
  process — name the conflict, explain why, propose the amendment, mark
  it as requiring approval. Precision here is not bureaucracy; it's the
  only thing that keeps eighteen-plus documents from silently
  disagreeing with each other six months in.
- **When a phase's precision points reveal a flaw in an earlier
  Confirmed decision** (as Phase 2's single-IR validation or Phase 3's
  false-positive/false-negative balance might), the correct response is
  always the amendment process, never a quiet fix that leaves the spec
  document describing something the implementation no longer does.
