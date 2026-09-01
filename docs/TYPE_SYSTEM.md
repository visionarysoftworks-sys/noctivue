# TYPE_SYSTEM.md — Type System

**Status:** Core shape Confirmed; several sub-features explicitly Deferred.

## 1. Summary

Static typing with local type inference, influenced by Rust, Swift,
Kotlin, and the ML family, without adopting any one wholesale. No
ordinary `null` — absence is represented with `Option<T>`.

## 2. Primitive Types

```text
Int      platform-width signed integer (explicit-width Int8/16/32/64/128 available)
UInt     platform-width unsigned integer (+ UInt8/16/32/64/128)
Float    64-bit IEEE 754 (Float32 available explicitly)
Bool     true / false
Char     Unicode scalar value
String   UTF-8 text
Unit     the empty/void type, written ()
```

## 3. Structs

```nv
User:
    id: Int
    name: String
    email: String
```

Equivalent explicit form: `struct User: ...` (SYNTAX.md §4).

## 4. Enums / Algebraic Data Types

```nv
enum Shape:
    Circle(radius: Float)
    Rectangle(width: Float, height: Float)
    Triangle(base: Float, height: Float)
```

`match` over enums **MUST** be exhaustive; the compiler **MUST** reject
a `match` missing a variant unless a `_` catch-all arm is present.

## 5. Tuples

```nv
let point: (Int, Int) = (3, 4)
```

## 6. Generics & Trait Bounds

```nv
fn largest<T: Comparable>(items: [T]) -> T:
    ...

trait Comparable:
    fn compare(self, other: Self) -> Int
```

Trait bounds use `+` for multiple bounds: `<T: Comparable + Clone>`.
Where-clause syntax for complex bounds is **Deferred** (not needed for
M0/M1; revisit once real generic stdlib code exists).

## 7. Traits (Interface-Like Behavior)

```nv
trait Drawable:
    fn draw(self)

impl Drawable for Circle:
    fn draw(self):
        ...
```

Traits provide Noctivue's interface/protocol mechanism. Default method
bodies in traits are **Proposed**, not yet Confirmed.

## 8. Collections

```text
[T]           growable array/list
Map<K, V>     hash map
Set<T>        hash set
```

Exact stdlib collection API surface belongs to the standard library, not
the core type system (Principle 9); only the type grammar is defined
here (SYNTAX.md §10, `type_expr`).

## 9. `Option<T>` — No Null

```nv
let maybe_user: Option<User> = find_user(id)

match maybe_user:
    Some(user): print(user.name)
    None:       print("not found")
```

`??` provides coalescing: `let name = maybe_user??.name ?? "unknown"`
(exact chained-optional semantics under `?.`-style access are **Open** —
see DECISIONS.md §4, "optional chaining semantics").

## 10. `Result<T, E>`

See ERROR_HANDLING.md for the full treatment; the type itself:

```nv
enum Result<T, E>:
    Ok(T)
    Err(E)
```

## 11. Function Types

```nv
let op: (Int, Int) -> Int = add
```

## 12. Type Aliases

```nv
type UserId = Int
type Handler = (Request) -> Result<Response, Error>
```

## 13. Type Inference

Inference is **local**: within a function body, types flow from
literals, parameter/return annotations, and prior bindings. Noctivue
does **not** perform whole-program (global) inference — every function
signature **MUST** be fully annotated. This bounds inference complexity
and keeps error messages attributable to a specific function, which
also serves the AI-tooling goal (AI_TOOLING.md) of precise, localized
diagnostics.

## 14. Explicitly Deferred / Out of Scope for v0.1

- Higher-kinded types
- Full dependent types
- Effect systems

These are deferred, not rejected — see DECISIONS.md for the amendment
process if a compelling need is demonstrated later.
