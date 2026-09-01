# FFI.md — Foreign Function Interface

**Status:** C ABI priority Confirmed; Rust interop Deferred.

## 1. Priority Order

1. **C ABI** — first-class, needed for the vast majority of systems/
   platform interop on day one.
2. **Rust interoperability** — potential direct interop with Rust crates
   beyond the C ABI boundary; **Deferred**, revisit once native-mode
   codegen is stable (post-M2).

## 2. `unsafe` Requirement

All FFI declarations and calls **MUST** be wrapped in `unsafe`:

```nv
unsafe:
    extern "C" fn strlen(s: *const Char) -> UInt

    let len = strlen(my_c_string)
```

`unsafe` marks the boundary where the compiler's safety guarantees
(ownership/borrow checking, bounds checking) stop being enforced and the
programmer takes responsibility for upholding them.

## 3. C Type Mapping

```text
Noctivue        C
Int8/16/32/64   int8_t/16_t/32_t/64_t
UInt8/16/32/64  uint8_t/16_t/32_t/64_t
Float32/64      float / double
Bool            (platform-defined; not guaranteed same layout as C _Bool)
*const T        const T*
*mut T          T*
```

## 4. Pointers

`*const T` and `*mut T` are raw, unowned pointers usable only within
`unsafe` blocks. They carry no ownership/lifetime tracking — crossing
into `*const`/`*mut` is exactly the point at which Noctivue's ownership
model hands off responsibility to the programmer/C convention.

## 5. Calling Conventions

`extern "C"` is the default and only calling convention specified for
v0.1. Additional conventions (`"stdcall"`, etc., for platform-specific
needs) are **Deferred** until concrete demand appears.

## 6. Ownership Boundaries

When a native-mode owned value crosses the FFI boundary (e.g., passed
by pointer to a C function that takes ownership), the transfer **MUST**
be made explicit via a boundary function that consumes the Noctivue
value and returns a raw pointer — never an implicit, silent handoff.
Symmetric "reclaim ownership from a raw pointer" functions are required
for the return path. Exact stdlib API names for these boundary
functions are Deferred to stdlib design.

## 7. Callbacks, Structs, Strings

- **Callbacks:** a Noctivue `fn` with a C-ABI-compatible signature may be
  passed as a C function pointer inside `unsafe`; closures that capture
  environment are **not** directly C-ABI compatible without an explicit
  trampoline (mechanism Deferred).
- **Structs:** `#[repr(C)]`-equivalent layout control for structs
  crossing the FFI boundary is required but the exact annotation syntax
  is **Open**.
- **Strings:** Noctivue `String` (UTF-8, not necessarily null-terminated)
  is not directly a C string; boundary conversion functions
  (`String` ↔ `*const Char` null-terminated) are part of the stdlib FFI
  module, not the core language.

## 8. Platform Libraries

Linking against system/platform libraries is a `noct` package-manifest
concern (TOOLCHAIN.md), not a language-level construct.

## 9. C++ Interoperability — Gated (M6, ADR-014)

C++ has no stable, portable ABI the way C does, so "C++ interop" is a
different problem from §1–8, not a variation on it — there is no single
type-mapping table to write. **Status: Experimental, gated**, in the
spirit of the UI research track (ROADMAP.md §2): a research spike must
validate an approach before it's Confirmed.

**Leading candidate:** generate a thin, stable C-ABI shim per C++
entity used (functions, and a bounded subset of class shapes — no
templates, no exceptions crossing the boundary), mirroring the shim-
generation strategy other memory-safe languages use for C++ interop,
rather than attempting to parse and bind arbitrary C++ headers
directly. Noctivue code then talks to the shim through the ordinary
§1–8 C-ABI mechanism — this tier adds a code-generation step in front
of existing FFI, not a new runtime boundary type.

**Explicitly out of scope for the first cut:** C++ templates/generics,
exceptions unwinding across the boundary, and multiple inheritance —
each is individually revisited only if the shim-generation approach
proves viable for the common case (plain classes, single inheritance,
non-template functions) first.

**Exit criteria (M6, IMPLEMENTATION_PLAN.md Phase 7):** a real-world
C++ library with a plain-function/simple-class surface (no templates)
is called from Noctivue through a generated shim, round-tripping a
non-trivial object without a memory-safety violation under a sanitizer
run.
