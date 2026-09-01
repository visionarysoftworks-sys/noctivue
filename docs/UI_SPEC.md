# UI_SPEC.md — UI Architecture

**Status:** Widget-tree model Confirmed (ADR-005); implementation
Experimental, gated by a research spike (§4). UI state model Open.

## 1. Model

Flutter-style declarative widget tree, expressed as ordinary Noctivue
code — not a separate markup or templating language:

```nv
Dashboard:
    column:
        text("Analytics")
        button("Refresh"): refresh()
        graph(data: revenue, type: line)
```

Expanded form of the same tree:

```nv
Dashboard:
    column:
        header:
            title("Analytics")
            button("Refresh"):
                refresh()

        graph:
            data: revenue
            type: line
            height: 320

            x_axis:
                field: date

            y_axis:
                field: amount

            tooltip:
                enabled: true

            legend:
                enabled: true
                position: bottom
```

## 2. "Ordinary Package," Not Compiler Magic

The UI framework (`column`, `button`, `text`, `graph`, etc.) is built
using public compiler/language APIs — the colon-block-as-trailing-
closure desugaring described in DECISIONS.md Issue 2 — rather than
special-cased in the compiler frontend. This keeps Noctivue's promise
that there is one language, not a language-plus-UI-DSL (Principle 1).

Component declarations (`Dashboard:`) are, syntactically, identical to
struct declarations (SYNTAX.md §11); which one a given declaration *is*
resolves during name resolution based on how the declaration is used
(built via calls that assemble a tree vs. built via field literals).
This is flagged as the highest-risk open syntax question in the spec
(DECISIONS.md Issue 2) and must be validated by a working parser/
resolver prototype before it is promoted to Confirmed.

## 3. UI State — Open Problem

Reference cycles are a structural risk for any ARC-based widget tree
(parent↔child, widget↔event-closure). This is **explicitly not solved**
in v0.1. Candidate directions (full detail: MEMORY_MODEL.md §4):

1. Weak-by-default closure capture — **rejected** as a general default;
   reintroduces null-like failure modes.
2. Value-oriented view-state model: application state lives outside the
   widget graph in ordinary (non-widget) managed or native values;
   widgets read and re-render from it rather than owning it. Reduces
   cycle surface area by construction.
3. Framework-enforced ownership discipline: parent strongly owns child,
   child holds `unowned` parent back-reference, enforced by the
   widget-tree runtime rather than a language-wide default. **Leading
   candidate**, pending prototype validation.
4. Fully explicit `weak`/`unowned` everywhere — highest safety,
   highest verbosity; fallback if 2/3 don't pan out.

State declarations in components use the `state` statement shown in the
grammar (SYNTAX.md §10, `state_decl`):

```nv
Dashboard:
    state loading = false
    state revenue = []
```

Whether `state` is sugar over the general native/managed variable
system or a distinct UI-runtime-tracked construct (for re-render
triggering) is part of the same Open question.

## 4. UI Research Track

Managed/UI development proceeds **in parallel** with native-language
development, but as a research spike, not a committed production
feature, until concrete gate criteria are met.

| Stage | Goal |
|---|---|
| **UI-S0** | ARC runtime prototype: validate that ARC (with `weak`/`unowned`) is workable as managed mode's core mechanism at all. |
| **UI-S1** | Widget-tree prototype: build the framework in §3-candidate #3 (or whichever direction UI-S0 supports) and stress-test cycle behavior with a realistic app. |
| **UI-M1** | Promotion to production track — **only if** all of: (a) ARC behavior is viable under real workloads, (b) the chosen cycle-mitigation strategy holds up without excessive `weak`/`unowned` noise, (c) widget-tree re-render performance is acceptable for interactive UIs, and (d) developer ergonomics are judged compelling in dogfooding, not just benchmarks. |

If UI-S0/UI-S1 fail their gates, this document's Confirmed status on
ADR-005 (the *model*) is not automatically invalidated, but the
*implementation path* would require a new ADR amendment (DECISIONS.md
§2, amendment process).

## 5. Rendering Targets — Deferred

Desktop/mobile/web rendering backend architecture is **Deferred**
entirely until UI-M1 gate criteria are met; no rendering-target
commitments are made in v0.1.
