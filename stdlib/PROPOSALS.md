//! Stdlib-track proposals to the language track.
//!
//! Each entry is a **request**, not a decision: it motivates a compiler /
//! runtime change from stdlib needs, specifies the exact surface, and
//! lists acceptance criteria. Accepted entries move to docs/DECISIONS.md
//! as ADRs and amend the frozen docs they cite; rejected entries stay
//! here with the reason, so the ask is never silently re-raised.
//!
//! Status conventions: Proposed → Accepted | Rejected | Superseded.

# Proposals (stdlib track → language track)

## P-001: `list_push` / `string_push` builtins (growth primitives)

**Status:** Proposed (2026-09-05).

**Problem.** Lists are fixed-size: literals, `l[i]`, `.length`,
iteration — no construction beyond literals (NIR.md §6, last two
bullets). This single gap blocks the entire M3 growth surface:
`list_push`, `list_map_*`, `list_filter_*`, `string_repeat`,
`string_split`, `strings/builder`, `collections/stack`,
`collections/queue` (stdlib/SPEC.md C5, §5.7/§5.9/§9). Everything else
about lists already works in the interpreter AND the VM (`ListLen`,
`ListIndex` are lowered and executed; `for` desugars to them).

**Key claim: growth does NOT need the ownership story first.**
NIR.md §6 says growth "needs the ownership story (borrowck) first".
That is true for *in-place* mutation (`push(&mut xs)`), which needs
aliasing rules. It is not true for *functional* growth returning a new
list — the IR already has that precedent: `FieldSet` clones the struct
and returns the new value (`vm.rs`: `new_fields = fields.clone()`),
and the interpreter's `Value::List` is a cloneable `Vec`. This
proposal asks only for the functional form, with the in-place form
explicitly deferred to post-borrowck (never precluded).

**Surface (normative).** Two free-function builtins, same status as
`print` (plain identifiers, no syntax change, no method-call lowering
needed — SPEC.md C1 stays intact):

```nv
list_push(xs: [T], v: T) -> [T]      // new list; xs observably unchanged
string_push(s: String, c: Char) -> String
```

Copy-on-write semantics, each stage's natural expression of it:

| Stage | Behavior |
|---|---|
| interp | clone the `Vec`, push, return the new `Value::List`; `String` + `Char` likewise. Input binding keeps the old value (observable in user code — this is the semantic, not an optimization detail). |
| VM | same over `VmValue` (clone + push). O(n) time/space, documented, not hidden. |
| native | STAGED: no lowering until a native list representation lands (Phase 3 Step 2+). Until then `noct build` over either builtin MUST fail loudly (same discipline as `FuncId::UNRESOLVED`), never silently miscompile. Runtime symbol sketch for that day: `nv_list_push`, `nv_string_push` in `runtime-native`, following the `NvStr` one-pointer header model. |

**Per-file changes (for the implementer — estimated small):**

1. `compiler/src/typeck/mod.rs` — special-case in `infer_call` next
   to `Some`/`Ok`/`Err` (generics don't unify at call sites, SPEC.md
   C3, so a `fn_sigs` entry cannot express `[T]`): arg 0 must be
   `List(e)` (or `Unknown`/`Error` passthrough), arg 1 must be
   `compatible_with(e)`, return `List(e)`. `string_push`: `(String,
   Char) -> String`. Mismatches → `E0200`, the existing mismatch code.
