# MODULES.md — Modules & Imports

**Status:** Core model Confirmed; package-manifest format Open (see TOOLCHAIN.md).

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

Paths are resolved: (1) relative to the current package's module tree,
then (2) against declared external dependencies (TOOLCHAIN.md §3). There
is no implicit global namespace merge across packages.

## 5. Re-exports — Proposed

Whether Noctivue needs an explicit re-export form (`export import ...`)
distinct from a plain `import` + `export` pair is Proposed, not decided;
tracked alongside the package-manifest format work.
