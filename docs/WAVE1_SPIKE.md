**Status:** Wave 1 spike record — measurements and recommendation for the ADR-023 lock amendment, not normative.

# WAVE1 SPIKE — TLS and Postgres-driver vehicle evaluation

Scope: Wave 1 SPIKE only. No product implementation, no change to any
existing file. This record judges the Phase 5 plan wording ("TLS via an
FFI binding to a vetted C library", "an FFI-bound driver") against
pure-Rust challengers per ADR-023, which keeps both contests explicitly
open and locks nothing until amended with measured rationale.

## 1. Method

Read first, in full: `docs/DECISIONS.md` ADR-023 (criteria, shortlist
rule, lock rule), `docs/IMPLEMENTATION_PLAN.md` Phase 5 items 2 and 4,
`docs/FFI.md` sections 1-8 plus section 9 (C++ scope, out of scope here
— see section 8 below), `docs/PHASE5_PRODUCTION.md` section 6
(non-requirements), `docs/TOOLCHAIN.md` section 3 (integrity/signing,
trust tiers), `docs/DEPLOYMENT.md` section 2 (static-footprint goal),
and `Cargo.lock` plus the workspace `Cargo.toml` files.

Static probes only, local tooling only, scratch confined to
`C:\Users\HP\AppData\Local\Temp\opencode` (never inside the repo).
No web access: any crate not in the local cargo cache cannot be
resolved, built, or measured here. Cells below are tagged
`[measured]` (observed in this spike) or `[knowledge]` (general
technical fact, NOT measured here — each maps to an item in section 6
that Wave 1 implementation must confirm before the ADR-023 lock
amendment).

Note on Phase 5 wording tension (not resolved here, flagged for the
amendment author): IMPLEMENTATION_PLAN.md Phase 5 item 2 places TLS in
M4, but PHASE5_PRODUCTION.md section 6 defers TLS/`https://` to Phase 6
("Client refuses loudly today"), and the item-4 amendment rides the
Postgres-class driver on the M5 executor into Phase 6. This spike
scores both vehicles against Phase 6 needs (TLS-terminating server plus
TLS client; driver with transactions as one designed unit), since that
is where the plan as amended actually consumes them.

## 2. Measured baseline (in-tree facts)

- `[measured]` `Cargo.lock` contains NO TLS crate and NO Postgres
  crate: no `rustls`, `openssl`, `native-tls`, `mbedtls`, `postgres`,
  `tokio-postgres`, `tokio`, or `libpq-sys`. Every candidate in this
  spike is a new dependency; none is vetted in-tree yet.
- `[measured]` What IS vetted in-tree: `ed25519-dalek 3.0.0` +
  `ed25519 3.0.0` + `curve25519-dalek 5.0.0` (pure Rust, `noct-cli`
  signing path), `sha2 0.11.0` (pure Rust), `rusqlite 0.40.2` over
  `libsqlite3-sys 0.38.2` with the `bundled` feature (`compiler` and
  `interp` manifests). Precedent cuts both ways: pure-Rust crypto is
  already trusted for signing, and vendored-C-via-`cc` is already
  trusted for the database.
- `[measured]` `cc 1.4.5` + `find-msvc-tools 0.1.12` + `shlex` are in
  the lock via the `libsqlite3-sys` bundled build, and
  `target/debug/build/libsqlite3-sys-*/out/sqlite3.lib` exists in the
  repo build tree: the vendored-C static-archive path on this
  Windows/MSVC toolchain is proven by the current build, not assumed.
- `[measured]` MSVC 14.44 (`VS 18 Community`, `HostX64/x64 link.exe`)
  is installed, but neither `link.exe` nor `cl.exe` is on `PATH` in a
  plain shell. Cargo builds find the toolchain anyway (Probe A below),
  so any vehicle that needs `cl.exe` on `PATH` as a manual step starts
  with a packaging handicap.
- `[measured]` Local registry cache holds 152 crate sources; none of
  the TLS/driver candidates is among them (section 5, Probe B).
- `[measured]` `cargo build --offline -p noct-cli` succeeds end to
  end (1m30s): the existing closed dependency set builds hermetically.
  (`cargo metadata --offline` on the workspace root errors on
  `fiat-crypto v0.3.0` download; harmless for this spike — no lock
  member lists it as a dependency — but recorded so nobody
  mis-triages it as a toolchain gap.)
- `[measured]` Workspace license is MIT (workspace root `Cargo.toml`):
  the license bar for any vehicle is MIT-compatibility, with any
  dual-license election recorded explicitly.

## 3. TLS scorecard

Criteria are ADR-023's, in order: static-link story on this
Windows/MSVC toolchain; license; client+server API coverage for
Phase 6 needs; auditability of the binding; packaging weight
(vendored C build vs cargo dep).

### 3.1 OpenSSL (vetted-C)

- Static-link story: `[knowledge]` poorest of the shortlist on this
  toolchain. A vendored build on Windows needs Perl plus NASM/assembler
  plus the `openssl-src`-style build script driving nmake; a system
  OpenSSL is not OS-bundled on Windows, so "system lib" means adding a
  vcpkg/Chocolatey/installer step. Either fork breaks the hermetic
  offline build measured in section 2. `[open-measure 1]`.
- License: `[knowledge]` Apache-2.0 since 3.0 — MIT-compatible, no
  election needed. Confirm exact version text at lock time.
- Client+server coverage: `[knowledge]` complete by definition
  (reference TLS implementation; full server and client sides, TLS 1.2
  and 1.3, session resumption, ALPN).
- Auditability of binding: poor fit for FFI.md section 6. The
  `openssl-sys` surface is hundreds of raw symbols with
  version-dependent availability macros; the `unsafe` boundary review
  would be the largest of any candidate, and callback-heavy APIs
  (verify callbacks, session callbacks) collide with FFI.md section 7
  (capturing closures are not C-ABI compatible; trampoline deferred).
- Packaging weight: `[knowledge]` heaviest — vendored build compiles
  hundreds of C/assembly files and ships a multi-MB static archive
  plus generated headers. Worst fit for DEPLOYMENT.md section 2
  (static, minimal-footprint binary).

### 3.2 LibreSSL (vetted-C)

- Static-link story: `[knowledge]` weak on Windows/MSVC. The portable
  branch targets Unix-likes first; MSVC/CMake support exists but is
  second-tier and historically churns across releases. Would need its
  own static-link proof on this exact toolchain. `[open-measure 1]`.
- License: `[knowledge]` ISC plus BSD-style files — MIT-compatible.
- Client+server coverage: `[knowledge]` adequate via `libtls` plus
  `libssl`/`libcrypto`, but `libtls` is primarily a client-side
  simplification API; server-side coverage is thinner and less
  exercised than OpenSSL's or rustls's. Phase 6 needs a
  TLS-terminating server (PHASE5_PRODUCTION.md section 2 will grow a
  TLS leg), so this asymmetry counts against it.
- Auditability of binding: `[knowledge]` smaller and cleaner than
  OpenSSL's (`libtls` is a deliberately narrow API), but the crate
  story (`libressl` bindings) is thinner and less maintained than the
  other candidates' — binding-maintenance risk, not binding-size risk.
- Packaging weight: `[knowledge]` lighter than OpenSSL, heavier than a
  cargo dep; still a vendored-C build with its own portability patches.

### 3.3 mbedTLS / TF-PSA (vetted-C)

- Static-link story: `[knowledge]` best of the vetted-C shortlist.
  CMake build with first-class MSVC support, designed for static
  linking and footprint tuning via a compile-time config header — the
  closest C-side match to the DEPLOYMENT.md section 2 goal. Still
  needs CMake on the build machine (unprobed here) and its own static
  proof. `[open-measure 1]`.
- License: `[knowledge]` dual Apache-2.0 OR GPL-2.0-or-later — usable
  only under an explicit, recorded Apache-2.0 election. The election
  must be minuted in the lock amendment; a silent default here is the
  exact license-conflict reopen trigger ADR-023 names.
- Client+server coverage: `[knowledge]` full client and server sides,
  TLS 1.2 and 1.3, with a small-RAM heritage that fits the
  minimal-footprint goal. PSA Crypto API direction (TF-PSA rebrand)
  means the API the binding targets must be pinned at lock time.
- Auditability of binding: `[knowledge]` narrow C API shaped for
  embedding; the `mbedtls` crate surface is small. The `unsafe`
  boundary review would be modest — closest a C candidate gets to the
  FFI.md section 6 ideal (isolated, auditable, not scattered).
- Packaging weight: `[knowledge]` lightest vendored-C option;
  config-header tuning can strip unused cipher suites. Still a C build
  step where a cargo dep has none.

### 3.4 rustls (pure-Rust challenger)

- Static-link story: `[measured for the mechanism]` pure cargo dep —
  static linking is the default output on `x86_64-pc-windows-msvc`
  with no C toolchain step for rustls itself. Provider caveat
  `[knowledge, must measure]`: the crypto provider decides the real
  story — `aws-lc-rs` (default) brings C++/assembly and a
  CMake-or-prebuilt decision; `ring` brings C/assembly via the already
  proven `cc` path (section 2). Provider selection is therefore part
  of the lock, not a detail. `[open-measure 2]`.
- License: `[knowledge]` MIT / Apache-2.0 / ISC — MIT-compatible with
  no election.
- Client+server coverage: `[knowledge]` both sides as first-class APIs
  (`rustls::ClientConnection`, `rustls::ServerConnection`), TLS 1.2
  and 1.3 only (no legacy versions — a fit, not a gap, for new Phase 6
  work). Windows platform trust-store integration needs a verifier
  crate decision (bundled roots vs platform verifier) at lock time.
  `[open-measure 3]`.
- Auditability of binding: no C binding at all — no `unsafe` boundary
  to review under FFI.md section 6, no C-ABI mapping table (FFI.md
  sections 3-5) to maintain. Supply-chain review shifts to the
  TOOLCHAIN.md section 3 track (integrity/signing, `c-shim`-vs-`native`
  tier labeling for `noct audit`), where the `ed25519-dalek`/`sha2`
  precedent already exists.
- Packaging weight: `[knowledge]` lightest — one cargo dep plus
  provider; no vendored build, no generated headers, no extra build
  tool (conditional on the `ring`-provider measurement in
  `[open-measure 2]`).

## 4. Postgres-driver scorecard

ADR-023 driver criteria: wire-protocol completeness;
transaction+savepoint support; fit with the M5 executor story;
packaging weight; plus the shared `unsafe`-boundary review and
`noct audit` tier labels.

Executor context `[measured from docs]`: the M4 floor is blocking OS
threads, one per connection/task (PHASE5_PRODUCTION.md sections 1-3;
CONCURRENCY.md section 4 as cited there), and the non-blocking M5
executor is deferred. The driver arrives with that executor as one
designed unit (IMPLEMENTATION_PLAN.md Phase 5 item-4 amendment), so a
driver that works correctly under blocking threads TODAY and needs no
second runtime TOMORROW has a structural advantage under ADR-009 (one
official async runtime; no competing executors).

### 4.1 libpq via FFI (plan's assumption)

- Wire-protocol completeness: `[knowledge]` complete by definition —
  the reference client library. Every auth method, startup parameter,
  extended-protocol path, `COPY`, and `LISTEN`/`NOTIFY` the server can
  offer, libpq speaks, including future server-side additions.
- Transaction+savepoint support: `[knowledge]` full (`PQexec` of
  `BEGIN`/`COMMIT`/`ROLLBACK`/`SAVEPOINT`, plus single-row and pipeline
  modes in recent versions). Would arrive as designed with the driver,
  per the item-4 amendment.
- M5 executor fit: mixed. `[knowledge]` The blocking API fits the M4
  blocking-threads floor trivially; the non-blocking API
  (`PQconnectPoll`, `PQsocket` + readiness notification) integrates
  with a custom executor without importing a second runtime — but every
  use crosses the FFI.md section 6 ownership boundary (connection
  handles are owned C pointers; acquire/release must be explicit
  boundary functions), and async error paths must translate foreign
  failure into `Result::Err` at the boundary (the M7 foreign-bridge
  rule in IMPLEMENTATION_PLAN.md Phase 8 states the pattern this
  project already demands).
- Packaging weight: `[knowledge]` heaviest driver option on Windows. No
  OS-bundled libpq; sourcing means a vendored libpq build (needs
  flex/bison, Perl, OpenSSL — strictly worse than the OpenSSL-only
  build in section 3.1) or a binary redistribution (vcpkg/EDB
  installer) that breaks the hermetic offline build and the
  DEPLOYMENT.md section 2 static-footprint goal. `[open-measure 4]`.
- `unsafe` boundary / tier: full FFI.md section 6 review; ships as a
  `c-shim` tier dependency in `noct audit` (TOOLCHAIN.md section 3,
  ADR-015) — same default trust as native once the boundary is
  audited, but the audit itself is the cost.

### 4.2 Pure-Rust Postgres crate (challenger: `postgres` blocking facade over the `tokio-postgres` wire core)

- Wire-protocol completeness: `[knowledge]` production-complete for
  Phase 6 needs — SCRAM-SHA-256 auth, extended query protocol,
  prepared statements, `COPY`, `LISTEN`/`NOTIFY`. It tracks libpq
  rather than defining the protocol, so server-side novelties land
  here later by construction. `[open-measure 5]`.
- Transaction+savepoint support: `[knowledge]` first-class client API
  (`transaction()`, nested savepoints) in the blocking facade —
  exactly the "one designed unit" the item-4 amendment requires, with
  no SQL-string plumbing leaking into `.nv` user code.
- M5 executor fit: best of the shortlist. `[knowledge]` The blocking
  facade runs correctly on plain OS threads (the M4 floor) while
  hiding its I/O driver internally, so it needs no Tokio reactor in
  Noctivue code and imports no second executor under ADR-009. The raw
  `tokio-postgres` async core is explicitly NOT recommended: it would
  require a Tokio runtime inside the process, a direct conflict with
  the one-official-runtime rule. Whether the facade's hidden internal
  runtime needs an ADR-009 interpretation note is flagged for the lock
  amendment. `[open-measure 6]`.
- Packaging weight: `[knowledge]` lightest — pure cargo dep, static by
  default, no system library, no binary redistribution. Best fit for
  DEPLOYMENT.md section 2.
- `unsafe` boundary / tier: none — same posture as the in-tree
  `ed25519-dalek`/`sha2`/`rusqlite` host-layer deps. Trust rides
  TOOLCHAIN.md section 3 signing/integrity enforcement, not an
  `unsafe` audit.

## 5. Packaging probe results

- Probe A (PASS, `[measured]`): scratch crate `spike_cc_shim` in
  `Temp\opencode`, built with `cargo test --offline`: `build.rs` via
  cached `cc 1.4.5` compiled a C file with an explicit
  acquire/ping/release ownership shape, producing both
  `libspike_shim.a` and `spike_shim.lib`, and the Rust `extern "C"`
  round-trip test passed. MSVC auto-detection worked with `cl.exe`
  NOT on `PATH`. Conclusion: the vendored-C static-link MECHANISM on
  this toolchain is proven and reproducible — but only for C sources
  that build through `cc` with zero extra build tools. Nothing about
  OpenSSL's Perl/NASM build, LibreSSL's portability shims, mbedTLS's
  CMake step, or a libpq vendored build was exercised; each of those
  is strictly harder than this probe and remains open.
- Probe B (BLOCKED, `[measured]`): scratch crate depending on
  `rustls = "0.23"` under `cargo build --offline` fails at resolution
  with the exact error `no matching package named 'rustls' found;
  location searched: crates.io index`. The same block applies to every
  new-crate candidate (any TLS crate, any Postgres crate, any new
  provider): NO candidate can be fetched, compiled, or measured in
  this offline spike. This is the honest boundary of the evaluation.
- Corroborating artifact (`[measured]`): `sqlite3.lib` in the repo
  `target/` tree plus the passing `cargo build --offline -p noct-cli`
  prove the hermetic-build property any vehicle must preserve: after
  adding the pick, the full offline build must still succeed from the
  committed lockfile with no network and no installer step.

Missing-measurement list (each names the exact blocker, none guessed):

1. Vendored-C static builds on this toolchain: OpenSSL (needs
   network for `openssl-src` + Perl + NASM), LibreSSL portable
   (needs network + CMake + portability-patch assessment), mbedTLS
   (needs network + CMake), libpq (needs network + its full build
   inputs or a binary-redistribution decision). Blocker: network +
   uninstalled build tools.
2. rustls crypto-provider decision: `ring` (rides the proven `cc`
   path) vs `aws-lc-rs` default (C++/prebuilt/CMake question) —
   binary-size and build-time deltas for each. Blocker: network.
3. Windows trust-store story for rustls (bundled roots vs platform
   verifier) and its `noct audit` tier/label treatment. Blocker:
   network + verifier-crate API read.
4. libpq sourcing decision for Windows (vendored build vs pinned
   binary redistribution) with size, provenance, and signing
   implications under TOOLCHAIN.md section 3. Blocker: network.
5. Pure-Rust driver protocol-conformance run against a real
   Postgres (SCRAM, extended protocol, COPY, NOTIFY, savepoints).
   Blocker: network + a test Postgres instance.
6. M5-executor integration sketch for each driver finalist,
   including the ADR-009 interpretation of the blocking facade's
   internal runtime. Blocker: design time, not network.

## 6. Ranked recommendation (for the ADR-023 lock amendment — NOT a lock)

TLS: 1) rustls (provider pinned by `[open-measure 2]`) —
2) mbedTLS/TF-PSA (under recorded Apache-2.0 election) —
3) OpenSSL — 4) LibreSSL.
Driver: 1) pure-Rust `postgres` blocking facade —
2) libpq via FFI. The raw `tokio-postgres` async core is ranked
below both: importing a Tokio reactor conflicts with ADR-009.

