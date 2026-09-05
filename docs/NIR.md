# NIR.md — Noctivue Intermediate Representation

**Status:** Instruction set **frozen for M1** (Phase 2 completion
record, IMPLEMENTATION_PLAN.md §4). The inventory in §4 is exhaustive
for the M0 language surface: adding an instruction now requires a
DECISIONS.md amendment naming the HIR construct that cannot be expressed
with the existing set. Structural questions beyond M1 (async lowering,
generics strategy, closure calling convention) remain Open per §6.

## 1. Design Goals

NIR is:

- **SSA-based** — each value assigned exactly once, standard for
  optimizing compilation and directly consumable by Cranelift.
- **Typed** — every value carries its Noctivue type (or a lowered
  representation of it), not erased to untyped bytes this early.
- **Suitable for both a native backend and a VM** — one representation
  feeds Cranelift/LLVM lowering *and* (if a bytecode VM is used for
  faster iteration, per ROADMAP.md M1) a bytecode emitter.
- **Mode-aware without being mode-forked** (see §2, DECISIONS.md
  Issue 6).

Closer in spirit to a simplified SSA/Cranelift-style IR than to a
simple stack-based bytecode.

## 2. Single IR, Mode-Tagged (not two IRs)

Per DECISIONS.md Issue 6, NIR uses **one** instruction/type system with
a mode tag (`native` / `managed`) attached per function and, where
relevant, per value. Mode only affects **lowering rules** (e.g., how a
struct allocation instruction becomes machine code — stack allocation
and move semantics for native, ARC increment/decrement insertion for
managed) — it does not fork the instruction set itself. This keeps the
"one language" principle intact one layer down the stack.

## 3. Structural Components

```text
Module
  ├── imports / external declarations
  ├── type definitions (structs, enums, generics-monomorphized or generic templates)
  └── functions
        ├── signature (params, return type, mode tag)
        └── basic blocks
              ├── block params (populated on entry blocks; reserved, VM ignores them)
              ├── Phi nodes (merges use explicit Phi — see §4 control-flow discipline)
              └── instructions (see §4 for the terminated-block rules)
```

## 4. Instruction Set (frozen for M1)

The complete inventory, grouped by category. Names are the canonical
textual form (see `Instr`'s `Display` impl, the single source of truth
alongside `compiler/src/nir/instr.rs`).

```text
Arithmetic / comparison    add, sub, mul, div, rem,
                           icmp, fcmp, neg, not,
                           const
Memory (native)            stack_alloc, load, store, move
Memory (managed)           heap_alloc, arc_retain, arc_release, weak_load
Aggregate                  struct_new, field_get, field_set,
                           list_len, list_index,
                           enum_tag, enum_payload (indexed), enum_new
Calls                      call, call_indirect,
                           closure_new, closure_call
I/O                        print
Control flow (terminators) branch, cond_branch, switch, return,
                           early_return, unreachable
Error handling             result_ok, result_err, try_unwrap,
                           option_some, option_none
Conversion                 to_string
SSA merge                  phi
```

Notes and deliberate deviations from the v0.1 sketch:

- There is **no `borrow` instruction**: borrow checking is a Phase 3
  (M2) static analysis over this IR, not an instruction. There is no
  `tail_call` either (no user-facing tail-call construct in M0).
- `switch` exists but match lowering currently uses test chains
  (`cond_branch` + tag/ICmp tests); `switch` is reserved for a future
  jump-table optimization and the Cranelift backend.
- Merges use explicit **`phi`** nodes (not phi-free block params):
  block `params` are populated on entry blocks and otherwise reserved.
- `enum_payload` takes a field **index** (multi-payload variants).
- `early_return` terminates `?`-propagation paths; it behaves like
  `return` in the VM but marks propagation sites for later backends.
- `print` exists because M0's `print`/`println` builtins lower directly
  (no stdlib to call into yet); the remaining builtins
  (`panic`/`assert`/`to_int`/…) are **not** representable yet — see §6.

### 4.1 Control-flow discipline (load-bearing invariants)

Every lowering path must uphold these; the differential suite
(`noct-cli/tests/differential.rs`) exists to catch violations as
*wrong output*, since most of them execute without errors:

1. **Every reachable block is terminated.** The VM errors only on
   blocks it actually visits, so an unterminated dead block is latent,
   not safe — terminate dead blocks anyway (`unreachable`).
2. **After a split, later code emits into the merge/continue block,
   never into the terminated pre-split block.** (The `split_current_block`
   helper swaps the merge block into the emission cursor.) Appending
   past a terminator silently reorders code *before* the branch.
3. **`?` splits before unwrapping**: `cond_branch` on the Option/Result
   value itself (Some/Ok are truthy), `early_return` of a reconstructed
   `None`/`Err(original payload)` on the else edge, infallible
   `try_unwrap` on the continue edge. `try_unwrap` on None/Err is a VM
   trap (`OptionUnwrapNone`/`ResultUnwrapErr`), never a silent Unit.
4. **`??` short-circuits**: the right side is evaluated solely in the
   else block; the merge `phi` takes the *unwrapped* payload on the
   taken edge, never the Option wrapper.
5. **Match is a test chain**, not positional-`switch` dispatch: arms in
   order, wildcard/identifier arms unconditional, literal arms via
   `icmp Eq`, variant arms via `enum_tag` compare (prelude `Some`/`Ok`
   via truthiness, `None`/`Err` via inverted truthiness), guards as
   in-arm branches to the next test, bindings via indexed payload
   extraction, no-match falls through to an `unreachable` trap. Arm
   bodies are lowered exactly once (never re-lowered for the Phi value).
