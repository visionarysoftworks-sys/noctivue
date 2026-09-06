# MODULES.md — Modules & Imports

**Status:** Local file modules implemented; package-manifest format Open (see TOOLCHAIN.md).

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

Paths are resolved relative to the importing file, then against the nearest
`lib/` directory in the current package. Every file is parsed independently,
linked through its import graph, and cycles are diagnosed at the import edge.
External dependencies remain package-manifest work (TOOLCHAIN.md §3).

## 5. Re-exports — Proposed

Whether Noctivue needs an explicit re-export form (`export import ...`)
distinct from a plain `import` + `export` pair is Proposed, not decided;
tracked alongside the package-manifest format work.
