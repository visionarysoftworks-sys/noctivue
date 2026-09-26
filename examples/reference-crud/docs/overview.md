# reference-crud — overview

Phase 5 reference application: a CRUD REST API backed by a real
(file) SQLite database, built end-to-end from public stdlib plus
the package manager (`docs/IMPLEMENTATION_PLAN.md` §7 exit
criterion 1, `docs/PHASE5_PRODUCTION.md`).

## Run it

From this directory (stdlib files first, entry point last — the
pre-import distribution model, `stdlib/SPEC.md` §1):

```text
noct run ../../stdlib/core/convert.nv ../../stdlib/core/compare.nv \
  ../../stdlib/option/option.nv ../../stdlib/result/result.nv \
  ../../stdlib/strings/string.nv ../../stdlib/strings/format.nv \
  ../../stdlib/testing/assertions.nv ../../stdlib/net/http/request.nv \
  ../../stdlib/net/http/response.nv ../../stdlib/net/http/client.nv \
  ../../stdlib/net/http/server.nv ../../stdlib/db/sqlite.nv \
  ../../stdlib/db/pool.nv lib/main.nv
```

Serves 60 s on port 18080, keeping `crud.db` next to the working
directory (git-ignored). The port is one line in `lib/main.nv`
(`app_port`) if 18080 is taken.

## Test it

- `noct test` — the self-contained serve smoke
  (`tests/crud_smoke.nv`: builtin-level, no stdlib needed).
- `cargo test --test http_load` (workspace root) — the full
  production proof: limits (413/400), 500 mapping, fuzz survival,
  concurrency overlap, and the 16×128×2 CRUD load with pool
  accounting, `integrity_check`, spot reads, and zero leak
  deltas. The harness derives its copy of this app from
  `lib/main.nv` (temp database, ephemeral port, shorter budget),
  so proof and package can never diverge; this package is the
  human-runnable original.

## Design rules held (contract pins)

- Binding-only SQL: values bind from JSON arrays; a self-grep
  test (`reference_app_binds_never_interpolates`) rejects
  interpolation in the harness copy.
- Task-local pools: every handler opens, uses, and closes its
  own pool; error paths check back in before closing
  (leak-on-`Err` is a contract failure).
- Body-in/body-out handlers, exact `method + path` routes,
  first-registered wins — the M4 server shape. Status codes,
  headers, and TLS are Phase 6 work, deliberately absent here.
