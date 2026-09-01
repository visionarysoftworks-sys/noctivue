# CONCURRENCY.md — Concurrency Model

**Status:** Structured-concurrency model Confirmed; API surface Open.

## 1. Structured Concurrency

Every asynchronous task has a bounded, statically visible lifetime tied
to the scope that spawned it. There is no "fire and forget" spawn by
default — a task's parent scope **MUST** either await or explicitly
detach it, and detaching is a deliberate, visible operation rather than
the default.

## 2. Core Keywords

```nv
async fn fetch_data() -> Result<Data, Error>:
    let response = await http.get(url)?
    parse(response)

task worker():
    loop:
        process_next()
```

- `async` marks a function as asynchronous; calling it produces a
  suspended computation.
- `await` suspends the current async context until the awaited
  computation completes.
- `task` introduces a unit of concurrent work managed by the structured
  task hierarchy.

## 3. Task Lifetime & Hierarchy

- A `task` spawned within a scope is a **child** of that scope.
- A scope's exit (normal, early return, or error) **MUST** wait for all
  child tasks to complete or be cancelled before the scope itself
  completes — no child task outlives its parent scope silently.
- Cancellation propagates top-down: cancelling a parent scope cancels
  its children.

## 4. Failure Propagation

An unhandled error in a child task propagates to its parent scope
(consistent with `Result`-based error handling, ERROR_HANDLING.md), which
**SHOULD** surface as a `Result::Err` from the awaited join point rather
than an out-of-band panic, except for genuine invariant violations
(which panic per ERROR_HANDLING.md §4).

## 5. One Official Runtime

Noctivue ships exactly one official async runtime/executor as part of
the standard toolchain, to avoid the ecosystem fragmentation seen in
languages with multiple competing async runtimes (Principle 9). Native
mode and managed mode each have runtime-appropriate scheduling
(RUNTIME.md), but the *language-level* `async`/`await`/`task` API is
shared.

## 6. Channels & Synchronization — Open

The exact API surface for channels, mutexes/locks, and other
synchronization primitives is **Open**. What's Confirmed is only that:

- Native-mode primitives are zero-cost where possible and integrate
  with ownership/borrowing (no implicit sharing without an explicit
  synchronization type).
- Managed-mode primitives integrate with ARC and the (also
  under-design, see MEMORY_MODEL.md §4) UI event-loop model.

## 7. Async Lowering (compiler-internal)

At the NIR level, `async` functions lower to an explicit state-machine
representation (mirroring common industry approaches) rather than
being interpreted directly; this is an implementation detail, not
user-visible surface syntax, and is elaborated in NIR.md §5 once M1
begins — considered **Deferred** design work, not required for M0.