2. `interp/src/lib.rs` — `is_builtin` + `eval_builtin` arms (mirror
   `to_string`'s fallthrough-tolerant style).
3. `compiler/src/nir/instr.rs` — two instructions in the Aggregate
   group, mirroring `ToString` (which carries `from_ty` because
   lowering is type-directed — `ListPush` carries `elem_ty: Ty` for
   the same reason):
   `ListPush { dst, list, elem, elem_ty }` →
   `%dst = list_push %list, %elem`;
   `StringPush { dst, string, ch }` →
   `%dst = string_push %string, %ch`.
   Plus `Display` arms (the NIR.md §4 table is generated from these).
4. `compiler/src/nir/lowering.rs` — `Ident("list_push" |
   "string_push")` → the instrs, in the same `if let` chain that
   special-cases `print`/`println` today.
5. `compiler/src/nir/vm.rs` — execution arms (clone + push).
6. `compiler/src/backends/cranelift/` — loud rejection until list
   lowering exists (see native row above).
7. `docs/NIR.md` §4 table + §6 bullets — amendment entries (NOT a
   silent rewrite): add the two instrs to the Aggregate row; revise
   the lists bullet (functional growth no longer waits on borrowck;
   in-place growth still does); the strings bullet is ALSO stale —
   drive-by correction below.
8. Editor completion lists, if builtins are enumerated outside the
   compiler (grep for `println` under `compiler/src/analysis.rs` and
   `editors/`) — nice-to-have, not acceptance-gating.

**Drive-by correction (for the NIR.md owner, who is editing that file
now):** §6 says "Strings are opaque past `len`/`==`/concat/
interpolation: no indexing…" — stale. `s[i] → Char`, `.length`, and
`Char == Char` all execute in interp AND VM today (`ListIndex` serves
`String`; stdlib `strings/string.nv` search loops are built on it and
pass). Only string *growth* is missing.

**Alternatives considered and rejected:**

- (a) Method syntax `xs.push(v)` — rejected: member-call lowering
  does not exist (`E1003` at runtime, SPEC.md C1). Revisit with the
  M3 method-call story, not here.
- (b) In-place `push(&mut)` now — rejected: that IS the borrowck-gated
  form NIR.md §6 means. This proposal explicitly does not ask for it.
- (c) `xs + [v]` concat operator — rejected: changes `Add` semantics,
  hides the O(n) cost inside innocent-looking arithmetic, and needs
  identical per-stage work anyway.

**Stdlib unlock table (ships in stdlib/ once builtins land):**

```nv
fn list_push(xs: [Int], v: Int) -> [Int]                        // thin wrapper, documents the builtin
fn list_map_int(f: (Int) -> Int, xs: [Int]) -> [Int]
fn list_filter_int(pred: (Int) -> Bool, xs: [Int]) -> [Int]
fn list_reverse(xs: [Int]) -> [Int]
fn string_repeat(s: String, n: Int) -> String                   // concat loop over string_push… (see note)
fn string_split(s: String, delim: Char) -> [String]             // needs BOTH builtins
// + strings/builder.nv, collections/stack.nv, collections/queue.nv graduate from reserved
```

Note: `string_repeat` needs only `string_push` (or even `+`-concat);
`string_split` needs `list_push` for the piece list. `string_join`
needs neither (concat loop over an existing list — already writable).

**Test plan (mirrors the Phase 2 discipline):**

- interp probes: push preserves the input (`xs` unchanged after
  `list_push`), push-to-empty, push-then-index/len, type errors
  (`list_push(1, 2)`, `list_push(xs, "s")` → `E0200`).
- `compiler/src/nir/vm_tests.rs` entries for both instrs.
- differential `run` vs `run-vm` cases (blocked today by the
  unrelated multi-file-VM gap, SPEC.md §7.5 — cases still get
  written, gate opens with the VM fix).
- stdlib smokes extended + goldens regenerated; SPEC.md C5 retired.

**Acceptance criteria:**

- [ ] `list_push([1,2], 3)` → `[1,2,3]` with the input binding
      observably unchanged, on `run` AND `run-vm`.
- [ ] Misuse is loud at every stage: `E0200` from typeck,
      `FunctionNotFound`-class error from the VM (never wrong-function
      dispatch), loud build failure from `noct build` until native
      lowering lands.
- [ ] No existing test regresses; NIR.md §4/§6 amendments recorded.
- [ ] Zero-cost conceptual overhead for users who don't call it: no
      syntax, keyword, or preamble change.

**Related, explicitly OUT of scope:** `to_string`'s narrow
`(String)->String` signature, `panic`/`assert`/`to_int`/`to_float` VM
lowering, the multi-file `run-vm` CFG gap, in-place mutation,
generics unification — each its own proposal.

## P-002: default-on lint `variant-named binding arm` (L-001)

**Status:** Implemented AND executed (2026-09-05): `noct-cli/src/
cmd_lint.rs` builds clean; `noct lint` over the fixture corpus fires
exactly the 5 predicted warnings (exit 0, everything else silent);
`cargo test` green (stdlib 3/3, integration 6/6, compiler lib 128).
Corpus audit: all 5 fires genuine — see below.

**Problem.** A bare-identifier match arm is a *binding*, never a
variant test (stdlib/SPEC.md C8 — probed: `Red:` matched everything,
`Red():` matched correctly). When such an arm names a known enum
variant and sits in *non-final* position, every arm below it is dead
and the match silently tests nothing. The parser cannot reject it
(SYNTAX.md §10 leaves the question to resolution); the type checker
treats it as a legal binding. Only a lint can catch it, which is
exactly TOOLCHAIN.md §5's "things the grammar can't" brief.

**Rule L-001 (default-on).** Warn when a match arm's pattern is a bare
`Ident(name)` where `name` equals a visible enum variant name
(`Item::Enum` variants in the file, plus `Some`/`Ok`/`Err`/`None`)
AND the arm is not the last arm of the match. Message names the trap
and the fix:

```text
warning[L-001]: `Red` here is a binding that matches everything, not a test for the `Red` variant
  = note: arms below this one are unreachable
  = help: match the variant with `Red():` (or `Red(x):` for payloads)
```

**Deliberately NOT warned (documented, not overlooked):**

- Trailing bare-`None` arms (`Some(v): …` / `None: …`) — the
  codebase-wide idiom today (fixtures, stdlib `option.nv`,
  `result.nv`). They are positionally correct by accident of order;
  warning on them would drown the real signal. A stricter form
  (`None():`) is future work, not this rule.
- `_` and `Variant(…)` arms — already the correct forms (SPEC.md S6).

**Detection (no new resolution needed).** One pass over the parsed
`Program`: collect variant names from top-level `Item::Enum`s (+ the
four prelude constructors); walk every `match`'s arms in order
(statements nest — `if`/`while`/`for`/`loop` bodies and nested match
arms must all be visited); fire on non-final `Pattern::Ident(name)`
with `name` in the set. No type information required, so the rule
runs pre-typeck and never emits false positives on ordinary bindings
(`x`, `user`, `found` are not variant names).

**Acceptance criteria:**

- [x] Warns on the probe6 shape (`Red:` first, `Blue` unreachable);
      silent on `Red():` / trailing-`None` / `_` shapes — verified by
      corpus audit: the only fires are `enums_match/basic.nv`
      `direction_label` (`North:`, `South:` — that function returns
      `"up"` for every input today, genuine bug) and
      `m1_complex_test.nv` `describe_dir` (`North:`/`South:`/`East:` —
      `describe_dir(West)` prints `"Going up"`, genuine bug, invisible
      because the fixture prints instead of asserting). Both filed as
      fixture bugs per this proposal, not rule bugs. All stdlib,
      smoke, `result_option`, `dashboard`, and payload-variant arms
      (`Circle(radius):`, `Ok(v):`, `Just(v):`) are silent.
- [ ] `noct lint` exit code stays 0 on warnings (implemented);
      machine-readable JSON form follows later with `--json`.
- [ ] Ships default-ON (structural, not stylistic — DECISIONS.md
      Issue 7 keeps style rules default-off; this one is a bug trap).

## P-003: manifest schema, lockfile, integrity & signing (fills ADR-017)

**Status:** Proposed (2026-09-05).

**Scope discipline.** ADR-017 (Proposed) already decides: filenames
(`nestpkg.nvpm`, `nestpkg.lock`), syntax family (colon blocks +
significant indentation, SYNTAX.md §§6–7 — not YAML/TOML), a dedicated
hand-written line-oriented parser homed as a `manifest` module in
`noct-cli` (the full compiler frontend is NOT involved), v1 field set
(name, version, description, dependencies with ADR-015 tiers,
dev-dependencies), and editor association by exact filename. This
proposal does NOT relitigate any of that. It specifies what ADR-017
leaves open: the exact grammar, version-requirement syntax, tier
encoding and the foreign-runtime opt-in mechanics, the lockfile
schema, the hash format, and the signing design (a hard pre-launch
precondition per TOOLCHAIN.md §3 and the `cmd_publish` stub).
Resolution-algorithm choice stays Deferred (TOOLCHAIN.md §3); registry
HTTP API is out of scope.

### 1. Manifest grammar (v1)

Line-oriented, indentation-significant, spaces only (a tab anywhere is
a loud error — the LANGUAGE_SPEC.md §2 "never a guess" ethos applies
to the manifest too). Indent width is free but must be consistent
within a block (Python rule). Values are scalars only — bare words,
integers, booleans, double-quoted strings with `\"`/`\\`/`\n` escapes
— or nested blocks. The single composite form is the `- ` sequence
(no maps-in-maps, no inline tables, no expressions, no interpolation —
ADR-017). Duplicate keys are errors. Unknown top-level sections are
errors, EXCEPT `x-`-prefixed sections, which v1 tools ignore (the
forward-compat escape hatch; a v2 section never silently degrades —
unknown non-`x-` sections fail closed).

```ebnf
manifest     = { section } ;
section      = "package:" block | "dependencies:" block
             | "dev_dependencies:" block | "x-" , name , block ;
block        = newline , INDENT , { entry } , DEDENT ;
entry        = key , ":" , ( scalar | block | sequence ) , newline ;
sequence     = newline , INDENT , { "- " , scalar , newline } , DEDENT ;
scalar       = bare_word | integer | boolean | quoted_string ;
```

### 2. Manifest schema (v1)

```text
package:
    name: myapp                 # required. ^[a-z][a-z0-9_]*$, max 64 chars.
                                # Registry reserves `std*`, `noct*`, `core`.
    version: 0.1.0              # required. Strict SemVer MAJOR.MINOR.PATCH,
                                # no pre-release/build metadata in v1.
    description: Short sentence. # optional, one line.
    authors:                    # optional sequence of strings.
        - A U Thor
    license: MIT                # optional SPDX identifier string.
    edition: 0.1                # optional language-edition string.

dependencies:
    http: ^1.2                  # scalar = caret requirement (see §3).
    legacy_c: =0.9.1            # exact pin.
    fast_png:                   # expanded form for tiers/sources.
        version: ^2.0
        tier: c-shim            # default when omitted: native.
    py_model:
        version: 1.0
        tier: foreign-runtime
        opt_in: true            # REQUIRED with foreign-runtime (§4).

dev_dependencies:               # same shapes; tiers required here too
    testkit: ^0.4               # (ADR-015: every dependency entry declares
                                # a tier — the scalar form declares native).
```

`edition` is opaque to v1 tools (recorded, echoed in errors) — its
semantics arrive with the stability policy (Phase 6/M5).

### 3. Version requirements (v1)

- `1.2.3` — caret: `>=1.2.3, <2.0.0` (documented LOUDLY at first use;
  the bare-means-caret rule is the format's sharpest edge).
- `=1.2.3` — exact pin.
- Compound ranges (`>=1.2, <2.0`), wildcards, pre-releases: v2
  (the line parser stays tiny until the registry era needs them;
  encountering them is a loud error naming the v2 roadmap, not a
  misparse).

### 4. Tier encoding and the foreign-runtime opt-in (normative)

- `tier:` one of `native` (default) `c-shim` `cxx-shim`
  `wasm-component` `foreign-runtime` (ADR-015). `cxx-shim` records an
  Experimental flag that `noct audit` surfaces (ADR-015).
- `opt_in: true` is REQUIRED alongside `tier: foreign-runtime`;
  `opt_in` with any other tier is an error (not a warning — a stray
  opt-in that silently stops meaning something is how audit tiers
  rot).
- Transitivity rule (the load-bearing one): a foreign-runtime package
  reachable ONLY transitively is a resolution ERROR naming the chain
  (`myapp → warez → py_model`), unless the top-level manifest declares
  that package directly with `opt_in: true`. Opt-in is per top-level
  project, never inherited — exactly TOOLCHAIN.md §3's "not eligible
  to be pulled in transitively".
- `source:` (expanded form only): default `registry`; `path:
  ../foo` (version still checked, hash/signature skipped — it is your
  own tree); `git: <url>` + `rev: <commit>` (lock pins the commit,
  hash covers the checked-out tree). Unknown source kinds are errors.

### 5. Lockfile (`nestpkg.lock`, generated, checked in)

Same syntax family (one dedicated parser for both files), canonical
form: sections and keys sorted byte-wise, exact versions only, LF
endings — byte-reproducible from the same resolution. Written by
`noct add`/`noct build --update-lock` (exact command split is
implementation detail); hand edits are legal but re-verified (a
mismatched hash fails the same as a corrupt download).

```text
lock_version: 1
packages:
    fast_png:
        version: 2.0.3
        source: registry
        content: sha256:9f2c…
        tier: c-shim
        signed_by: key:7ad1…
    myapp_local:
        version: 0.1.0
        source:
            path: ../myapp_local
        tier: native
```

- `content:` is `sha256:<lowercase hex>` over the canonical package
  bytes (the tarball as published; §6). Unknown hash prefixes are
  hard errors (algorithm agility without silent downgrade); v1 knows
  only `sha256`.
- `signed_by:` is the publisher key-id that signed this version (§7);
  absent ONLY for `path` sources (nothing to verify — your tree).
- The lock records the experimental flag for `cxx-shim` entries so
  `noct audit` needs no re-derivation (ADR-015).

### 6. Integrity (required now, enforced at first fetch)

Threats addressed: mirror/tamper in transit, cache poisoning,
substitution of a different tarball under a known version. NOT
addressed here: name squatting and malicious-but-legitimately-signed
code (registry policy + audit, M5).

- Hash computed at publish, recorded in the registry index AND the
  lock at `add` time; verified on every fetch and every build that
  touches the cache. Mismatch = hard error printing expected vs
  actual (never warn-and-continue).
- `noct build` first checks manifest-vs-lock consistency (a changed
  requirement with a stale lock is an error prompting `add`/
  `--update-lock`, not a silent re-resolve — reproducible builds
  mean the lock is the build input).

### 7. Signing (designed now, enforced before public launch)

Threats addressed: registry/index compromise serving attacker bytes
under a trusted name+version; long-term verifiability (a user can
re-verify years later from the lock alone).

- Primitive choices (no invented crypto): Ed25519 signatures, SHA-256
  content hashes. Signed payload is the domain-separated ASCII
  string `noctivue-publish-v1:<name>@<version>:<content-hash>`
  (domain separation kills cross-protocol signature reuse).
- `noct publish` signs with the publisher's local key and uploads
  `{tarball, signature, key-id}`; the registry index serves all three
  plus the key directory. `--dry-run` runs every check except upload
  (manifest valid, version unused, signature verifies locally).
- Trust model: TOFU with a paper trail. First `add` prints the new
  key-id and records it in the lock (`signed_by`); every later fetch
  fails closed on key-id OR signature mismatch. Rotation is explicit
  (`noct add <pkg> --rotate-key`, re-prints, re-records) — never
  automatic, never transitive.
- Verification is mandatory on fetch for `registry`/`git` sources
  (fail closed, no `--insecure` flag exists on purpose); `path`
  sources are exempt (your tree, your responsibility).
- Keys live with the user, not the project: per-OS config home
  (`%APPDATA%/noct/keys`, `~/.config/noct/keys`,
  `~/Library/Application Support/noct/keys`) holding one file per
  key-id. Private keys never leave that directory (publish signs
  locally; the registry never sees them).
- `noct audit` (M5) rows are ready now: `name version tier
  signed_by audit-status` — the vulnerability-DB column arrives
  later, the trust columns exist from day one.

### 8. Lifecycle mapping (who reads/writes what)

| Command | Manifest | Lock | Network |
|---|---|---|---|
| `create` | writes template | — | no |
| `add pkg` | adds/updates req | re-resolves, hashes, verifies sig, records `signed_by` | yes |
| `build`/`run`/`test` | read | must be consistent (else error) | only on cache miss |
| `publish [--dry-run]` | validates | — | yes (no on dry-run) |
| `audit` | — | reads trust columns | vuln-DB only (M5) |

### 9. Test plan (no network — fixture index under `noct-cli/tests/`)

- Golden manifest↔AST pairs (incl. every §2 shape) + lockfile
  canonicalization goldens (key order, LF, exact bytes).
- Negative corpus (each a loud, cited error): tabs, inconsistent
  indent, duplicate keys, unknown section, non-`x-` future section,
  bad name, bad SemVer, compound range (v2 error), missing `opt_in`,
  stray `opt_in`, transitive foreign-runtime, hash mismatch, sig
  mismatch, unknown hash prefix, rotated key without `--rotate-key`.
- Round-trip: parse→serialize→parse is identity on all goldens.
- Parser fuzz: random byte mutations of a valid manifest never panic
  (errors only) — the manifest parser is pre-build attack surface.

### 10. Acceptance criteria

- [ ] `manifest` module parses every §2 shape and rejects every §9
      negative with a line-numbered error.
- [ ] `add`→lock→`build --offline` round-trips against the fixture
      index with zero network.
- [ ] Signature verification fails closed in all four mismatch modes
      (bad sig, wrong key, rotated key, unknown algorithm).
- [ ] `audit` output shows tier + `signed_by` per row from lock data
      alone.
- [ ] TOOLCHAIN.md §3 + DECISIONS.md gain the ADR-017 amendment
      (this design, unchanged or revised) — the amendment process in
      DECISIONS.md §2 governs, not this file.

**Explicitly OUT of scope:** resolution algorithm, registry HTTP API,
version-range v2, yank/revoke flows, the vuln database, `noct create`
templates (M5 `--template ci` covers those).

## P-004: formatter architecture + canonical style (v1 ships here)

**Status:** v1 implemented + executed (2026-09-05):
`noct-cli/src/cmd_fmt.rs` + `noct-cli/tests/fmt.rs` (8/8 green);
v2 architecture decided here, implementation open.

**Why two versions.** The normative bar (TOOLCHAIN.md §4: lossless
density-level transformation; DECISIONS.md Issue 5: the formatter owns
semicolons) needs an AST printer. But the parser DROPS comments
(comment tokens flow through `Spanned<Token>` and die in
`parser/grammar.rs` — `noct doc` re-extracts them from source lines
for exactly this reason). An AST printer without comment attachment is
a comment deleter: lossy by construction, and therefore NOT shippable
as "the official formatter". So: v1 does everything that is provably
trivia-safe today; v2 is specified here and gated on comment
attachment. No silent scope cut — the boundary is explicit.

**Canonical decisions (binding on both versions):**

- D1 — Indent is 4 spaces per level. Tabs in leading whitespace are a
  loud error (same ethos as the manifest, P-003 §1 — never a guess).
- D2 — At most ONE consecutive blank line; exactly one `\n` at EOF;
  LF endings (a `\r` is trailing whitespace and dies with it).
- D3 — Semicolons: Issue 5 solution (a) is ADOPTED (was Proposed):
  the formatter removes statement-separating `;` (one statement per
  line, expanded form) and introduces `;`-joined lines ONLY under the
  v2 density heuristic (short single-purpose sequences under the line
  cap). Semicolons are never load-bearing; `noct fmt` output contains
  none unless v2's heuristic put them there.
- D4 — Density default is EXPANDED (braced one-liners and compact
  `name: stmt` forms stay legal input and keep their AST, but the
  formatter's output is expanded form). Rationale: expanded form is
  the only form where every construct has exactly one spelling, so it
  is the only sane canonical target; compaction is a heuristic, not
  an identity.
- D5 — Line cap 100 columns for v2 heuristics (formatter breaks past
  it, never an error). Borrowed value, recorded so it can be tuned
  with data later rather than debated forever now.
- D6 — Concise vs. explicit declarations are NEVER rewritten
  (SYNTAX.md §4 equivalence is a reader choice, Issue 7's
  fragmentation warning applies — a default-off lint may opine, the
  formatter stays silent).

**v1 scope (shipped): trivia canonicalizer.** Byte-level line ops
only: strip trailing whitespace, collapse 3+ blank lines to one,
exactly-one trailing newline, LF. String-aware: `"""…"""` spans are
opaque (multi-line strings exist — lexer tests prove it — so a naive
strip would corrupt values); `//` comments, `"…"`/`'…'` literals and
escapes are lexed well enough to skip correctly. Leading whitespace
(including tabs) is untouched — the lexer judges indentation, not v1.
`--check` mode (exit 1 + file list, no writes). In-command safety:
re-pipeline the output and refuse to write if error diagnostics
INCREASED; refuse to write if not idempotent
(`fmt(fmt(x)) != fmt(x)` is a formatter bug — loud, not persisted).

**v2 scope (open): AST printer.** Prints from the resolved `Program`
with comments re-attached from token spans (leading/trailing trivia
by span proximity — the design, not the code, is the deliverable
here). Safety gates, both blocking: (1) idempotency on the corpus;
(2) AST-equivalence — formatted output parses to an identical AST
(requires `PartialEq` on `ast` types or JSON comparison via the
`cmd_ast` serializer; whichever lands first). Density heuristic per
D4/D5, semicolons per D3, expressions gain one-space operator
padding. Format-on-save rides the LSP later (TOOLCHAIN.md §6), never
as a second implementation.

**Test plan (v1, executed):** `noct-cli/tests/fmt.rs` — idempotency
cases, `--check` exit codes, triple-quote preservation incl.
trailing spaces INSIDE the span, blank-collapse, missing-EOF-newline,
CRLF handling, no-op byte-identity on the stdlib corpus.

**Acceptance criteria:**

- [x] v1 normalizes only trivia (byte-diff on fixtures shows
      whitespace-line changes and nothing else).
- [x] `--check` + idempotency + no-new-errors guards in-command.
- [x] CLI tests green (8/8); `fmt --check` clean on `stdlib/`
      (42 empty placeholders normalized to single-newline via fmt
      itself; reserved gate re-verified). 3 tracked fixtures deviate
      (`function_no_return_type`: no EOF newline; `dashboard_nonui`,
      `simple_vm_test`: CRLF) — left untouched, other track's call.
- [ ] v2: comment attachment design reviewed; AST-equivalence gate
      implemented; D3–D5 heuristics with corpus goldens.
