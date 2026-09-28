# PHASE5_PRODUCTION.md — M4 Production Semantics (Phase 5 Build Contract)

**Status:** Normative for Phase 5 implementers. Exit criteria are those of
`docs/IMPLEMENTATION_PLAN.md` §7 (Phase 5/M4): (1) a CRUD REST API on a real
DB using only public stdlib + the package manager; (2) correct behavior under
concurrent load with no crashes, deadlocks, or leaks; (3) native ownership
discipline holds for connection/socket/file lifetimes under load (sanitizer or
equivalent leak pass). Reference DB is SQLite, file-backed, per ADR-019
(`docs/DECISIONS.md`). Where current code already satisfies a rule, the path
is cited and the rule is a *hold-the-line* gate, not new work.

Global discipline (applies to every section): **loud errors, never silent.**
Every failure below surfaces as a `Result::Err` naming the cause or a
diagnostic with a code — never a default value, never a dropped request, never
a swallowed close. `run-vm` and native `build` MUST keep failing loudly
(`unknown-runtime-symbol`) on host-IO builtins they do not yet lower, per the
established pattern in `stdlib/db/sqlite.nv` — silent VM/native divergence is
a release-blocking bug.

## 1. Pool semantics (`stdlib/db/pool.nv`)

The checkout/checkin protocol in `stdlib/db/pool.nv` already satisfies the M4
shape and MUST NOT be redesigned: a `Pool` is a plain value threaded through
calls (no global mutable state); `pool_open` / `pool_checkout` /
`pool_checkin` / `pool_close` / `pool_run` are the complete surface.

- **Checkout/checkin:** `pool_checkout` reuses the idle slot when filled,
  else opens fresh via `checkout_fresh`. Every checked-out `Db` MUST be
  returned via `pool_checkin` (or consumed by `pool_run`) on all paths,
  including error paths — leak-the-handle-on-`Err` is a test failure.
  `pool_checkin` with an occupied slot closes the incoming connection and
  keeps the old one (`close_keep_slot`) — already implemented; keep.
- **Max-size + exhaustion:** `pool_open` with `max_size < 1` MUST return
  `Err("pool max_size must be at least 1")`. Checkout at cap MUST return
  exactly `Err("pool exhausted (max_size reached)")` — already implemented;
  tests MUST pin both strings byte-for-byte so callers can match on them.
- **Idle eviction:** explicitly NONE in M4. One idle slot, no timers, no
  background reaper — the M4 runtime is blocking OS threads
  (`docs/CONCURRENCY.md` §4) and there is no executor to run eviction on.
  Idle connections live until `pool_close`. Eviction policy is Phase 6 work.
- **Thread/task safety:** `Pool`/`Db` values MUST NOT cross a task boundary.
  Handles are per-interpreter registry ids and a task worker gets fresh,
  empty `db`/JSON/task registries (`docs/CONCURRENCY.md` §4;
  `interp/src/lib.rs::for_task`). The reference app MUST open and use its
  pool on the thread that serves (handler thread), never in `main` for use
  in handlers. Passing a `Db`/`PoolCheckout` into a `task` call is a loud
  runtime error (`unknown database handle ... (was it closed?)`), never
  cross-thread use.
- **Shutdown with checked-out connections:** `pool_close` closes the idle
  slot only and reports `Ok(1)` / `Ok(0)` — already implemented; keep.
  Checked-out connections stay the borrower's responsibility: the reference
  app's shutdown path MUST check every connection back in before
  `pool_close`, and the load test MUST assert final `open` accounting
  returns to its pre-run value. `pool_close` MUST NOT invalidate
  checked-out handles.

## 2. HTTP server production shape (`stdlib/net/http/server.nv`, `interp/src/lib.rs` serve loop)

- **Concurrency model:** one task per SERVER today (serve loop runs in a
  `task`; connections are handled sequentially inline — `interp/src/lib.rs`
  `http_server_serve_loop`). Phase 5 MUST promote this to **one task (OS
  thread) per connection**: the accept loop MUST never block on a handler.
  Sequential handling serializes the load test and hides handler-lifetime
  bugs the exit criteria exist to catch. Handler dispatch stays
  body-in/body-out (`http_server_route`); status-aware handlers are
  Phase 6 (§6).
