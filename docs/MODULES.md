# MODULES.md — Modules & Imports

**Status:** Local file + directory modules implemented; qualified
access (`alias.item`, `alias::Type`) implemented via an export-gated
linker rewrite; package-manifest format Decided (ADR-017,
`nestpkg.nvpm`) with file-backed registry resolution (see
TOOLCHAIN.md). Re-exports (`export import ...`) **Decided** (implemented
in parser, linker, formatter, and LSP). Directory-module nesting rules
**Decided** (implicit-by-directory). Both diagnostic gaps in §4.1
**Fixed** (unaliased qualifier stripping; qualified private access
hint). Linter rules L-004 (whole-module import style) and L-005
(ad-hoc error types) added as default-off style rules.

## 1. Compilation Units

Each `.nv` file is a module. A directory of `.nv` files forms a module
tree mirroring the directory structure, in the tradition of Rust/Go —
exact nesting/`mod`-declaration rules (implicit-by-directory vs.
explicit re-export files) are **Decided**: implicit-by-directory is the
chosen rule; explicit `mod` declarations are not supported. The directory
name serves as the module name; a file `a/b/c.nv` is module `a::b::c`.
This is implemented and tested.

## 2. Import

```nv
import http
import json::Parser
import mypackage::utils as utils
```

- `import path` brings a module into scope under its own name.
- `import path::Item` brings a specific item into scope.
- `as` renames the imported binding.

## 3. Export

```nv
export
User:
    id: Int

export fn calculate(x: Int) -> Int:
    x * 2
```

Only `export`-marked top-level declarations are visible to importers of
a module; everything else is module-private by default (safe default,
Principle 7).

## 4. Path Resolution

Paths are resolved relative to the importing file, then against the current
package's `lib/` directory. Workspace package names are also searched as
checked-out sibling packages, `packages/<name>/lib`, and cached packages
under `.noct/packages/<name>-<version>/lib`. Every file is parsed
independently, linked through its import graph, and cycles are diagnosed at
the import edge. Registry and Git fetching remain package-manager work.

Directory modules: when `a/b.nv` does not exist, `import a::b` falls
back to `a/b/<last>.nv` then `a/b/mod.nv` (exact files always win),
so a library can live in a directory named for its module
(`core/constants/constants.nv` serves `import core::constants`).

Package-name imports search, in order: a checked-out sibling
`<pkg>/lib/`, `packages/<pkg>/lib/`, a committed `vendor/<pkg>-*/lib/`
tree, then the fetched `.noct/packages/<pkg>-*/lib/` cache. `vendor/`
precedes the cache deliberately: a vendored tree is reviewed and
committed, an extracted one is whatever the last fetch left behind —
and `vendor/` is the committed offline escape hatch, so a vendored
project resolves with no network and no cache
(`docs/TOOLCHAIN.md` §3).

## 4.1 Linking & qualified access

Linking flattens: the root keeps its private items, dependencies
contribute only `export` items. On top of the flattened program the
linker rewrites qualified references that name a linked module to their
flat form. What works today, and what does not:

```text
WORKS   alias.item / module.item            field and method access in expressions
WORKS   alias::item(...)  module::item(...) calls
WORKS   alias::Type                         type position: params, returns,
                                             let/var annotations
WORKS   alias::Type { field: v }            struct-literal name
WORKS   import m::Type  ->  Type            item import, then the bare name
WORKS   pkg::Type                           full module-path prefix is registered
                                             too, so `import semver::version` also
                                             qualifies as `semver::Type`

NEVER   alias.Type  (a DOT, not `::`)       E0100 — a dot is member access, not a
                                             type path; use `alias::Type`
```

The qualifier is `::` in every position. A dot (`v.Version`) is member
access and is never a type path — it does not parse in a type
position or before a struct literal, by design, not by gap.

**Declared-before-use is NOT required.** Call order inside a file is
free: a function may call one defined later in the same file, in a root
file or in a dependency, exported or private. Resolution is not a
single forward pass over source order.

## 4.2 Re-exports: `export import`, `export *`

```text
export import foo      re-export another module's exported items
export *               forward this file's exported items
export * except a, b   ...with names listed for exclusion
```

`export *` is accepted and validated but **contributes nothing**, and
that is deliberate rather than a stub. Linking here is flat: a root
file already contributes *all* of its items, and a dependency already
contributes exactly its `export`ed items, so every name `export *`
asks to forward is already in the linked program. Re-injecting it would
push a *second* definition of each re-exported name.

The limitation that hides is worth stating: **a barrel cannot use this
form to withhold a name.** A root keeps its private items and a
dependency's non-exports are never contributed at all, so
`export * except x` has nothing left to remove — the exclusion list is
checked, not honoured. Giving it real meaning needs a namespaced
barrel model, which is an architecture decision, not a linker tweak.

So that the form cannot rot into a comment that lies, a name in an
`except` list that is not an export of the file writing it is a loud
`E0110` — not a silently dead line. Use `export import` when you want
to control exactly what a barrel exposes.

**Qualified private access** (`p.private_one()` where `private_one`
lacks `export`) is refused, because the rewriter only rewrites exported
members. It emits `W0101` naming the private item and the one-word fix
(`add export to use it as p.private_one`), matching the flat-use hint
rather than blaming the alias with a bare `E0201`.

The expression rewrite is export-gated (`document.path` is untouched
when `path` is a local field rather than an export), and rewriting is
root-scoped (dependency files are never rewritten), so locals that
share a module's name keep working. Item imports
(`import m::item [as x]`) are used flat or under their alias and need
no qualifier.

Diagnostics: `E0100` cycle, `E0101` unresolvable module (span covers
the import text), `E0102` private item via direct import, `E0103`
missing item, `E0109` alias on a trait/impl/module/derive item
(only plain items can be renamed — use the module form instead),
`W0101` warning when a flat use matches a *private* item of a
module-only import (the one-word fix is `export`). `noct test`'s
single-file fixture runner, `noct lint`, and LSP hover/definition
stay single-file; `lint` L-002 and LSP diagnostics are
graph-aware (flattened uses count, E0201s for linked exports are
suppressed), so the editor agrees with `noct run`.

## 5. Re-exports — Decided

Re-exports use the explicit form `export import path [as alias]`. This
is distinct from a plain `import` + `export` pair and is now
**implemented** (ADR-027). The re-export expands to all exported items
of the target module, making them available as if they were defined
in the current module. This enables library authors to create
convenience preludes (e.g., `export import resilience::prelude`) so
consumers see one import instead of many, without weakening
per-declaration `export` as the visibility default. Whole-file export
is NOT the default — explicit per-declaration export remains the
visibility rule.
