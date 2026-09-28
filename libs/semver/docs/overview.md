# semver — overview

Status: v0.2.0, native tier, pure `.nv` (no builtins beyond
`print`/`println`/`assert`, so it runs on `run` today and stays portable
to `run-vm`/`build`).

## What it does

- `version.nv` — parse `MAJOR.MINOR.PATCH[-pre][+build]`, render it back
  exactly as it was spelled, and order two versions two ways:
  `core_cmp` (the numeric core, which is the relation P-003 fixes) and
  `version_cmp` (SemVer precedence, pre-release aware and build-blind)
  plus `eq`/`lt`/`lte`/`gt`/`gte` over the precedence order and
  `build_eq` over the metadata precedence ignores.
- `requirement.nv` — parse a requirement in one of the two spellings the
  manifest accepts (`1.2.3` caret, `=1.2.3` exact) and test a version
  against it on the numeric core.
- `digits.nv` — INTERNAL digit and character-class scanning (see below).

## Why it exists

The catalog called for a `semver` package whose "caret semantics [are]
identical to P-003 by construction". That is the point: the manifest
grammar already fixes what a version and a requirement may *spell* and
what they *mean* (P-003 §§2–3), and `noct add` / `noct build` / `noct
audit` all depend on those rules. A library that reimplemented them
differently would let a dependency mean one thing in its manifest and
another in the code that consumes it. So this package implements the
same rules, and `tests/semver_test.nv` ends with a "toolchain parity"
block asserting the requirements a manifest may actually declare
(`0.1.0`, `1.0.0`) resolve identically here.

## Where this package is ahead of the manifest — and where it refuses to be

v0.2.0 adds pre-release and build-metadata parsing. That puts the
parser one step ahead of the P-003 v1 manifest grammar
(`nestpkg/src/lib.rs`, `SemVer::parse`), which still splits on `.` and
demands digits, so it rejects `1.2.3-rc.1` and `1.2.3+build`. Being ahead
is a deliberate choice, and it is deliberately one-directional:

- **Everything the manifest can spell, this package parses and orders
  the same way.** The parity block at the end of the test file is that
  claim, case by case.
- **The parser accepts spellings the manifest cannot produce**, which
  is safe: nothing in the toolchain can hand a consumer a pre-release
  version yet, and a library user cannot widen a manifest by reaching
  for this parser.
- **The requirement half refuses to follow.** `parse_requirement`
  rejects a pre-release or build section with an error naming the
  manifest grammar, and `req_satisfied` compares the numeric core
  (`core_cmp`), field for field what the crate's `req_satisfied` does.
  This is the line the package does not cross: a requirement helper
  that matched a semantics the manifest cannot express would let a
  dependency mean something its own `nestpkg.nvpm` cannot say — the
  same argument that makes compound ranges an error, and the same
  argument that kept the parser behind the grammar in v0.1.0.
- **When P-003 grows the grammar, one commit switches
  `requirement.nv` from `core_*` to `version_*`** and the parity block
  grows its suffix rows. The precedence rules are already implemented
  and tested here, so that commit is a small one. Not before.

## Decisions worth knowing

- **Two orders, and they are not the same relation.** `core_cmp` is
  the numeric core: `1.2.3-rc.1` equals `1.2.3`. It is what P-003 and
  the crate implement, and what `requirement.nv` matches on.
  `version_cmp` is SemVer *precedence*: pre-release aware, build
  metadata ignored, so `1.0.0-alpha < 1.0.0` and
  `1.0.0+build == 1.0.0`. They disagree only on inputs the manifest
  cannot spell, which is exactly what the test file's "orders: ok"
  block asserts in both directions.
- **A total order over precedence, not over spellings.** Because
  precedence ignores build metadata, `1.0.0+a` and `1.0.0+b` are
  "equal" — the spec says so and every SemVer implementation agrees.
  A total order on spellings does not exist without inventing a rule
  the spec does not have, so `version_eq` means *equal precedence* and
  `build_eq` is how a caller asks "same build metadata?" instead.
  Both are tested, including the pair that differs.
- **Strict spelling, still.** Exactly three numeric core fields, no
  leading zeros (`01.0.0` is rejected — `01` and `1` must not be two
  spellings of one version). Suffix sections follow the spec to the
  letter: identifiers are `[0-9A-Za-z-]`, must be non-empty
  (`1.2.3-a..b` is an error), a wholly numeric pre-release identifier
  may not carry a leading zero (`1.0.0-01`), and a numeric *build*
  identifier may (`1.0.0+001` — the spec allows it, and it cannot
  affect anything).
