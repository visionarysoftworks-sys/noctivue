# TANK.md — stdlib work guide (side-agent briefing)

Cold-start brief for an agent working the Noctivue standard library.
You have repo access. The repo is source of truth; verify every claim
below against the tree before acting (parts of this tree move fast —
see "Coordination" at the end).

## 0. History in one paragraph

`examples/tank/` was the first seed (one math lib + demos, short names
like `min`). It has been RETIRED as a library home: `stdlib/` at the
repo root is the canonical, SPEC'd, harnessed home, and tank's unique
edge asserts were folded into `tests/fixtures/stdlib/smoke_core.nv`.
Do not recreate a second library home. The multi-file CLI support
(`noct run/build a.nv b.nv`, concatenation in order, per-file
diagnostics) built for tank REMAINS — `stdlib/SPEC.md` §1 depends on it
as the pre-import distribution model.

## 1. Mission

Extend the stdlib foundation per `stdlib/SPEC.md` (the contract — read
it FIRST, fully, including §§7–10): more runnable modules, missing
type-twins (`[+Float]`/`[+String]` markers), more smoke coverage. Do
NOT redesign the contract; amend it like DECISIONS.md (propose, never
silently rewrite) when reality disagrees with it.

## 2. The map (read in this order)

1. `stdlib/SPEC.md` — API contract: what exists, what is RUNNABLE vs
   SPEC-ONLY vs reserved, error conventions E1–E4, style S1–S6, gates.
2. `stdlib/dev.md` — ground rules (stdlib belongs to the language;
   renames need amendments).
3. `tests/stdlib_test.rs` — the three gates (foundation zero-errors,
   reserved zero-items, smoke exit-0). Header comment explains the
   golden/differential situation.
4. `tests/fixtures/stdlib/`, `tests/golden/stdlib/` — smoke mains +
   expected transcripts (goldens compared via CLI, outside the harness).
5. `docs/NIR.md` §6 — language gaps that bind stdlib scope (fixed-size
   lists, opaque strings, generics strategy open).
6. `noct-cli/src/cmd_run.rs` (`read_sources`, `owner_file`) — the
   concatenation mechanics you rely on. READ-ONLY for you (see §7).

## 3. Hard constraints (verified by execution — do not re-probe without
cause; SPEC §2 has the evidence table)

- Free functions only, never `x.method()` (struct FIELD reads ok).
- No `const` globals → `fn` accessors (`pi()`, `e_const()`).
- No generic functions → monomorphic twins (`min_int`, `list_len_string`).
- No trait/impl dispatch → free fns with struct first arg.
- No list growth (fixed-size: literal, index, `.length`, iteration).
- No Map/Set values (reserved files stay header-only).
- No `type` aliases. Enum match arms use paren form (`Some(v):`, `None:`).
- `to_string` takes `String` only → interpolate (`"{x}"`).
- List `.first`/`.last` rejected by typeck → index reads with guards.
- Strings: `==`, `+`, interpolation, `[i]`→Char ONLY. No split/trim/chars.
- Unary minus is avoided (`0 - 5`, never `-5`); `!` for Bool-not.
- Preconditions panic per E1 (`panic(msg)`); absence is Option (E2);
  explainable failure is `Result<T, String>` (E3); propagate with `?`/`??` (E4).

## 4. Naming (normative — SPEC §3/S-note)

Suffixed monomorphic names NOW (`min_int`, `pow_int`, `list_len_string`,
`option_expect`, `assert_int_eq`); bare names (`min`, `pow`) are RESERVED
for the generic future (`min` becomes generic, `min_int` a deprecated
alias). Short names like `abs`/`gcd`/`is_even` already shipped — new
names follow the suffix convention. snake_case fns, PascalCase types.

## 5. What remains (pick in order)

