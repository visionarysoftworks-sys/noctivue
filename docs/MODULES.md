# MODULES.md — Modules & Imports

**Status:** Local file + directory modules implemented; qualified
access (`alias.item`, `alias::Type`) implemented via an export-gated
linker rewrite; package-manifest format Decided (ADR-017,
`nestpkg.nvpm`) with file-backed registry resolution (see
TOOLCHAIN.md).

## 1. Compilation Units

Each `.nv` file is a module. A directory of `.nv` files forms a module
tree mirroring the directory structure, in the tradition of Rust/Go —
exact nesting/`mod`-declaration rules (implicit-by-directory vs.
explicit re-export files) are **Proposed**, not finalized.

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

NEEDS   pkg::Type                           parses; qualifier NOT stripped (see below)
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

The `pkg::Type` asymmetry is a *diagnostics* problem, not a rewrite
bug: the rewriter strips only *registered alias* prefixes, so
`import semver::version` registers `version` and never `semver`. A
qualifier that was never imported is therefore silently left in place
and the reader gets a type mismatch (`expected pkg::Type, found Type`)
that points at the type instead of at the missing import. Import the
type as an item (`import pkg::m::Type`) and write it flat, or import
the module with an alias and use `alias::Type`.

**Known diagnostic gap.** Qualified access to a *private* item of a
dependency (`p.private_one()` where `private_one` lacks `export`) is
refused because the rewriter only rewrites exported members — but the
resulting error blames the ALIAS (`E0201 unknown identifier p`) rather
than naming the private item and the `export` fix. A flat use of the
same name gets the `W0101` hint; the qualified form does not yet.

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

## 5. Re-exports — Proposed

Whether Noctivue needs an explicit re-export form (`export import ...`)
distinct from a plain `import` + `export` pair is Proposed, not decided;
tracked alongside the package-manifest format work.
