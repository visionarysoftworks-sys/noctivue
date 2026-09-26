# CONCURRENCY.md — Concurrency Model

**Status:** M4 surface Confirmed (task/await/sleep); async fn Deferred.

## 1. Structured Concurrency (M4 floor)

Every task has a bounded lifetime tied to the scope that spawned it.
There is no "fire and forget" — a task's parent scope **MUST** join
(or the runtime joins it at scope exit). Detaching is not yet in the
surface.

## 2. Core Keywords (M4: task/await)

```nv
task fetch_status(url: String) -> Int:
    sleep(50)
    200

fn main():
    let h = fetch_status("https://example.com")
    let code = await h
    println("status={code}")
```

- `task` introduces a top-level concurrent unit; calling it spawns
  a new OS thread and returns an opaque join-handle (`Int`).
- `await` blocks the caller until the task completes and yields the
  task body's value. Awaiting anything other than a live handle fails
  loudly; handles are single-use.
- `async fn` is **Deferred** (M5+): the current surface is `task` +
  `await` + `sleep` only. The parser has `Token::Async` reserved; the
  interpreter has no `async` lowering yet.

## 3. Task Lifetime & Hierarchy

- A `task` is always top-level; local `task` declarations type-check
  (bound to `Int` handle) but calling them fails loudly with
  `undefined name` — lift to top level to spawn.
- A scope's exit (normal return, early return, or error) **joins all
  outstanding tasks** before the process exits (implemented in
  `Interpreter::drain_tasks`). No child task outlives its parent scope.
- Failure propagation: a task that panics or returns `UncaughtError`
  fails the awaiting scope with the same value; un-awaited task
  failures are reported at drain time.

## 4. One Official Runtime (M4 floor: blocking threads; M5: executor per ADR-024)

- M4 floor (today, unchanged): real OS threads
  (`std::thread::spawn`), one per task call. This is the
  structured-concurrency floor, not a tuned executor.
- No shared mutable state crosses threads: arguments and return
  values are the only channel. Each worker gets a fresh interpreter
  with empty `db`/JSON/task registries (handles are meaningless
  across threads).
- M5 executor (ADR-024, merging `docs/EXECUTOR_DESIGN_DRAFT.md`):
  a fixed work-stealing pool (default `available_parallelism`
  capped at 64, `NOCT_WORKERS` override) plus ONE readiness thread
  owning the non-blocking listener/connection set and the tickless
  mono-ms timer heap (ADR-020), plus a bounded blocking bridge
  (default cap = worker count, queue-full is a loud `Err`) for
  SQLite calls, legacy blocking sections, and sync-`await` joins.
  `task`/`await` source semantics are preserved; the
  thread-per-connection model is retired with no dual-runtime flag
  period. Connections become executor tasks; the M4 wire contract
  is unchanged.

## 5. Channels & Synchronization (M5 surface per ADR-024)

Library API over executor handles — never new keywords. All loud
on misuse:

- `oneshot() -> (Sender, Receiver)`: single-value rendezvous for
  request/reply; dropped-sender recv is `Err`, never `Unit`.
- `channel(cap: Int) -> (Sender, Receiver)`: bounded MPMC queue;
  send on full / recv on empty suspend (backpressure, never
  silent drop); use after close is a loud `Err`; `cap < 1` is a
  loud `Err` at construction. Unbounded channels are rejected —
  every queue has a cap.
- `mutex<T>(v: T)` + scoped guards: `lock(m)` suspends (never
  blocks a worker) until acquired; guards borrow and cannot
  escape the lock scope; `try_lock` returns `None` instead of
  suspending; no reentrant locking (loud `Err`, never deadlock).
- `sleep_async`, `timeout(ms, handle)`, and `hedge` live one layer
  up as blessed packages per IMPLEMENTATION_PLAN.md Phase 6 item
  3 (composed from oneshot + channel + timers), not language
  features. A `select` helper is deferred (ADR-024: reopened only
  on measured evidence).
- The multiplexed pool keeps the M4 surface and pinned strings
  byte-for-byte, plus one explicit opt-in: `pool_checkout_wait
  (pool, timeout_ms)` suspends until checkout or timeout (timeout
  is a loud `Err`); `Err`-on-exhaustion stays the default
  backpressure signal.

Confirmed underneath (unchanged): native-mode primitives are
zero-cost where possible and integrate with ownership/borrowing
(no implicit sharing without an explicit synchronization type —
payloads MOVE through `Sender`/`Receiver`, `Mutex<T>` guards
BORROW); managed-mode primitives integrate with ARC and the
(also under-design, see MEMORY_MODEL.md §4) UI event-loop model
— nothing here precludes convergence (owner: UI track). Handles
are values passed explicitly (no global channels/registries);
bare values still cannot cross tasks. Not in v1: condition
variables (expressible via channel), RwLock (mutex covers the M5
workload), global registries, atomics beyond executor internals.
The M4 stdlib helper `sleep_builtin(ms: Int) -> Unit` keeps its
name and at-least contract (wrapped by
`stdlib/concurrency/task.nv::sleep`); inside `async fn` it
suspends instead of holding a thread.

## 6. Async Lowering (M5 shape locked by ADR-024; backends: interp first)

- Lowering shape (no new `Instr` — NIR §4 stays frozen): each
  `await` inside an `async fn` splits the block; the merge block
  opens with `phi` nodes for every live value plus a state tag;
  resume dispatches on the tag via `cond_branch`/`switch`;
  suspension returns via `early_return`. NIR §4.1 rule 10
  (phi-completeness) applies to resume edges unchanged — a
  missing incoming on a resume edge traps as `MalformedCfg`.
- `await` is context-sensitive: a suspend point inside `async
  fn`, a bounded blocking-bridge join inside sync `fn`/`task`
  bodies (ADR-024).
- Cancellation is cooperative at suspend points only
  (`cancel(handle)` is library-level, idempotent, loud on unknown
  handles); a cancelled scope still joins every child before
  returning — cancellation composes with join, never detaches.
- Backend order: the interpreter executes first; `run-vm` and
  native `build` keep failing loudly (`unknown-runtime-symbol`)
  on executor-dependent builtins until lowered there, with
  differential suspend/resume/phi coverage (interp-vs-VM) before
  native lowering begins. The M4 refusal above (NIR/VM paths
  refuse `task` calls; the interpreter is the only task-capable
  backend) describes today; it lifts backend-by-backend under
  ADR-024 AC4, never silently.
