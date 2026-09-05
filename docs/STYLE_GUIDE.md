# STYLE_GUIDE.md — Style Guide & Developer-Facing File Identity

**Status:** Naming conventions Proposed (recommendation, not enforced).
File-identity requirements Confirmed (ADR-012); logo artwork Deferred.

## 1. Design Principles Recap

See ARCHITECTURE.md §3 for the full ten-principle list; the style guide
operationalizes Principles 3, 5, 6, and 10 specifically.

## 2. Naming Conventions (recommended, not compiler-enforced)

```text
Types           PascalCase     User, DatabaseConnection, HttpServer
Functions       snake_case     calculate_total, load_user, fetch_data
Variables       snake_case     user, user_count, is_active
Constants       SCREAMING_CASE MAX_CONNECTIONS, DEFAULT_TIMEOUT
UI components   PascalCase     Dashboard, UserCard, LoginPage, RevenueGraph
```

The compiler does **not** reject code that violates these conventions —
naming style is never load-bearing for parsing (COMPILER_ARCHITECTURE.md
§4). `noct fmt`/`noct lint` may optionally flag violations, but such
rules ship **default-off** (DECISIONS.md Issue 7) to avoid the core
tooling silently imposing a house style teams haven't opted into.

## 3. Concise vs. Explicit Declaration Style

Both are canonical (SYNTAX.md §4). The style guide's only
recommendation: prefer concise (`User:`) by default; reach for explicit
(`struct User:`) when it genuinely aids a reader unfamiliar with the
codebase, not as a blanket house rule. Teams that want a stricter,
enforced convention may enable the corresponding opt-in lint rule.

## 4. Formatting

Formatting decisions (line breaking, semicolon placement, brace vs.
indentation choice at the point of writing) belong to `noct fmt`, not to
individual authors' hand-formatting (SYNTAX.md §7, TOOLCHAIN.md §4).

## 5. Documentation Comments

Use `///` for declarations and `//!` for module-level documentation
(LANGUAGE_SPEC.md §3); write doc comments as complete sentences, since
they are extracted verbatim into generated docs and AI-tooling AST
output.

## 6. Developer-Facing File Identity (ADR-012)

A programming language's identity is communicated not only through
syntax but through file extensions, icons, syntax highlighting, CLI
branding, package naming, editor integration, documentation, and
tooling. This section is the canonical requirements source; it is also
referenced from LANGUAGE_SPEC.md, TOOLCHAIN.md, and ARCHITECTURE.md so
none of those documents need to restate it.

### 6.1 Canonical Identifiers

```text
Language name:     Noctivue
Short name:        nv
File extension:    .nv
Proposed MIME type: text/x-noctivue   [PROPOSED — not formally registered]
```

Tooling that needs a language identifier **MUST** use `noctivue` (full)
or `nv` (short) consistently rather than inventing per-editor variants.

### 6.1b Project-Adjacent File Identity (ADR-017)

```text
Manifest:   nestpkg.nvpm   (custom Noctivue-flavored syntax, exact filename)
Lockfile:   nestpkg.lock   (generated, checked in — no editor identity)
Env file:   *.nv.env       (dotenv-compatible KEY=VALUE)
```

Editors associate `nestpkg.nvpm` by exact filename (highlighting
approximates the `noctivue` grammar until a dedicated injection grammar
exists). `nestpkg.lock`, being generated, gets none of the `.nv`
identity per §6.7. `*.nv.env` files end in `.env` so existing
dotenv tooling highlights them with no Noctivue-specific work.

### 6.2 File Identity Requirement

`.nv` is the canonical identity of Noctivue source code — not merely a
technical suffix. Editors, IDEs, operating systems, file managers, Git
clients, documentation systems, and developer tools **SHOULD**
eventually recognize `*.nv` as "Noctivue Source Code" with an
associated file icon, recognizable even at small sizes.

### 6.3 Brand Asset Set (Deferred — design task, not a spec task)

```text
Noctivue Logo       full brand mark
Noctivue Symbol      icon usable independently of the wordmark
Noctivue File Icon    the .nv file-type icon specifically
Noctivue Wordmark     text logotype
```

The symbol/icon **MUST** remain recognizable at 16×16 and **SHOULD**
scale cleanly through 24, 32, 48, 64, 128, 256, and 512 px. The exact
visual design is explicitly **Deferred** as a branding/design task, not
something to freeze inside the language grammar specification
(design brief §8: "leave the exact visual design as a separate
branding decision").

**Icon design principle:** the icon **MUST NOT** simply be the text
"NV" inside a generic document icon. It should communicate technology,
precision, modernity, creativity, and software engineering, without
becoming visually complicated — in the spirit of how developers
recognize major language icons at a glance (Rust's gear, Python's
snakes) without reading text.

### 6.4 Editor Integration (Deferred, prioritized roughly in this order)

```text
Visual Studio Code   (first official target — file icon, syntax
                      highlighting, language mode, formatter,
                      diagnostics, autocomplete, go-to-definition,
                      symbol navigation, hover info, refactoring,
                      debugging support when available)
JetBrains IDEs
Neovim
Zed
Sublime Text
other editors supporting language/file associations
```

### 6.5 Operating System File Association (Deferred)

The installer/toolchain should eventually register `.nv` with supported
operating systems such that double-clicking a `.nv` file opens it in
the user's configured Noctivue-aware editor rather than attempting to
execute it. The association **MUST NOT** hard-code a specific editor —
it stays compatible with user-selected tooling.

### 6.6 Repository/Search Tool Detection (Deferred)

GitHub, GitLab, Bitbucket, Sourcegraph, code-search systems, and
documentation generators should eventually recognize `.nv` as Noctivue
source rather than generic text (via a linguist-style language
definition contribution once the project is public).

### 6.7 Generated/Internal Files

`.nv` represents human-written Noctivue source code specifically.
Generated/internal artifacts (e.g., `.nvir` for a serialized NIR
artifact, if it becomes useful) **MUST NOT** visually compete with
`.nv` in editors/file browsers — e.g., they should not share the same
icon or be surfaced in the same "source files" view by default.

### 6.8 Documentation Placement

This requirement (Developer-Facing File Identity) is intentionally
cross-referenced, not duplicated in full, from LANGUAGE_SPEC.md,
TOOLCHAIN.md, and ARCHITECTURE.md — this file (STYLE_GUIDE.md §6) is
the single source of truth for it, per the design brief's request that
`ARCHITECTURE.md`, `LANGUAGE_SPEC.md`, `TOOLCHAIN.md`, and
`STYLE_GUIDE.md` all reference a "Developer-Facing File Identity"
section without four divergent copies drifting out of sync.
