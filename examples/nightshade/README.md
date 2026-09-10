# Nightshade

Nightshade is a small text-mode project tracker used as a Noctivue
example. It intentionally stays within the currently implemented core
language: structs, enums, functions, lists, matching, and assertions.

Run the entrypoint from the repository root:

```text
cargo run -p noct-cli -- run examples/nightshade/lib/main.nv
```

Run its smoke test from the project directory:

```text
cd examples/nightshade
cargo run -p noct-cli -- test
```

Local `import` declarations are now loaded automatically from the
importing file's directory or the package's `lib/` directory:

```text
cargo run -p noct-cli -- run lib/main.nv
```

Local workspace package imports are supported. A package entrypoint may
live at `lib/main.nv`, and imports resolve from the current package's
`lib/`, checked-out sibling packages, `packages/`, and `.noct/packages/`.
