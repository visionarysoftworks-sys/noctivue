**Status:** Draft proposal for the M5 executor ADR — not normative until merged into docs/DECISIONS.md.

# M5 Non-Blocking Executor — Design Draft

Scope: the M5 executor that replaces the M4 blocking-threads floor
(`docs/CONCURRENCY.md` sections 1-6) with non-blocking sockets,
`async fn`, cancellation, and non-blocking timers, while preserving
structured concurrency, task-local handle rules, and the one-official-
runtime rule (ADR-009). Normative parents are cited per claim; every
divergence from them is marked DIVERGENCE with rationale.

Conventions used below: Confirmed means a cited doc already locks the
point; Proposal means this draft asks the future ADR to lock it.

## 1. Requirements

R1. Non-blocking sockets (Proposal, builds on PHASE5 section 2).
The accept loop MUST never block on a handler. Listeners and
connections run non-blocking; readiness is polled by the executor, not
by holding one OS thread per connection. The M4 wire contract is
unchanged: HTTP/1.1 close-delimited, 8 KiB header cap, 1 MiB body cap,
413/400/404/500 mapping, bounded incremental reader. No wire-visible
change ships with the executor.

R2. `async fn` lowering shape (Proposal, constrained by NIR section 5).
`async fn` lowers with the already-validated shape and no new
instruction (NIR section 4 stays frozen; any new `Instr` needs a
DECISIONS.md amendment naming the inexpressible HIR construct):
suspend points are block splits, resume is `cond_branch`/`switch`
dispatch on a state tag, carried state is `phi` nodes, suspension
returns are `early_return`. Concretely, each `await` inside an
`async fn` splits the block; the merge block opens with `phi` nodes
for every live value plus a state tag; resume jumps through the
dispatch prologue. `task` keeps its M4 surface (spawn returns an
opaque `Int` handle, `await h` yields the body value, single-use
handles, loud failure on unknown/double await) but its implementation
moves onto the executor (see section 2, M4-floor fate: migrated).
`await` is context-sensitive: inside `async fn` it is a suspend point
(non-blocking); inside a synchronous `fn` or `task` body it blocks the
current worker via the blocking bridge (section 2, option A) so
existing call shapes keep working.

R3. Cancellation semantics (Proposal, composes with ADR-009).
Cancellation is cooperative and delivered only at suspend points
(`await` in `async fn`, channel send/recv on empty/full, timer wait,
blocking-bridge join). There is no preemptive kill and no ad-hoc kill
flag (PHASE5 section 3 forbids the latter for M4; this draft extends
the ban: implementers MUST NOT invent out-of-band thread kills).
New surface is library-level, not keywords: `cancel(handle) -> Unit`
(requests cancellation; idempotent; unknown handle is a loud error)
plus scope-cancel on task groups once their syntax lands. A cancelled
scope STILL JOINS: scope exit (normal, early, error, or cancelled)
joins every outstanding child before returning, exactly as
`Interpreter::drain_tasks` does today. Cancellation therefore composes
with structured concurrency rather than punching a hole in it: the
cancelled child reports `Err("cancelled")`-class loud values (exact
strings pinned by tests), the awaiter observes the cancellation as a
value composable with `?`, un-awaited cancellations are reported at
drain time, and no child outlives its parent scope. Rationale: ADR-009
is Confirmed (no fire-and-forget); a cancel that detaches would be
fire-and-forget under another name. Argued FOR join-on-cancel.

R4. Non-blocking timers on ADR-020 (Proposal).
Timers build on the monotonic millisecond source (`time_mono_ms_builtin`
/ `stdlib/time/instant.nv`), never on wall clock. `sleep(ms)` keeps its
at-least, never-exact contract (PHASE5 section 3) but stops holding an
OS thread: inside `async fn` it suspends the task and resumes no sooner
than `ms` later via a timer heap owned by the executor. Negative inputs
still panic loudly. Exact or upper-bound timing assertions stay
forbidden in tests; only `elapsed >= ms` with generous upper bounds.
No calendar, timezone, NTP, or deadline-object builtin ships here;
deadlines/timeouts are library API over timers (IMPLEMENTATION_PLAN
Phase 6 item 3).

