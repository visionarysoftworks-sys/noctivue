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
Lockfile:   nestpkg.lock   (generated, checked in — read-only servicing, §6.7)
Env file:   *.nv.env       (dotenv-compatible KEY=VALUE)
```

Editors associate `nestpkg.nvpm` by exact filename. Manifest and
lockfile are served by the dedicated `nestpkg` language — same
extension and same language-server binary as `noctivue` (one
official everything, TOOLCHAIN.md §1): the manifest gets
diagnostics, hover, completions, outline, and canonical formatting,
while the lockfile, being generated, gets a read-only posture
(parse diagnostics; never hand-edited). Neither carries the `.nv`
source identity (no `.nv` grammar, no source-view surfacing, §6.7).
The registry index stores each published version's own manifest as
`manifest.nvpm` — same schema, same parser, server-side data with
no editor identity. `*.nv.env` files end in `.env` so existing
dotenv tooling highlights them with no Noctivue-specific work; no
Noctivue language is associated with them (and bare `*.env` is
never claimed).

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
Generated/internal artifacts **MUST NOT** visually compete with
`.nv` in editors/file browsers — in practice this is satisfied by
posture (no diagnostics, never surfaced as source, never packed),
not by a distinct icon: both artifact kinds below ride the
`noctivue` language outright (same grammar, same file identity),
with exactly one tweak — the language server publishes no
diagnostics for them, since error squiggles on generated files
would be noise.

```text
.nvc    build artifact — the compiled distribution output. This is
        what ships instead of source: no plaintext logic to copy,
        and something signatures/integrity can attach to. That
        raises the bar (deterrence + no source leakage), it does
        not make reverse engineering impossible — native code
        disassembles and bytecode decompiles, so the docs MUST
        never promise immunity. Think Flutter's `app.so`: one
        compiled module per target, found in build outputs and
        used at package/run time, never committed, never packed
        into source tarballs. No producer in the tree yet
        (`noct build` goes through the cargo piggyback and emits
        platform executables); reserved for the direct backend.
.nvir   portable interface + IR. Near term: human-inspectable IR
        dumps and incremental-build cache (the `rustc --emit-mir`
        role). Long term: the compiled module interface — public
        signatures and types without implementations — that closed
        `.nvc` distribution depends on: consumers cannot compile
        against a closed package they cannot see into, so the
        interface half is what makes the artifact half usable.
        Also the portable execution format for the managed/VM tier,
        against `.nvc`'s platform-native one.
```

Both extensions are gitignored; neither has a producer or consumer
in the toolchain today. `nestpkg.lock` is the worked example of
the read-only posture: generated, but checked in and
human-readable, so it shares the `nestpkg` language read-only
instead of going unserviced.

### 6.8 Documentation Placement

This requirement (Developer-Facing File Identity) is intentionally
cross-referenced, not duplicated in full, from LANGUAGE_SPEC.md,
TOOLCHAIN.md, and ARCHITECTURE.md — this file (STYLE_GUIDE.md §6) is
the single source of truth for it, per the design brief's request that
`ARCHITECTURE.md`, `LANGUAGE_SPEC.md`, `TOOLCHAIN.md`, and
`STYLE_GUIDE.md` all reference a "Developer-Facing File Identity"
section without four divergent copies drifting out of sync.
