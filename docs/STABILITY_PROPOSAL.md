**Status:** Draft proposal for Phase 6, not normative.

# STABILITY_PROPOSAL.md — Stability Policy (Draft)

This draft sketches the SemVer and channel commitments Phase 6/M5 owes as
a governance decision (`docs/IMPLEMENTATION_PLAN.md` §8.6), frequently a
hard enterprise-adoption precondition. Nothing here binds until it lands
as its own ADR (`docs/DECISIONS.md` §2); conflicts fail toward Confirmed
text.

## 1. The language: what SemVer covers

- Covered surface: accepted syntax (`docs/SYNTAX.md`), diagnostic codes
  (`E####`/`W####`), the IR shape consumers pin (`docs/NIR.md`), and the
  `noct` CLI surface (`docs/TOOLCHAIN.md` §2, including `--json` flags in
  `docs/AI_TOOLING.md`).
- Major: any previously accepted program is newly rejected; any diagnostic
  code is removed or renumbered; IR or CLI-argument shape changes break a
  pinned consumer. New keywords and reserved-word promotions are major.
- Minor: new syntax that only accepts previously rejected programs; new
  diagnostic codes; new CLI flags that default off.
- Patch: bug fixes where the implementation converges on the documented
  behavior with no surface change.

## 2. Standard library: addition-only within a major

- Within a major version, stdlib changes are additions: new modules and
  functions are minor; behavior fixes that match documented contracts are
  patches. Signature changes, removals, and renames are major-only.
- Renames follow the amendment rule, never a silent rewrite: stdlib names
  are public forever (`stdlib/SPEC.md` ground rules; `libs/README.md`
  "why not stdlib"), so a rename proposal lands as a DECISIONS.md
  amendment with migration notes, and the old name survives through the
  deprecation window in §5.

## 3. First-party libraries (`libs/`): independent versions

- Each package versions independently by SemVer in its own `nestpkg.nvpm`
  (`libs/<name>/`, `libs/README.md` layout); iterating here never requires
  a language amendment (Principle 9: libraries, not core growth).
- Trust-tier labels (`native` / `c-shim` / `cxx-shim` / `wasm-component` /
  `foreign-runtime`, `docs/TOOLCHAIN.md` §3) are stable per package: a tier
  change is a major version bump for that package, with the `noct audit`
  row visibly changing. Graduation to the blessed-package set never moves
  the API into stdlib silently.

## 4. Channels: LTS and stable (sketch, not decided)

- Proposal only: a rolling `stable` channel plus time-based `lts` windows.
  The window length is explicitly OPEN — Option A: 6-month LTS with
  12-month security backports; Option B: annual LTS with 24-month
  backports. Phase 6 picks one; this draft does not.
- Within a channel, updates are patches and compatible minors only; majors
  always open a new channel, never rewrite the old one.

## 5. Deprecation: warn first, remove late

- Every removal is preceded by a warning phase delivered through lint rules
  and diagnostic codes (default-on warnings for use of deprecated items),
  never by release notes alone.
- Minimum window: one full LTS window (once §4 is decided) or one major
  version, whichever the landing ADR states — never shorter. Removal
  happens only in a major, with migration notes in the amendment.

## 6. Governance: where decisions live

- Amendments, never silent rewrites (`docs/DECISIONS.md` §1 taxonomy):
  any change to §§1–5 lands as a new ADR or ADR amendment, with rationale
  and migration impact stated. Quiet edits to this policy are rejected in
  review.
- Record locations: language/stdlib commitments in `docs/DECISIONS.md`;
  package-level notes in the owning `libs/<name>/docs/overview.md` and the
  manifest version itself; pipeline-visible proof in `noct audit` and
  `noct publish --dry-run` outputs.

## 7. Open items (Phase 6 decides)

- Whether new diagnostic codes are minor (proposed) or always patch.
- The LTS window length (Option A vs B above) and backport scope.
- Whether `stable` auto-follows or pins per project (lockfile interplay).