R5. Multiplexed pools (Proposal, preserves PHASE5 section 1).
The `pool_open`/`pool_checkout`/`pool_checkin`/`pool_close`/`pool_run`
surface and its exact error strings (`pool max_size must be at least 1`,
`pool exhausted (max_size reached)`) are PRESERVED byte-for-byte. What
changes is the implementation behind it: from the M4 single-idle-slot
value-threaded pool to a multiplexed async pool with a bounded checkout
queue, idle-eviction policy hooks (none in M4 per PHASE5 section 1;
policy itself is M5 work), and `pool_run` as a suspending checkout
instead of a blocking one. Backpressure on exhaustion stays an
`Err`, never an implicit wait, unless the caller explicitly opts into
a waiting checkout variant in library code.

R6. Task-local handle rules PRESERVED (Confirmed, hold the line).
`Pool`/`Db` values MUST NOT cross a task boundary as bare values.
Handles stay per-worker registry ids; each worker starts with fresh,
empty `db`/JSON/task registries (`interp/src/lib.rs::for_task`
precedent). The reference pattern (open and use the pool on the thread
that serves) is unchanged. Passing a `Db`/`PoolCheckout` into a `task`
call stays a loud runtime error (`unknown database handle ...`),
never cross-thread use. Sharing a pool ACROSS tasks requires an
explicit sync type (section 3); bare-value sharing stays rejected.

R7. Structured concurrency + no-fire-and-forget PRESERVED (Confirmed,
ADR-009). Every task has a bounded lifetime tied to its spawning scope;
the parent scope joins (or the runtime joins at scope exit). Detaching
stays absent from the surface. `await` on a failed task still yields
the failure VALUE (`EarlyReturn`/`UncaughtError` map at the boundary);
panics still fail the awaiter loudly (`task {id} panicked`-class) and
un-awaited failures still fail the run at drain (`background task
{id} failed`, exit 1). Cancellation (R3) adds a new outcome, not a new
escape from joining.

R8. One official runtime, no competing executors (Confirmed, ADR-009).
This draft proposes exactly ONE executor. Research spikes may prototype
alternatives off-tree, but the tree ships one scheduler, one timer
heap, one readiness loop, one cancellation protocol. `task`, `async fn`,
channels, mutexes, timers, and the HTTP serve loop all run on it.
A second executor model (per-workload runtimes, user-selectable
schedulers, vendored external runtimes as alternatives) is rejected in
section 2.

R9. Dependency-free stdlib ethos (Confirmed posture, holds).
No new runtime dependency ships with the executor without a
packaging/audit justification equal to the ADR-023 bar (static-link
story, license, `unsafe` boundary review, tier label in `noct audit`).
The reference design in section 2 uses only `std` (`TcpListener` /
`TcpStream` non-blocking mode already in the tree, `std::thread`,
`Mutex`/`Condvar`/channels, timer heap in plain Rust). In particular
no Tokio/mio/async-std vendoring is proposed. If the Wave 1 spike
reopens this (throughput shortfall under section 5 load, proven by
measurement), it reopens via ADR amendment with the measured rationale,
per ADR-023's lock rule — not by silent `Cargo.toml` growth.

R10. Loud errors, never silent (Confirmed, PHASE5 global discipline).
Every new failure names its cause as a `Result::Err` or a coded
diagnostic: unknown/double await, unknown cancel handle, closed-channel
send/recv, exhausted pool, oversized payload, timer misuse. Default
values, dropped requests, and swallowed closes stay forbidden.
`run-vm` and native `build` MUST keep failing loudly
(`unknown-runtime-symbol`) on executor-dependent builtins they do not
yet lower; silent VM/native divergence stays release-blocking.

## 2. Design options

### Option A (RECOMMENDED): fixed work-stealing pool + readiness event loop, std-only

Shape: N worker threads (default: available parallelism, capped, see
open questions) run a shared work-stealing task queue; ONE readiness
thread owns the non-blocking listener set, the connection readiness
set, and the timer heap (mono-ms, ADR-020). Workers never block on I/O:
a task that would block (socket read/write would-block, timer wait,
channel empty/full) suspends its state machine and yields the worker;
readiness or timer expiry re-queues it. A small bounded blocking bridge
(`spawn_blocking`, fixed cap, queue-full is a loud `Err`, never silent
growth) runs the few operations that cannot suspend: legacy blocking
sections, SQLite calls through rusqlite (bundled, ADR-019 — no new
dependency), and sync-`await` joins from non-`async` contexts.