- **Request limits (all MUST, all tested):** header block cap **8 KiB**;
  body cap **1 MiB** (both measured on the wire bytes). Over-cap bodies
  MUST get `413` with reason `Payload Too Large` (add the mapping to
  `http_reason_phrase` in `stdlib/net/http/response.nv`, which today falls
  through to `"Unknown"`), a `Connection: close`, and no handler
  invocation. The current single-`read` 4096-byte buffer MUST be replaced
  by a bounded incremental reader — silent truncation is forbidden.
- **Malformed requests:** `400 Bad Request`, connection closed, server
  keeps serving — already implemented (`interp/src/lib.rs` serve loop);
  hold the line. A malformed request MUST NEVER panic the server task,
  poison the route table, or alter pool state. Fuzz the request line +
  header block in tests (garbage bytes, missing version, NULs, over-long
  single line).
- **Graceful stop: DRAIN in-flight.** `http_server_stop` sets the shutdown
  flag (already implemented); Phase 5 MUST additionally (a) stop accepting
  new connections immediately, (b) let already-accepted connections run to
  response-completion, (c) return from `http_server_serve` only after all
  connection tasks have joined. Drop-on-stop is rejected: it turns every
  deploy into client-visible `500`s and makes the load test's
  zero-failure accounting meaningless.
- **Route precedence:** exact `method + path` equality, **first-registered
  wins**, unmatched → `404 Not Found` — already implemented; hold the line.
  No prefix/glob matching in M4 (`path_pattern` is an exact path despite
  its name). Handler returning a non-`String` MUST become `500`, not the
  current `200 "handler error"` body — a 200 for a failed handler is a
  silent lie.

## 3. Task/cancellation semantics (`stdlib/concurrency/task.nv`, `docs/CONCURRENCY.md`, `interp/src/lib.rs::spawn_task/join_task/drain_tasks`)

- **`await` on a failed task** yields the task body's failure VALUE, not a
  trap: `EarlyReturn`/`UncaughtError` map to values at the thread boundary
  — already implemented in `spawn_task`; hold the line. Concretely, a task
  returning `Err(e)` awaits to `Err(e)` in the awaiter, composable with `?`.
- **Panic propagation:** a Noctivue-level failure (`Panic`, e.g. failed
  `assert`, negative `sleep`) inside a task fails the awaiter with
  `task {id} panicked (worker thread died)`-class loud errors; un-awaited
  task failures are reported at scope drain (`background task {id}
  failed`, exit 1) — already implemented in `join_task`/`drain_tasks`;
  hold the line and pin the strings. There is deliberately no way to
  "catch" a sibling task's panic except by awaiting it.
- **No cancellation in M4.** There is no cancel primitive
  (`stdlib/concurrency/{channel,mutex,sync,atomic}.nv` are reserved;
  `SPEC.md` §9) and none is added: cancellation arrives with the M5
  non-blocking executor (`docs/CONCURRENCY.md` §4). Tasks run to
  completion; scopes join at exit (no fire-and-forget — already
  implemented). Implementers MUST NOT invent an ad-hoc kill flag.
- **Sleep is at-least, never exact:** `sleep(ms)` blocks the calling OS
  thread for *no less than* `ms` (already implemented via
  `std::thread::sleep`; negative inputs panic — `stdlib/concurrency/task.nv`).
  Tests MUST assert `elapsed >= ms` with generous upper bounds only; exact
  or upper-bound timing assertions are forbidden (flaky by construction).
- **Spawn limits:** none at the language level in M4 (one OS thread per
  task call). The server's per-connection threads (§2) are therefore
  bounded by the OS, not the runtime — the load-test client counts in §5
  are chosen to fit comfortably inside that, and connection-count
  caps/backpressure are Phase 6 (§6), not a hidden M4 limiter that fails
  the load test opaquely.

## 4. DB discipline (`stdlib/db/sqlite.nv`, `interp/src/lib.rs` `db_*_builtin`)