Rationale in one breath: both first picks preserve the two properties
this spike could actually verify — hermetic offline cargo builds and
zero new build tools — while the C options each demand at least one
unproven build input (Perl/NASM, CMake, or a binary redistribution)
that Wave 4 static deployment would then carry forever; and the
in-tree precedent (`ed25519-dalek`/`sha2` pure-Rust crypto trusted for
signing, `cc`-vendored C trusted for SQLite) already accepts
pure-cargo deps as auditable host-layer dependencies, so the plan's
"FFI-bound" wording buys review cost without buying trust the project
does not already grant.

The single strongest counter-argument to this pick: both
recommendations reimplement protocols whose reference lives in C, so
anything the reference gets first — a new Postgres auth method or
server-side protocol turn (bites the driver), a FIPS-certified module
or platform TLS-policy integration an enterprise customer mandates
(bites TLS), or a Windows-specific integration the pure-Rust crates
deprioritize — reopens the contest on the implementer's schedule, not
ours; the plan's FFI-bound assumption was precisely a hedge against
reference-lag, and if Phase 6 enterprise needs (Kerberos/GSSAPI,
certified crypto, OS store policy) materialize, the hedge was
correctly priced and this ranking falls.

## 7. What Wave 1 implementation must measure to confirm or reopen it