- **ASCII only, refused loudly.** The grammar is ASCII, and the
  language measures `String.length` in bytes while indexing by
  character, so a non-ASCII character would make a range walk read past
  the end of the string. `parse_version_from` therefore checks the
  whole input up front and returns a named `Err` — never a guess, never
  an index panic. `1.2.3-é` and `1.é.3` are both errors.
- **Errors, never guesses.** Every malformed input is `Err` with a
  message naming the problem. Nothing is coerced, truncated, or
  defaulted.
- **Compound ranges are refused.** `>=1.0, <2.0` is a loud error, not a
  misparse. v1 of the manifest grammar treats it that way, and a
  requirement helper that quietly accepted more than the manifest would
  let a dependency express something its own manifest cannot.
- **Caret ceiling by base.** `^1.2.3` admits `<2.0.0`; `^0.2.3` admits
  `<0.3.0`; `^0.0.3` is exactly `0.0.3`. `caret_upper_bound` exposes the
  ceiling so a caller can explain a rejection.
- **No identifier list is ever materialised.** The language cannot
  build a list, so a pre-release with an unbounded number of dotted
  identifiers is compared by walking both sides with cursors
  (`next_pre_field` / `pre_cmp`) and stopping when one side runs out.
  There is no cap on how many fields a pre-release may carry, and
  `tests/semver_test.nv` checks a 20-field one.
- **No digit-conversion builtin.** The language has no `char -> Int`
  conversion and `to_int` is `run`-only, so `digits.nv` matches the ten
  digit literals, and the alphanumeric class is a one-character string
  range test (string comparison is lexicographic, so for one ASCII
  character that *is* a code-point range test). That keeps the package
  pure and total, and it is why fields are scanned by index range
  instead of being sliced into strings: there is no slice expression
  and no `char -> String` conversion either. `copy_range` is the one
  place a substring is built, by concatenation.

## Module layout and the linking rule

`digits.nv` is INTERNAL: it carries `export` only because the linker
drops private items of dependency files, so a private helper called by
an exported function would vanish when this package is consumed
(rule 1 in `libs/README.md`). Application code should import `version`
or `requirement`, never `digits`. `version` and `requirement` also
item-import each other rather than module-importing, because a type
name does not land in scope through a bare module import. Every
`/// INTERNAL —` helper in `version.nv` (`index_of_char`, `copy_range`,
`is_ascii_run`, `is_ident_run`, `is_numeric_run`, `valid_dotted_idents`,
`field_value`, `parse_core`, `parse_pre`, `parse_build`,
`or_fallback`, `next_pre_field`, `num_text_cmp`, `pre_field_cmp`,
`pre_cmp`) is exported for that same reason.

## Traps this package hit, so the next change does not

- **Byte length vs character index.** `s.length` is a UTF-8 byte count
  and `s[i]` is a character, so `while i < s.length { s[i] }` panics on
  non-ASCII input. Every walk here spends a byte budget instead
  (`budget = budget - "{c}".length`), and `requirement.nv`'s
  `has_range_syntax` was fixed to do the same in v0.2.0 — it walks raw
  caller input before the ASCII guard and used to panic on `é=1.2.3`.
- **The tree-walking interpreter has a small call-stack budget.** A
  flat `main` with ~150 statements, or a chain of calls ~10 deep, will
  overflow it (`thread 'main' has overflowed its stack`) — with no
  diagnostic, and it depends on how deeply the chain nests, so it can
  be triggered by one extra `let`. Hence one test function per section,
  and `let`-bound values instead of a call nested inside a call inside
  an `assert`. `noct test` is the only gate that catches this.
- **`noct fmt` rewrites, it does not suggest.** It joins sibling
  statements onto one line and will re-wrap a long `Err("...")`, so
  long messages are written short enough to survive it. Run
  `noct fmt <files>` and then `noct fmt --check`; never hand-tune.
- **`noct diagnostics` reports the first file only** when given
  several paths, so the gate is really run per file.

## Gates (from the package directory)

    noct diagnostics lib/*.nv tests/*.nv     # clean (one file per run)
    noct lint lib/*.nv tests/*.nv            # clean
    noct fmt --check lib/*.nv tests/*.nv    # clean
    noct test                                # 1 passed (16 sections)
    noct publish --dry-run                   # ready (unsigned, no deps)

## What "market ready" still requires

- Version ranges beyond the manifest's two v1 spellings — **skipped
  on purpose**: the manifest grammar has to move first, and this
  package moves in the same step, never ahead of it.
- When P-003 v2 lands: switch `requirement.nv` to the precedence order
  and accept suffixes in requirements, in one commit, with the parity
  block extended. The precedence half is done and tested.
- Optionally: a public `pre_release(v) -> String` accessor, if an
  application ever needs the section as a value rather than through
  `Version.pre`.
- Benchmarks are unnecessary at this size; the cost is a few passes
  over a short string.
