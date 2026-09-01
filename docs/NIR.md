# NIR.md — Noctivue Intermediate Representation

**Status:** Structural shape Proposed; do not treat as frozen (design
brief §19 explicitly warns against overdesigning NIR before language
semantics stabilize).

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
              ├── block params (SSA block arguments, phi-free style)
              └── instructions (terminated by exactly one control-flow instruction)
```

## 4. Instruction Categories (illustrative, not exhaustive)

```text
Arithmetic / comparison    add, sub, mul, div, icmp, fcmp, ...
Memory (native)            stack_alloc, load, store, move, borrow
Memory (managed)           heap_alloc, arc_retain, arc_release, weak_load
Aggregate                  struct_new, field_get, field_set, enum_tag, enum_payload
Calls                      call, call_indirect, tail_call?
Control flow (terminators) branch, cond_branch, switch, return, unreachable
Error handling             result_ok, result_err, try_unwrap (lowering of `?`)
```

Ownership metadata (native) and reference-count operations (managed)
are represented as explicit instructions rather than implicit
side-effects, so both backends and analysis/optimization passes can
reason about them directly.

## 5. Async Lowering — Deferred

`async`/`await`/`task` lowering to an explicit state-machine
representation in NIR is Deferred until M1 concurrency work begins
(CONCURRENCY.md §7); no instruction set is specified here yet.

## 6. Not Yet Specified

Per the design brief's explicit caution against overdesigning NIR
early, the following are intentionally left unspecified in v0.1:
generics monomorphization strategy vs. generic-template retention in
NIR, exact calling-convention lowering for closures, and the precise
instruction encoding for VM (M1) vs. native (M2) consumption. These are
tracked as **Open** and will be specified once M1 implementation begins.
