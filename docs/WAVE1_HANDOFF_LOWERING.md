**Status:** Handoff to the backend track — Wave 1 status-aware client + connection caps. Not normative spec; the normative record stays `docs/DECISIONS.md` + `docs/PHASE5_PRODUCTION.md`.

# Wave 1 Handoff — Lowering-Track Follow-up

## Context

Wave 1 delivered status-aware handler *returns* (`HttpResponse`
struct or `String`→200, both runtimes, shared wire layer,
11/11 `http_load`, 19/19 parity). Two items could not land without
new `Instr` variants, and per the mirror rule no interp-only
builtins shipped: the status-aware **client** and **connection
caps**. This note specifies them for the track that owns
`compiler/src/nir/lowering.rs`, `compiler/src/nir/instr.rs`, and
the Cranelift backend.

## 1. `HttpSendFull` — status-aware client

- New instruction `HttpSendFull { dst, method, url, headers, body }`
  returning `Result<HttpResponse, String>`, reusing the
  `HttpResponse` struct value (`status: Int, reason: String,
  headers: [[String]], body: String` — the existing
  `stdlib/net/http/request.nv` shape; typeck accepts it via the
  Named-type passthrough precedent, SPEC.md C6, as already proven
  by struct-returning handlers).
- New builtins `http_send_full_builtin(method, url, headers, body)`
  with typeck signatures: interpreter + VM arms mirroring
  `http_send_builtin` exactly (same plaintext discipline, same
  `https://` refusal, same loud arg errors).
- `.nv` wrappers in `stdlib/net/http/client.nv` (`http_send_full`,
  plus `http_get_full`/`http_post_full` conveniences if they pay
  for themselves) with `///` docs.
- Native backend: loud rejection entries (same contract as every
  host-IO builtin), never silent lowering.

## 2. `HttpServerSetLimits` — connection caps

- New instruction `HttpServerSetLimits { server, max }` plus
  builtins `http_server_set_limits(srv, max_conn)` (typeck sigs,
  interp + VM arms) and a `server.nv` wrapper. Default: current
  unlimited behavior (zero existing-test churn).
- Over-cap behavior: accept-and-answer **503** immediately
  (`reason_phrase` + `response.nv` already carry 503) — every
  connection stays terminal for load accounting, never silently
  dropped. Log server-side via `eprintln` (matches the handler-500
  precedent; aids operators).
- Negative/zero caps are loud `Err`s, not silent clamps.

## 3. Tests to extend (all existing patterns, no new harness)

- `tests/http_load.rs`: full-send round trip (status + headers +
  body), caps (low cap + excess parallel conns → 503s, server
  survives, valid traffic after), VM parity for both paths
  (`run-vm` server/client combos per `tests/parity_hostio.rs`
  patterns — read, do not modify, that file).
- `cargo test --test http_load`, `--test http_test`,
  `-p noct-cli --test parity_hostio`, `-p compiler --lib`,
  `-p interp --lib` stay green; `noct diagnostics/lint/fmt
  --check` clean on touched `.nv` files.

## 4. Docs to update on landing

- `stdlib/SPEC.md` amendment log (client surface + caps
  semantics + defaults).
- `docs/PHASE5_PRODUCTION.md` §6 (mark client + caps delivered;
  keep TLS/auth/metrics/rate-limiting listed).
- `docs/DECISIONS.md` only if a surface name changes (ADR-017
  rule for renames).

## 5. Terms

- Proposed time-box: 5 working days from handoff. On expiry
  without pickup, Wave 2 absorbs this work (the executor effort
  touches the same runtime files; one coordinated push then
  replaces two merges).
- Mirror rule holds throughout: both runtimes or it does not
  land. Review must reject interp-only builtins.
- No scope: TLS, driver, transactions, rate limiting,
  percent-decoding, or any other Phase 6 item.