- **Binding is the ONLY value path.** `db_exec`/`db_query` take values
  exclusively as a JSON array of scalars in `params` — already implemented
  and documented (`stdlib/db/sqlite.nv`: "never interpolate values into
  SQL text"). Enforcement posture is **convention + test, plus the runtime
  guards that already exist**: non-array params fail loudly (`params must
  be a JSON array like ...`), non-scalars fail loudly (`unsupported JSON
  param ... (scalars only)`), unknown/closed handles fail loudly
  (`unknown database handle ... (was it closed?)`) — all in
  `interp/src/lib.rs`; hold the line. True interpolation *detection* is
  impossible at runtime (a concatenated string is just a string), so the
  reference app MUST additionally carry a test that greps its own sources
  for string-building into SQL argument positions, plus review. No new
  runtime guard is required.
- **Transactions: OUT OF SCOPE for M4.** Single SQLite statements are
  atomic, every M4 CRUD handler is expressible as single-statement reads
  and writes, and handler DB access is task-local (§1), so no M4 exit
  criterion needs multi-statement atomicity; adding `begin/commit/rollback`
  now would ship half a contract (no savepoints, no isolation-level story,
  no interaction with the Phase 6 client-server driver) that the reference
  app would then depend on. Revisit with the Postgres-class driver in
  Phase 6, as one designed unit.
- **JSON interchange edge cases (all MUST be tested):** scalars bind as
  `Text` / `Integer(1|0)` for `true`/`false` / `Null` / `i64` / `f64`
  (`parse_json_params`); rows render per `render_json_value` (ints bare,
  reals shortest-round-trip so `42.0` never becomes `42`, text quoted,
  blobs as quoted lowercase hex, NULL as `null`). Nested values: objects/
  arrays are rejected as params — nest via `doc_get_doc`/`JsonDoc`
  (`stdlib/encoding/json/document.nv`), tested round-trip. Unicode:
  full round-trip including `\uXXXX` and surrogate pairs (the doc parser
  already handles both; pin with a test). Large ints: bind path is `i64`
  — tokens beyond `i64` range fall through to `f64` with precision loss,
  while the language `Int` is `i128`: values outside `i64` MUST be a loud
  `Err` or a documented-and-tested coercion, never silent truncation;
  closing this gap (reject vs. widen) is required Phase 5 work. Doc
  parsing stays depth-capped (max 100, `nesting too deep`) — network and
  DB text is untrusted input by definition.

## 5. Load-test acceptance spec

- **Harness shape (normative):** a Rust integration test (sibling to
  `tests/http_test.rs` / `tests/async_test.rs`) drives the reference CRUD
  app built from public stdlib only: **N = 16 concurrent clients × M =
  128 requests each (2048 total)** against a fixed port, mixing
  create/read/update/delete plus `GET /health`-style reads, plus a
  background trickle of unknown routes (expect `404`) and malformed bytes
  (expect `400`, server survives). Clients use raw TCP or the stdlib
  client through `noct run` tasks — either is acceptable; raw TCP is
  preferred for the malformed leg.
- **Pass thresholds:** ZERO crashes, ZERO deadlocks (every request gets a
  terminal response within a 30 s watchdog — a hung request fails the
  run), ZERO leaks (see below). Latency is **RECORDED, not thresholded**
  at this stage: the harness MUST print p50/p99 per endpoint to stdout
  for the report, but no p50/p99 value fails the run — the M4 runtime is
  the blocking-threads floor (`docs/CONCURRENCY.md` §4) and thresholding
  it now would gate the milestone on tuning, not correctness.
- **File-backed DB (ADR-019, binding):** the load run MUST target a
  temp-file database, never `:memory:` alone; after the run the harness
  MUST assert `PRAGMA integrity_check = ok` and spot-read written rows
  back. An additional `:memory:` smoke run is allowed but does not count.
- **Sanitizer/leak method (Windows-compatible, no new toolchain):** (a)
  pool accounting returns to pre-run values (`open` delta zero, idle slot
  exactly one connection, `pool_close → Ok(1)`); (b) process thread count
  sampled via `std` before/after returns to baseline (connection tasks all
  joined per §2-drain); (c) the Rust test harness's own leak detection
  plus a second consecutive load cycle with no growth in peak RSS trend
  and no `unknown database handle` / `already awaited` diagnostics. ASan
  is NOT required (MSVC-hostile for this toolchain); (a)–(c) plus
  `integrity_check` are the M4 leak pass. Native-mode ownership proof for
  handle lifetimes rides on (a)–(c) until the sibling backend track
  delivers sanitizer-ready native `db`/`http` lowering.

## 6. Explicit NON-requirements for M4

Each item below is OUT for Phase 5. Implementers MUST NOT build them; reviewers
MUST reject them as scope creep.

- **TLS / `https://`** → Phase 6. Client refuses loudly today
  (`unsupported URL scheme (expected http://)`); server is plaintext-only.
- **Authentication / authorization** → Phase 6. (The `AuthResult` shapes in
  `examples/noctivue-expert-server-service.nv` are illustrative only.)
- **Replication / failover / client-server DB driver** → Phase 6, with the
  deferred Postgres-class driver (ADR-019).
- **Rate limiting / backpressure / connection-count caps** → Phase 6
  resilience work (`IMPLEMENTATION_PLAN.md` §8.3).
- **HTTP/2** → Phase 6 stretch at earliest; HTTP/1.1 close-delimited is
  the M4 wire.
- **Status-aware client + handler status codes** → Phase 6 (client returns
  body-only `Ok` today, `stdlib/net/http/client.nv`).
  - *Update 2026-09-26 (phase 6/wave 1):* the HANDLER half is
    delivered — handlers may return `HttpResponse` struct values
    for exact-status responses (plain `String` still means 200;
    unknown/non-String/Err/panic still 500; wire table extended).
    The CLIENT half (`HttpSendFull`) plus connection caps stay
    deferred: they need new `Instr` + lowering + Cranelift entries
    owned by the backend track (follow-up spec in the wave
    record) — no interp-only builtins per the mirror rule.
- **ORM** → never core; ecosystem territory (`IMPLEMENTATION_PLAN.md` §7.4).
- **Metrics / tracing / OTel, structured timestamps, `noct audit`
  enforcement, sandboxing** → Phase 6 (`IMPLEMENTATION_PLAN.md` §8.1–8.2).
- **Idle-pool eviction, transactions, cancellation, spawn limits** →
  deferred per §§1/3/4 above, not forgotten.
  - *Update 2026-09-26 (integration audit — partial M5 core already
    runs; recorded rather than left to contradict the code):* two of
    these four are no longer purely deferred. `spawn limits` is
    **live**: `interp/src/lib.rs::spawn_task` refuses loudly past
    `executor_worker_count()` (default `available_parallelism` capped
    at 64, `NOCT_WORKERS` override, loud on malformed values), and
    `join_task` admits through a bounded blocking bridge with a 30 s
    watchdog. `cancellation` is now **live end to end**:
    `task_cancel_builtin(id)` (typeck `(Int) -> Unit`, lowered to
    `Instr::TaskCancel`, implemented in the interpreter, wrapped as
    `task::task_cancel` in `stdlib/concurrency/task.nv`), so a program
    can set the flag; the `sleep` cancel checkpoint observes it and
    `await` yields the pinned `Err(task {id} cancelled)`. Parity is
    interpreter-only — the NIR VM spawns no tasks and Cranelift
    rejects `Instr::TaskCancel` loudly, so `noct run-vm` / `noct build`
    refuse any `task` declaration. The `TicklessTimerHeap` and
    `timer_now_ms` remain orphaned (no users, no tests). Idle-pool
    eviction and multi-statement transactions remain genuinely
    deferred. Full accounting and the owner action required are in
    `docs/DECISIONS.md` ADR-024's "Implementation status"; the shipped
    vs. Wave-2 split is also in `CONCURRENCY.md` §4. This audit changed
    no executor code; note also that `spawn limits` being live is a new
    observable loud failure not listed in ADR-024's
    explicitly-breaking list.
