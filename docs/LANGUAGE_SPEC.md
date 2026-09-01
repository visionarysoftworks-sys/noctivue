# LANGUAGE_SPEC.md — Noctivue Language Specification v0.1

**Status:** Experimental. This document defines the lexical foundation and
core keyword set. Grammar productions live in [SYNTAX.md](./SYNTAX.md);
this file covers what SYNTAX.md's terminals mean.

## 1. Source Encoding

- Source files **MUST** be UTF-8, without a byte-order mark.
- Identifiers **MAY** contain any Unicode letter/digit per
  [UAX #31](https://unicode.org/reports/tr31/) (`XID_Start`/`XID_Continue`),
  enabling identifiers such as:

```nv
let café = "Kampala"
let привет = "Hello"
```

- Noctivue is **case-sensitive**. `user` and `User` are distinct identifiers.

## 2. Whitespace, Newlines, Indentation

- Indentation is structurally significant in the default (indentation)
  syntax (SYNTAX.md §2). Recommended indentation width: **4 spaces**.
  Tabs and spaces **MUST NOT** be mixed within one block's indentation.
- Inconsistent indentation (e.g., a dedent that doesn't match any
  enclosing indentation level) **MUST** produce a compiler error, never a
  best-effort guess.
- Newlines terminate statements in indentation mode unless the previous
  line ends with a token that syntactically requires continuation (an
  open paren/bracket/brace, a trailing binary operator, etc.).
- Braces (`{ }`) are always a legal alternative block delimiter and
  suspend indentation-sensitivity within them (SYNTAX.md §7).

## 3. Comments

```nv
// line comment
/* block comment */
/// documentation comment (attaches to the following declaration)
//! module-level documentation comment
```

Doc comments (`///`, `//!`) are parsed into structured documentation and
are available to `noct doc` and `noct ast --json` (AI_TOOLING.md).

## 4. Identifiers & Naming Conventions

Identifiers start with `XID_Start` or `_`, continue with `XID_Continue`
or `_`. Naming *style* (PascalCase types, snake_case functions) is a
formatter/linter recommendation, not a compiler-enforced rule — see
STYLE_GUIDE.md §2.

## 5. Literals

```text
Integers:    42   1_000_000   0xFF   0o17   0b1010
Floats:      3.14   2.0e10   1_000.5
Booleans:    true   false
Characters:  'a'   '\n'   '\u{1F600}'
Strings:     "hello"
Multiline:   """
             multiple
             lines
             """
Interpolated: "Hello, {name}!"
              "Total: {price * quantity}"
```

- Integer literals default to `Int` (platform-width signed integer,
  see TYPE_SYSTEM.md §2) unless a suffix or contextual type says
  otherwise.
- String interpolation `{expr}` accepts arbitrary expressions, evaluated
  in the enclosing scope. `\{` / `\}` escape literal braces.
- Underscores in numeric literals are purely for readability and carry
  no semantic meaning.

## 6. Operators & Punctuation

```text
Arithmetic:    + - * / %
Comparison:    == != < <= > >=
Logical:       && || !
Assignment:    = += -= *= /= %=
Range:         .. ..=
Optional/err:  ? ??
Arrow:         ->
Access:        . ::
Structural:    : , ; ( ) [ ] { }
```

Full precedence table: SYNTAX.md §9.

## 7. Keyword Table

### 7.1 Core keywords (Confirmed, reserved from v0.1)

```text
as       async    await    break    const    continue
else     export   false    for      if       import
in       let      loop     match    return   true
var      while
```

### 7.2 Type/system keywords (Confirmed, reserved — used only in
*explicit* style; see SYNTAX.md §4)

```text
enum     fn       impl     struct   trait    type     unsafe
```

### 7.3 Memory/concurrency keywords (Confirmed, reserved)

```text
owned    borrow   managed  weak     unowned  task
```

`managed` is reserved as a keyword now; its exact declaration-site
syntax is **Proposed**, not finalized (MEMORY_MODEL.md §4).

### 7.4 Reserved for future use (Confirmed as reserved words; unassigned semantics)

```text
actor    defer    extern   macro    native   operator
protocol reflect  spawn    static   where    yield
```

These are reserved conservatively — deliberately not a large list — so
that adding the feature later does not break existing programs that used
the identifier as a name. No semantics are assigned yet
(DECISIONS.md §4).

### 7.5 Contextual keywords

None in v0.1. If a future feature needs a contextual keyword (recognized
only in specific grammar positions), it will be proposed via ADR rather
than added silently, since contextual keywords interact with the
colon-first parsing strategy (COMPILER_ARCHITECTURE.md §4) and must be
checked against it explicitly.

## 8. Modules & Compilation Units

A `.nv` file is a compilation unit. See MODULES.md for `import`/`export`
semantics, path resolution, and package boundaries.

## 9. Normative Language

This and all linked documents use RFC-2119-style terms:

- **MUST / MUST NOT** — hard requirement.
- **SHOULD / SHOULD NOT** — strong recommendation; deviation needs justification.
- **MAY** — genuinely optional.

Not every sentence in this documentation set is normative — narrative and
rationale text is not, and is written in ordinary prose.