1. Missing twins marked `[+Float]`/`[+String]`/`[+Bool]` in SPEC §5.
   Twin conventions: the suffix `[+T]` on a free fn signature means the
   function ships with a native variant for type `T` (e.g. `[+Float]` adds
   a `Float` overload, `[+String]` adds a `String` overload). Shipped files
   use the base name without suffix; the suffix is purely a SIGNFIER in the
   source. Per SPEC §5.3 `int_eq` carries `[+Float, String, Bool, Char]`,
   `int_lt` carries `[+Float]` only, `option_is_some` carries `[+String,
   +Bool]`, and `list_len` carries `[+String, +Bool]`. New twins must be
   added via SPEC amendment, not silently.

2. Native-status tags (S3: `Native: yes`/`loop`/`needs-builtin`/`spec-only`)
   on any fn missing one. Updated conventions for Phase 3 Step 2:
   - `Native: yes` — straight-line, no control flow, safe for `noct build`.
   - `Native: loop` — contains `while`/`for`/`break`/`continue`; runs
     natively via `run` but `noct build` emits `unresolved callee` until
     the VM/lowering lands (P-001 discipline).
   - `needs-builtin: X` — requires a builtin that Phase 3 Step 2+ may
     provide (e.g. `needs-builtin: list-push`, `needs-builtin: string-push`).
   - `spec-only: <reason + phase>` — reserved for a future phase beyond
     M3. Every public fn must carry exactly one S3 tag; missing tags trigger
     a lint warning (L-002 family).

3. Aggregate slot model (Phase 3 Step 2). The native backend now supports
   a slot-based representation for aggregate values (`[T]` lists and `String`).
   Key points:
   - Lists `[T]` are lowered to a sequence of slot instructions; `.length`,
     index `[i]`, and iteration are RUNNABLE natively.
   - Strings are treated as `[Char]` slots; `string_len`, `string_char_at`,
     and `==`/`+`/`interpolation` are all native.
   - `needs-builtin: list-push` and `needs-builtin: string-push` are the
     only Phase 3 Step 2+ additions that require runtime support; until those
     builtins land, `noct build` over them fails loudly (same discipline as
     `FuncId::UNRESOLVED`).
   - Aggregate slots follow the `NvStr` one-pointer header model:
     `list_push`, `string_push` lower to `ListPush {dst, list, elem, elem_ty}`
     and `StringPush {dst, string, ch}` respectively.

4. More smoke asserts for thin spots (compare against SPEC §5 line by line).
5. Reserved promotions ONLY with a SPEC amendment + gate updates in
   `stdlib_test.rs` (FOUNDATION/RESERVED lists) — never silently.
6. Differential coverage: SPEC §10 records VM-differential as blocked;
   NATIVE differential is newly possible for straight-line + control-flow
   subsets (the backend learned branches recently) — high-value, discuss
   before building (shared target-dir rules apply — read
   `noct-cli/tests/native_build.rs` header first).

## 6. Verification (run all three, every change)

```powershell
cargo test -p noctivue --test stdlib
cargo test -p noct-cli --test differential
cargo test --workspace   # full gate; non-negotiable before handoff
```

Plus manual, for any behavior change:
`noct run <SPEC-§5.1-files-in-order> <smoke>` vs `noct run-vm ...` —
byte-identical stdout expected where the VM supports the constructs
(see §10 for known VM gaps: `to_int`/`to_float`, fn-refs-as-values).

## 7. Scope and coordination (binding)

- YOUR files: `stdlib/**`, `tests/fixtures/stdlib/**`,
  `tests/golden/stdlib/**`, `tests/stdlib_test.rs`, `stdlib/*.md`
  (via amendment proposals, not rewrites).
- READ-ONLY for you: `noct-cli/**`, `compiler/**`, `interp/**`,
  `docs/**` (except proposing SPEC/DECISIONS amendments in chat).
  Another agent owns the toolchain + backend. If a stdlib need requires
  toolchain work (new builtin, diagnostic change), write the proposal
  (exact CLI behavior + example + which phase owns it) and STOP — do not
  reach across the boundary yourself.
- Never break the gate. Never commit unless asked. Small reviewable
  batches; report per batch (files touched, gates run, counts).
- Concurrent-editing incident (recorded): an in-flight edit elsewhere in
  the tree once made this suite fail transiently with zero code cause;
  if a green suite goes red with no related change, re-run twice before
  investigating, and capture the failing test NAME + message immediately.
