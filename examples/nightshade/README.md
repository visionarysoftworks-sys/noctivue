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

External package dependencies and module namespaces are not implemented
yet; this first step supports local `.nv` modules.
