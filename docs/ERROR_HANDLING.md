# ERROR_HANDLING.md — Error Handling

**Status:** Confirmed core model.

## 1. `Result<T, E>` for Recoverable Errors

```nv
enum Result<T, E>:
    Ok(T)
    Err(E)
```

Any operation that can fail in an expected, recoverable way returns
`Result<T, E>` rather than throwing.

## 2. Propagation with `?`

```nv
load_user(id: Int) -> Result<User, Error>:
    database.users.find(id)?
```

`expr?` on a `Result<T, E>`: if `Ok(v)`, evaluates to `v`; if `Err(e)`,
immediately returns `Err(e)` from the enclosing function (which
therefore **MUST** itself return a compatible `Result`). This is
purely control-flow sugar — it does not perform unwinding, allocation,
or hidden dynamic dispatch, keeping it usable in native mode.

## 3. No Conventional Exceptions

The core language has no `throw`/`catch`/`try` exception mechanism.
This is deliberate: exceptions introduce non-local, often untyped
control flow that works against static, exhaustively-checked error
paths. `Result` + `?` makes every fallible call visible in a function's
signature.

## 4. Panics

Panics are reserved for **unrecoverable programmer/invariant
failures** — a violated precondition, an index out of bounds, an
assertion failure — situations where continuing execution would be
unsound, not situations an API caller is expected to handle.

- A panic in **native mode** aborts the current process (or, where the
  platform/runtime configuration allows, unwinds to a defined boundary —
  exact unwind-vs-abort configurability is **Deferred**).
- A panic in **managed mode** (e.g., inside a UI event handler)
  **SHOULD** be caught at a well-defined boundary (e.g., the framework's
  event dispatch loop) and surfaced as a developer-visible error rather
  than crashing the whole application — exact boundary semantics are
  **Deferred** to UI_SPEC.md once the managed runtime is prototyped.

## 5. `Option<T>` vs `Result<T, E>`

Use `Option<T>` when absence itself is not an error (`find_user` may
legitimately find nothing). Use `Result<T, E>` when failure carries
information about *why* (`parse_config` failing needs to explain what
was malformed). This convention is documented, not compiler-enforced.

## 6. Error Types

`E` in `Result<T, E>` is an ordinary Noctivue type — typically an
`enum` describing the failure cases of a module. There is no
language-mandated `Error` trait in v0.1; a standard-library error
trait/convention (for interop between libraries' error enums) is
**Deferred** to the stdlib design phase, not the core language.