Suspend/resume lowering: per R2. Each `await` in `async fn` is a block
split with a state tag; live values become `phi` incomings on the merge
block (NIR section 4.1 rules 1-10 apply unchanged, including rule 10
phi-completeness — a missing incoming on a resume edge traps as
`MalformedCfg`, never defaults to `Unit`). Suspension returns via
`early_return` carrying (state tag, saved state). Resume is a dispatch
prologue (`cond_branch`/`switch` on the tag) at function entry. The
interpreter executes the same shape it validates today for `?`/`??`/
match; the VM and Cranelift lowerings reuse it later. No new `Instr`.

Cancellation delivery: at suspend points only. Each task carries a
cancel flag checked on every resume and on every suspend attempt. `cancel
(handle)` sets the flag (idempotent) and re-queues the task if parked;
the next suspend/resume boundary unwinds it to `Err("task {id}
cancelled")`-class value (exact string pinned in tests). Scope-cancel
sets every child flag, then JOINS every child (R3/R7) before the scope
returns. Cancel never aborts Rust code mid-statement and never kills a
thread.

M4 blocking floor fate under A: MIGRATED (this draft's pick).
`task`/`await` source semantics are preserved; the thread-per-spawn
implementation is retired and `task` becomes an executor task. The M4
floor is not kept as a parallel default and not left behind a feature
flag as a competing runtime (that would violate R8). What remains of
blocking: (a) `sleep_builtin(ms)` keeps its name and at-least contract
but inside `async fn` suspends instead of holding a thread; called from
a sync context it uses the blocking bridge and still blocks that
worker only; (b) one OS thread per connection disappears — connections
become executor tasks, which is the point. Rationale: one official
runtime (ADR-009) cannot mean two schedulers selected by flag; the
M4-per-connection-thread model is already documented as OS-bounded, not
runtime-bounded (PHASE5 section 3), so keeping it as default preserves
the exact unboundedness M5 exists to remove; migration keeps every
existing program compiling and passing (section 4) while moving the
tuning burden (caps, backpressure, eviction) to the executor where
Phase 6 resilience work expects it (IMPLEMENTATION_PLAN Phase 6 item 3).

### Option B: thread-per-core isolated event loops + cross-thread channels, no stealing

Shape: one single-threaded event loop per core (N loops, each with its
own task queue, timer heap, and readiness set); tasks are pinned to the
spawning loop; cross-loop work moves only through explicit bounded
channels. No work-stealing, no shared queue, no shared timer heap. I/O
readiness is still poll-based std-only.

Suspend/resume lowering: identical NIR shape to option A (block splits,
tag dispatch, `phi`-carried state, `early_return` suspension) — the
lowering is executor-agnostic by design (NIR section 5). Only the
re-queue target differs (own loop, never stolen).

Cancellation delivery: same cooperative-at-suspend-points protocol, but
scope-cancel of children pinned to other loops needs a cross-loop
message round-trip before the join can complete, so cancelled-scope
drain latency is the sum of loop wake latencies rather than one queue
wake.

M4 blocking floor fate under B: FEATURE-GATED dual runtime during
migration (`--executor={threads,loop}`), then migrate. Rejected: the
gate period is two schedulers in the tree (violates R8 even
temporarily), doubles the load-harness matrix, and pinning reintroduces
a placement question (which loop runs the accept loop? which loop runs
a handler's pool?) that option A's shared queue never asks. B's only
advantage (no shared-queue contention, simpler reasoning about cache
locality) does not pay for a second scheduler on the M5 workload
(16x128 mixed CRUD, SQLite-serialized writes — contention lives in the
DB, not the queue).

### Option C (considered, REJECTED): vendor an external async runtime crate

Shape: depend on an established Rust async runtime for the pool,
readiness, and timers; lower `async fn` to its future model instead of
the NIR state-machine shape.

Suspend/resume lowering: foreign — the NIR block-split resume design
(NIR section 5, validated) is bypassed in favor of the crate's poll
model, stranding the differential suite's lowering-discipline coverage.
Cancellation delivery: the crate's token model, not the scope-join
model — join-on-cancel would need re-proving against foreign semantics.
M4 floor fate: migrated, but onto a dependency. Rejected under R9: no
packaging/audit justification has been made (static-link story on
Windows, license review, `unsafe` boundary audit, `noct audit` tier
label), and the dependency-free ethos plus the ADR-023 lock rule
require measurement before vendoring. Reopen only with section-5-style
load numbers showing the std-only design cannot meet a stated,
non-thresholded goal — and then as an ADR amendment, not a quiet
`Cargo.toml` edit.

### Recommendation

Option A. It is the only option that satisfies R8 without a dual-
runtime interregnum, reuses the validated NIR lowering unchanged,
delivers cancellation at the same suspend points the scheduler already
owns, retires the OS-bounded thread-per-connection model that M5 exists
to replace, and ships with zero new dependencies under R9.

## 3. Sync primitives surface

Status guardrail (Confirmed, CONCURRENCY section 5): native-mode
primitives are zero-cost where possible and integrate with
ownership/borrowing; there is NO implicit sharing without an explicit
synchronization type. Managed-mode primitives integrate with ARC and
the UI event-loop model (still under design; this draft only requires
that nothing here precludes it). `channel`/`mutex`/`sync`/`atomic`
stdlib files are reserved today (PHASE5 section 3); this section proposes
the promotion shape — library API over the executor, never new keywords.

Proposed surface (all library functions over executor handles, all
loud on misuse):

- `oneshot() -> (Sender, Receiver)`: single-value rendezvous for
request/reply. Send moves the value; recv suspends until it arrives or
the sender drops (dropped-sender recv is `Err`, never `Unit`).
- `channel(cap: Int) -> (Sender, Receiver)`: bounded MPMC queue, async
send/recv. Send on full suspends (backpressure, never silent drop);
recv on empty suspends; send/recv on closed is a loud `Err`; `cap < 1`
is a loud `Err` at construction. Unbounded channels are a NON-GOAL
(section 6): every queue has a cap so backpressure is explicit.
- `mutex<T>(v: T) -> Mutex<T>` + scoped guards: `lock(m) -> Guard`
suspends (never blocks a worker thread) until acquired; the guard
borrows (native: borrow-checked, cannot outlive the lock scope; the
guard cannot be sent across tasks); unlock is scope exit; forgetting a
guard to extend the critical section is a loud error, not silent
extension. `try_lock` returns `None` immediately instead of suspending.
No reentrant locking (second `lock` of a held mutex in the same task is
a loud `Err`, never a deadlock).
- `select` over receivers with fair polling is deferred to library
design (open question); `sleep_async` (non-blocking sleep, R4),
`timeout(ms, handle)`, and `hedge` live one layer up as blessed
packages per IMPLEMENTATION_PLAN Phase 6 item 3, composed from
oneshot + channel + timers — not language features, not keywords.

Ownership integration (native): channel payloads MOVE across tasks
(single owner at a time, mirroring today's args-and-return-values-only
rule generalized to an explicit type); `Mutex<T>` guards BORROW for the
critical section and cannot escape it. This keeps "no implicit sharing
without an explicit sync type" literal: bare values still cannot cross
tasks (R6 holds); values inside `Sender`/`Receiver`/`Mutex` can, with
the sync type witnessing the sharing. Managed mode: the same handles
integrate with ARC retain/release at the boundary (RFC in the UI track;
no semantics invented here). Cross native/managed sharing follows the
disjoint-graphs rule (DECISIONS Issue 3, MEMORY_MODEL section 7):
explicit conversion only, never implicit.

What is NOT proposed: condition variables (expressible via channel),
reader-writer locks in v1 (deferred; mutex covers the M5 workload),
global channels/registries (no global mutable state — handles are
values passed explicitly, as pools are today), atomics beyond what the
executor needs internally (the `atomic.nv` file stays reserved).

## 4. Migration and compatibility

Existing `task`/`await` programs: SOURCE-COMPATIBLE, behavior-
compatible at the value level. Every M4 program compiles unchanged and
computes the same values: spawn still returns an `Int` handle, `await`
still yields the body value, single-use/unknown-handle loud errors
unchanged, drain-at-scope-exit unchanged. What changes is performance
shape, not semantics: tasks multiplex over the pool instead of holding
OS threads. Programs that depend on thread identity (none exist in the
tree by construction — workers are anonymous and handles are opaque)
or on thread-count side channels (see harness below) MUST be updated;
value-level goldens do not change.

Load harness (`tests/http_load.rs`): EXTENDED, not replaced. The
16x128 two-cycle file-backed CRUD run (pool accounting, `integrity_check
= ok`, spot reads, serial gates, latency RECORDED-not-thresholded)
stays the M4-correctness floor and MUST keep passing unchanged except
for the two assertions that name the old implementation: the
`live_connection_threads` baseline/peak gauges (thread-per-connection
counts) and any wall-time overlap assertion calibrated to 400 ms
blocking sleeps. Those are re-baselined to executor gauges
(live tasks, parked tasks, blocking-bridge depth, peak multiplexed
connections) with the same intent (prove overlap structurally, prove
zero deltas across two cycles). New legs are ADDED alongside (section 5),
never substituted for the existing legs: a maintainer must be able to
run the old legs and see the old guarantees.

Worker-private registries (`for_task`): PRESERVED in phase 1 of the
migration. Each executor worker starts with fresh empty `db`/JSON/task
registries; `Db`/`PoolCheckout` handles stay meaningless across tasks
and the `unknown database handle` loud error stays. Relaxation arrives
only through section 3 types: a pool shared across tasks is a
`Mutex<Pool>` or a channel-passed handle, never a bare `Db` that
teleports. Tables/clones that are safe to share (function/struct/enum
tables, immutable globals) keep the `for_task` clone precedent; no new
shared-mutable state is introduced outside the executor's own
(audited, internal) queue/heap/registry.

Explicitly breaking (the complete list — nothing else breaks):
(a) thread-count observability: `live_connection_threads` peaks and any
test asserting `peak >= 2 OS threads` are re-baselined to task gauges;
(b) timing shape: `sleep` no longer holds an OS thread, so overlap
tests calibrated to blocking durations are recalibrated (same
structure, new constants); (c) pool internals: single-idle-slot
accounting (`idle: -1` sentinel, `Ok(1)/Ok(0)` close reporting) may gain
queue-depth fields behind the preserved surface and strings — tests
pinning the STRINGS keep passing; tests pinning the STRUCT SHAPE are
updated; (d) serve-loop internals: `http_server_serve_loop` moves from
inline-sequential/blocking to readiness-driven dispatch — its stdlib
route-table surface (`route`, `serve`, `stop` with drain) is unchanged.
Nothing else: no syntax change, no manifest change, no lockfile change,
no registry change, no error-string change except the one new
`cancelled`-class string family.

## 5. Acceptance criteria

AC1. Extended load harness (normative, sibling to `tests/http_load.rs`).
Keep the full M4 run (16 clients x 128 requests, file-backed temp DB,
404/400 trickles, poolcheck `1`, integrity `ok`, row-count and
membership spot reads, two consecutive cycles with zero deltas) and add:
(a) a timeout leg — handlers with injected latency past a library
`timeout(ms, ...)` budget return the timeout `Err` loudly and the
server keeps serving (no hung request past the 30 s watchdog);
(b) a hedge leg — a `hedge` blessed-package helper racing two
equivalent reads returns the first success and cancels the loser, with
the loser proven joined (no leaked task gauge delta); (c) latency
still RECORDED-not-thresholded (p50/p99 per endpoint to stdout, sizes
and budgets from PHASE5 section 5 unchanged). Zero crashes, zero
deadlocks (every request terminal within the watchdog), zero leaks by
the PHASE5 triple (pool accounting delta zero, task/connection gauges
to baseline, second cycle no growth).

AC2. Cancellation tests (normative). Unit + integration: cancel a
sleeping task (observes `cancelled`-class value at the awaiter,
composable with `?`); cancel a channel-parked task; scope-cancel N
children and assert ALL joined before scope exit (gauge deltas zero)
with each reporting cancelled; cancel-after-completion is a no-op success
(idempotent); cancel of unknown/double handle is a loud error;
un-awaited cancelled tasks are reported at drain with exit 1. At least
one test pins that a cancelled scope's SIBLING values are unaffected
(cancellation does not poison the scope).

AC3. No-regression list (normative, hold-the-line). Drain semantics
(no fire-and-forget: program exit joins everything; drop-on-stop
rejected for `http_server_stop`: stop accepting immediately, drain
accepted connections to response-completion, return only after join);
pool accounting (both pinned strings byte-for-byte; `pool_close`
returns `Ok(1)/Ok(0)`; checked-out handles stay the borrower's
responsibility; `pool_close` never invalidates checked-out handles);
loud-failure discipline (double/unknown await, unknown cancel, closed-
channel use, exhausted pool, oversized header/body, malformed bytes —
each a named `Err`/diagnostic, none a default or a drop); at-least
sleep (`elapsed >= ms`, negative panics); binding-only SQL (self-grep
test from the current harness, unchanged); file-backed DB with
`integrity_check` plus `:memory:` smoke only as supplement.

AC4. Backend parity discipline. Executor builtins fail loudly
(`unknown-runtime-symbol`) on `run-vm`/`build` until lowered there;
no silent VM/native divergence. The NIR lowering for `async fn`
carries differential coverage mirroring the Phase 2 pattern
(interp-vs-VM identical observable behavior on suspend/resume/phi
programs) before native lowering begins.

AC5. Zero-new-dependency audit. `cargo tree`/manifest review shows no
new runtime crate for the executor; any exception carries the ADR-023-
grade justification (static-link probe, license, `unsafe` review, audit
tier) as an amendment, not as code first.

## 6. Non-goals and open questions

Non-goals (explicitly OUT for the M5 executor ADR; implementers MUST
NOT build them, reviewers MUST reject them as scope creep):

- Work-stealing tuning as a guarantee: queue sizes, steal half/full
policy, worker count formula, and latency thresholds are engineering
choices, recorded-not-thresholded like M4 latency. No p50/p99 gate.
- TLS/`https`, auth, rate limiting/backpressure caps beyond explicit
bounded queues, HTTP/2, status-aware client codes, ORM, metrics/tracing
export shape, `noct audit` enforcement/sandboxing, replication/failover,
client-server DB driver pick: all Phase 6 items owned elsewhere
(PHASE5 section 6, ADR-023, IMPLEMENTATION_PLAN Phase 6). Timeouts and
hedging ship as blessed LIBRARY packages (Phase 6 item 3), not as
keywords or executor flags.
- Preemptive cancellation, thread kills, deadlines as builtins, priority
scheduling, task pinning/affinity API, unbounded channels, reentrant
mutexes, RwLock v1, `select` v1 beyond the open question below.
- Wall-clock dates, timezones, calendars, NTP, sleep precision beyond
at-least (ADR-020 non-goals hold).
- VM/native lowering of the full executor in v1: interp first with loud
failures elsewhere (AC4); Cranelift/`runtime-native` scheduler work
follows as its own plan.
- Cross native/managed value sharing beyond explicit conversion
(DECISIONS Issue 3 holds); managed event-loop integration beyond
not-precluded.
- Multi-statement transactions and idle-pool eviction POLICY (arrive
with the Postgres-class driver / Phase 6 design as one unit, per
PHASE5 section 4 and section 1 notes).

Open questions (could not be decided from the docs alone — no silent
picks taken; the ADR must resolve or explicitly defer each):

1. Worker-count default and cap: `available_parallelism` with what cap
and what override (env var? manifest key?)? How does the blocking
bridge cap interact with SQLite's serialized writes under the 16x128
mix — bound the bridge below the pool max, or let pool exhaustion
`Err` be the backpressure signal?
2. `await` in sync contexts: blocking-bridge join (this draft's R2) vs.
loud error directing the user to `async fn`? The draft picks the
bridge for source compatibility; the ADR should confirm the bridge
cannot mask handler-lifetime bugs the M4 exit criteria exist to catch.
3. Timer granularity and heap policy: tickless heap vs. fixed tick; does
`sleep(0)` yield or return immediately? At-least semantics admits both.
4. Channel `select`/fairness: is a `select` helper required in v1, or do
timeout/hedge packages compose it adequately from oneshot + timers?
5. Pool surface growth: does the multiplexed pool need an explicit
waiting-checkout variant (`pool_checkout_wait`) alongside the
`Err`-on-exhaustion default, or is the queue internal to `pool_run`?
Byte-for-byte string preservation constrains but does not settle this.
6. Handle exhaustion and id reuse: are task/channel/mutex ids ever
reused within a run, and what is the loud error when a stale id from a
drained scope is awaited/cancelled?
7. Native-backend scheduler: does `runtime-native` get its own executor
port (same protocol, native codegen) or does native link the same Rust
executor — and which does the WASM backend (M6) assume?
8. Observability hooks: does the executor expose task/queue/timer gauges
for the OTel blessed package now (names frozen), or are gauges
test-only until Phase 6 observability lands?
9. Managed/UI event loop: does the single readiness thread converge with
the future UI event loop (RUNTIME/MEMORY_MODEL open items), or do UI
and server executors stay distinct under the one-runtime rule — and who
decides?
10. RNG-in-executor: does jitter for timeouts/hedging consume ADR-021
explicit-seed handles (deterministic replay preserved) rather than any
ambient source — and how do tests pin replay across interleavings?
