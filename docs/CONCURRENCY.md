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

## 4. One Official Runtime (M4: blocking threads)

- M4 uses real OS threads (`std::thread::spawn`), one per task call.
  This is the structured-concurrency floor, not a tuned executor.
- No shared mutable state crosses threads: arguments and return
  values are the only channel. Each worker gets a fresh interpreter
  with empty `db`/JSON/task registries (handles are meaningless
  across threads).
- A non-blocking executor with `async fn` and proper cancellation
  arrives at M5 (see ROADMAP.md).

## 5. Channels & Synchronization — Open

The exact API surface for channels, mutexes/locks, and other
synchronization primitives is **Open**. What's Confirmed is only that:

- Native-mode primitives are zero-cost where possible and integrate
  with ownership/borrowing (no implicit sharing without an explicit
  synchronization type).
- Managed-mode primitives integrate with ARC and the (also
  under-design, see MEMORY_MODEL.md A4) UI event-loop model.
- The current M4 stdlib helper is `sleep_builtin(ms: Int) -> Unit`
  (blocking, wrapped by `stdlib/concurrency/task.nv::sleep`).

## 6. Async Lowering (compiler-internal) — Deferred

At the NIR level, `async` functions would lower to an explicit
state-machine; this is **Deferred** design work for M5. The current
M4 NIR path refuses `task` calls loudly (no lowering attempted); the
VM path (`run-vm`) also refuses them. The interpreter is the only
task-capable backend today.
