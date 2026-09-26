# semver — overview

Status: v0.1.0, native tier, pure `.nv` (no builtins beyond
`print`/`println`/`assert`, so it runs on `run` today and stays portable
to `run-vm`/`build`).

## What it does

- `version.nv` — parse a strict `MAJOR.MINOR.PATCH` version, render it
  back, and order two versions (`version_cmp` / `eq` / `lt` / `lte` /
  `gt` / `gte`).
- `requirement.nv` — parse a requirement in one of the two spellings the
  manifest accepts (`1.2.3` caret, `=1.2.3` exact) and test a version
  against it.
- `digits.nv` — INTERNAL digit scanning (see below).

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

## Decisions worth knowing

- **Strict spelling, deliberately.** Exactly three numeric fields, no
  leading zeros (`01.0.0` is rejected — `01` and `1` must not be two
  spellings of one version), no pre-release or build metadata in
  v0.1.0. A version this package rejects is one the manifest parser
  rejects, so the two can never disagree about what is spellable.
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
- **No digit-conversion builtin.** The language has no `char -> Int`
  conversion and `to_int` is `run`-only, so `digits.nv` matches the ten
  digit literals. That keeps the package pure and total, and it is why
  fields are scanned by index range instead of being sliced into
  strings: there is no slice expression and no `char -> String`
  conversion either.

## Module layout and the linking rule

`digits.nv` is INTERNAL: it carries `export` only because the linker
drops private items of dependency files, so a private helper called by
an exported function would vanish when this package is consumed
(rule 1 in `libs/README.md`). Application code should import `version`
or `requirement`, never `digits`. `version` and `requirement` also
item-import each other rather than module-importing, because a type
name does not land in scope through a bare module import.

## Gates (from the package directory)

    noct diagnostics lib/*.nv tests/*.nv     # clean
    noct lint lib/*.nv tests/*.nv            # clean
    noct fmt --check lib/*.nv tests/*.nv    # clean
    noct test                                # 1 passed
    noct publish --dry-run                   # ready (unsigned, no deps)

## What "market ready" still requires

- Pre-release and build-metadata parsing (`1.2.3-rc.1+build`), with
  precedence rules — the semantics are standard but the spec text is
  not yet written down for this package.
- A total order that accounts for pre-release, which is *not* the same
  relation as the numeric one implemented here.
- Version ranges beyond the manifest's two v1 spellings, if and when
  the manifest grammar grows them (this package must move in the same
  step, never ahead of it).
- Benchmarks are unnecessary at this size; the cost is one pass over a
  short string.