6. **Loops thread state through `phi`**: the `for` index is a
   header-Phi (init on entry, incremented value on back-edge). Loop
   exits converge on a merge block; `for` exit is a branch, never a
   `return` (the old `Return None` discarded accumulators).
7. **Values thread like the interpreter's `eval_body`**: every statement
   updates the running value (`Unit` for non-value statements); an
   explicit `return` ends the list. Previously stale values leaked
   through trailing `let`s.
8. **Structural equality/comparison** (`icmp`) mirrors the
   interpreter's `values_equal`/`value_cmp` (including `String`/`Char`
   and mixed numerics); incomparable pairs are runtime errors, never
   quiet `false`.
9. **No silent recovery inside the VM**: unknown callees are
   `FuncId::UNRESOLVED` and fail lookup loudly (the old `FuncId(0)`
   fallback called an arbitrary function); list literals build `List`
   values (the old always-`Struct` made every `for` over a literal run
   zero iterations).

### 4.1 Control-flow discipline (load-bearing invariants) — Amendment (2026-09-04)

Item 1 ("every reachable block is terminated... terminate dead blocks
anyway") is extended to cover Phi completeness explicitly, since the
VM's original Phi handler defaulted to `Unit` on an incomplete
`incoming` list rather than trapping — the same failure signature this
section already warns about ("most of them execute without errors"),
just not yet listed as its own numbered rule:

10. **Every Phi's `incoming` list must cover every actual predecessor
    that can reach it.** A Phi reached via a predecessor block with no
    corresponding `incoming` entry — or reached with no predecessor
    recorded at all — is a lowering bug. The VM traps
    (`VmError::MalformedCfg`) rather than defaulting to `Unit`, matching
    `try_unwrap`'s existing "no silent recovery" discipline (item 3).
    Any future control-flow construct that adds a new edge into an
    existing merge block MUST add the corresponding `incoming` entry at
    the same time — this is the concrete failure mode `break`/`continue`
    lowering (NIR.md §6) needs to watch for once implemented, since it
    adds new edges into existing loop-exit merge blocks.

Ownership metadata (native) and reference-count operations (managed)
are represented as explicit instructions rather than implicit
side-effects, so both backends and analysis/optimization passes can
reason about them directly.

## 5. Async Lowering — Validation Outcome (Phase 2 record)

CONCURRENCY.md §7's state-machine hypothesis was validated against the
real M1 NIR as Phase 2 requires — and the honest outcome is that §7 is
too thin to validate *against*: it specifies no suspend-point shape, no
resume dispatch, and no state representation beyond "explicit
state-machine, implementation detail". What Phase 2 *does* establish:

- No new instructions look necessary: suspend points are block splits,
  resume is `cond_branch`/`switch` dispatch on a state tag, carried
  state is `phi` nodes, and suspension returns are `early_return` —
  all exercised daily by `?`/`??`/match lowering now.
- No design work beyond that starts here: the executor, scheduler, and
  `task` semantics belong to M4/M5 (IMPLEMENTATION_PLAN.md Phases 5–6),
  and CONCURRENCY.md §7 stays Deferred until then. If that design ever
  needs an instruction the §4 set cannot express, it goes through the
  amendment process in the §4 header.

## 6. Not Yet Specified

- Generics monomorphization strategy vs. generic-template retention in
  NIR: still Open.
- Exact calling-convention lowering for closures: still Open
  (`closure_new`/`closure_call` carry captures opaquely for now).
- Builtin coverage beyond `print`: `panic`/`assert`/`to_int`/
  `to_float`/`to_string` have no NIR representation yet — valid
  programs using them fail only at lowering/VM time, and only with a
  loud `UNRESOLVED` error, never silently. Owned by a future milestone
  with the stdlib surface (M3+).
- Nested-variant and literal *sub*patterns in match arms (e.g.
  `Circle(0)`): valid code the lowerer rejects loudly via
  `unimplemented!` rather than miscompiling. Owned by whoever extends
  match lowering next.
- Unguarded-`Bool` match guards are not verified by typeck (the
  interpreter panics, the VM truthiness-branches): a frontend gap,
  recorded here so it isn't rediscovered.
- `break`/`continue` are now fully lowered (HIR nodes +
  `loop_exit`/`loop_cont` lowering cursors + interpreter unwind signals;
  `E0205` outside loops, warning on discarded `break` values). Remaining
  limits, still Open: no *labeled* break/continue (multi-level loops exit
  one level only — check LANGUAGE_SPEC.md before assuming that is
  sufficient), and `break` values are evaluated-then-discarded (loops
  have no value channel).
- Lists are fixed-size: literals, `l[i]`, `.len`, `.first`/`.last`,
  `for` — no push, concat, or construction beyond literals, so
  `map`/`filter`/`reverse` cannot be written in user code today (the
  `.map`/`.filter`/`.sort` members are acknowledged placeholders, not
  implementations). Growth needs the ownership story (borrowck, Phase 3
  Step 3) first, then M3 collections. Owned by whoever does either.
- Strings are opaque past `len`/`==`/concat/interpolation: no indexing,
  slicing, iteration, or chars, so `contains`/`split`/`trim` cannot be
  written in user code today. Needs a String method-call story (M3
  string helpers, FFI.md §7), not just new builtins. Owned by whoever
  takes M3 strings.