Confirm measurements (all network-gated; run before the lock
amendment): fetch and `cargo build` each finalist on this
Windows/MSVC toolchain; record clean-build time and `spike_shim`-style
round-trip tests; record release binary-size deltas for rustls+`ring`
vs rustls+`aws-lc-rs` vs mbedTLS-vendored; run a TLS client+server
handshake interop matrix (each vehicle against the others and against
one external peer); run the pure-Rust driver against a real Postgres
through transactions, savepoints, concurrent pooled use, and one
`COPY` plus one `LISTEN`/`NOTIFY` leg; pin every license text and any
dual-license election for `noct audit`.

Reopen triggers (ADR-023's lock rule, restated as checks): throughput
shortfall under a PHASE5_PRODUCTION.md section-5-style load harness;
ANY static-link failure (missing build tool, non-hermetic step, or
binary redistribution that breaks the section-5 offline-build proof);
any license conflict or unrecorded dual-license election; any
protocol-conformance failure the C reference passes; any M5-executor
integration that requires a second runtime (revives libpq or reopens
the driver contest, and forces the ADR-009 interpretation note).

## 8. Boundaries observed (not evaluated)

PHASE5_PRODUCTION.md section 6 non-requirements were treated as hard
walls: no auth/authorization design, no replication/failover, no ORM —
a driver-side auth-method remark above concerns only which wire bytes
the vehicle speaks, never Noctivue auth semantics. FFI.md section 9
(C++ shim tier) is out of scope: none of these vehicles needs it, and
no conclusion here depends on it.
