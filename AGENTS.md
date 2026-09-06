# AGENTS.md

## Build & Test

```bash
cargo build --workspace
cargo test --workspace
```

### Key test suites
- Compiler: `cargo test -p compiler`
- Interpreter: `cargo test -p interp`
- Native backend: `cargo test -p runtime-native`
- CLI (all): `cargo test -p noct-cli`
- CLI add tests: `cargo test -p noct-cli --test add`
- Differential tests: `cargo test -p noct-cli --test differential`
