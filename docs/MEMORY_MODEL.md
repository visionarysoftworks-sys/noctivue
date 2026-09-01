# MEMORY_MODEL.md — Native & Managed Memory Model

**Status:** Bifurcated model Confirmed (ADR-001). Several specifics
below are explicitly Proposed/Open — read status tags carefully; this is
the most architecturally sensitive document in the set.

## 1. Why Two Modes

Systems/embedded/CLI/server code wants deterministic, GC-free resource
management. UI/application code benefits from simpler reference-counted
semantics where a tracing-GC pause would be unacceptable but manual
ownership reasoning would slow development. Rather than pick one and
compromise both audiences, Noctivue defines two modes sharing one
syntax, type system, and compiler frontend (ADR-001).

## 2. Native Mode (default)

Ownership + borrowing, inspired by Rust but deliberately simplified:

- Every value has a single owner at a time.
- Ownership can be **moved** (transferred) or **borrowed** (temporary,
  non-owning reference).
- The compiler enforces that borrows cannot outlive their owner and that
  a value is not used after it is moved.

Noctivue reduces Rust's learning curve through:

- **More inference** — lifetimes are inferred in the common case; only
  genuinely ambiguous cases require annotation (exact annotation syntax
  is **Open** — see §5).
- **Simpler lifetime reasoning** — function-local and single-owner
  patterns (the large majority of real code) need zero lifetime
  annotations at all.
- **Safe defaults** — value semantics (copy-on-assign) for small,
  `Copy`-eligible types (primitives, small structs of primitives),
  reducing how often ownership rules are even in play.
- **Compiler assistance** — borrow-check diagnostics are designed to
  suggest concrete fixes, and are exposed machine-readably
  (`noct diagnostics --json`) so IDEs/agents can offer one-click fixes.

**Not a goal:** cloning Rust's lifetime *syntax*. The exact annotation
form for the rare case that needs one is **Open** (§5).

## 3. Managed Mode

Uses ARC (Automatic Reference Counting). Intended for UI, application
scripting, and plugin environments — code where predictable low-level
performance matters less than development speed and where ownership
graphs are often genuinely shared (e.g., a widget referenced by both a
parent and an event callback).

Opt-in is explicit; native is the default. Illustrative (not final)
syntax:

```nv
managed User:
    id: Int
    name: String
```

**Status: Proposed, not finalized.** Alternatives under consideration
include a module-level mode declaration (`mode managed` at file top) vs.
this per-declaration marker; the per-declaration form is favored
provisionally because it keeps mode visible at the point of use, which
matters for the AI-tooling and IDE-hover goals (AI_TOOLING.md), but this
is not locked.

### 3.1 ARC's Known Weakness: Cycles

Reference cycles (A strongly owns B, B strongly owns A) leak memory
under pure ARC. Noctivue defines the escape hatches:

```nv
weak     // does not keep the referent alive; access returns Option<T>
unowned  // does not keep the referent alive; access assumes validity
         // (a safety-checked runtime trap on dangling use, not UB)
```

`unowned` trades a small runtime check for ergonomics where the
programmer can guarantee lifetime alignment (e.g., a child's back-
reference to a parent that is guaranteed to outlive it structurally).
`weak` is the safe default for genuinely uncertain lifetimes.

**Noctivue does not claim ARC eliminates memory-management complexity.**
It trades tracing-GC pauses for cycle-awareness as the primary residual
burden on the developer.

## 4. UI State & Cycle Mitigation — Open Problem

This is explicitly **not** solved yet. Candidate directions
(elaborated in UI_SPEC.md §3, cross-referenced from DECISIONS.md Issue 4):

1. Weak-by-default closure capture in UI contexts — rejected as a
   general default (reintroduces null-like failure at a distance); see
   DECISIONS.md Issue 4.
2. A dedicated, value-oriented view-state model (state lives outside the
   widget graph; widgets read it, they don't own it) — reduces cycle
   surface area structurally rather than through reference discipline.
3. Widget-tree-runtime-enforced ownership: parent strongly owns child,
   child holds `unowned` parent pointer, enforced by the framework
   rather than by a general language rule. **Currently the leading
   candidate**, tracked for validation in the UI-S0/UI-S1 research spike
   (ROADMAP.md).
4. Fully explicit `weak`/`unowned` everywhere, no framework-provided
   default.

## 5. Native-Mode Lifetime Annotation Syntax — Open

Whether Noctivue needs explicit lifetime syntax at all (vs. inference
covering ~100% of practical cases with a hard compiler error directing
the user to restructure code, rather than to annotate) is genuinely
undecided. This is deliberately left open rather than pre-committing to
a Rust-like `'a` syntax, per design brief §44 ("not Rust with colons").

## 6. Two Runtimes

See RUNTIME.md for the native and managed runtime facility split; both
share language syntax, type system, compiler frontend, common IR
concepts (NIR.md §2 — mode metadata, not a forked IR), and tooling.

## 7. Cross-Mode Boundary

Per DECISIONS.md Issue 3, native and managed value graphs are **Open**
but *provisionally* treated as disjoint in v0.1: values cross the
native/managed boundary only through explicit conversion functions,
conceptually similar to an FFI boundary (FFI.md). Implicit sharing
across modes is not supported pending further design work.
