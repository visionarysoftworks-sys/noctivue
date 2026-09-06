# Noctide IDE

## Official Product & Architecture Specification

**Project:** Noctivue
**Product:** Noctide
**Designation:** The Noctivue IDE
**Status:** Finalized Product Direction
**Target:** M5–M8
**Primary Implementation Goal:** Noctide ultimately becomes a Noctivue application built substantially in Noctivue itself.

---

# 1. Product Identity

**Noctide** is the official integrated development environment for the Noctivue programming language and ecosystem.

Noctide is not intended to be:

* a generic text editor,
* a VS Code clone,
* merely an LSP client,
* a syntax-highlighting application,
* or an IDE framework with Noctivue support added afterward.

Noctide is a **language-specific development environment**, comparable in role to the dedicated IDEs built around major programming ecosystems.

Its purpose is to provide one environment in which developers can understand, create, build, test, debug, document, and maintain Noctivue software.

The ultimate objective is for Noctide to be capable of developing **Noctivue itself**.

---

# 2. Core Philosophy

Noctide should understand Noctivue code rather than merely display it.

This requires deep integration with the Noctivue language infrastructure.

The IDE should understand:

* lexical structure,
* syntax,
* modules,
* declarations,
* types,
* symbols,
* scopes,
* imports,
* visibility,
* generics,
* functions,
* structures,
* traits/interfaces where applicable,
* expressions,
* diagnostics,
* dependencies,
* targets,
* build configuration,
* tests,
* documentation,
* and the relationships between all of these.

The IDE therefore operates on a **semantic model of the program**, not simply a collection of text files.

---

# 3. Noctide's Relationship to Noctivue

Noctide is part of the Noctivue ecosystem rather than an independent development platform.

The intended architecture is:

```text
                    Noctide
                       │
             ┌─────────┴─────────┐
             │                   │
       IDE Presentation     IDE Services
             │                   │
             └─────────┬─────────┘
                       │
             Noctivue Language
               Infrastructure
                       │
          ┌────────────┼────────────┐
          │            │            │
        Lexer        Parser     Semantic Model
          │            │            │
          └────────────┼────────────┘
                       │
                Compiler / LSP
                       │
                 Runtime / CLI
```

Noctide should reuse the authoritative Noctivue language infrastructure wherever practical.

The IDE must not independently reimplement the language's semantics.

---

# 4. Universal Noctivue Project Model

Noctivue should not divide projects into fundamentally different project species such as:

```text
app
server
desktop
library
wasm
embedded
```

These describe **uses and targets**, not fundamentally different kinds of Noctivue projects.

A Noctivue project remains a Noctivue project regardless of what it produces.

The project's manifest is the authoritative source for its configuration, dependencies, targets, and capabilities.

Conceptually:

```text
                 Noctivue Project
                        │
                     .nvpm
                        │
             ┌──────────┴──────────┐
             │                     │
          Targets              Capabilities
             │                     │
      native / wasm /       networking / GUI /
      embedded / etc.       filesystem / etc.
```

This permits one codebase to support multiple targets without requiring the project to be migrated between project types.

---

# 5. Project Creation

Noctivue's official project creation mechanism is:

```text
noct create <project-name>
```

The project generator is a first-class part of the Noctivue toolchain.

Project creation is not simply the copying of a directory template.

The generator should:

1. establish the project identity,
2. create the package manifest,
3. establish the initial configuration,
4. determine declared targets and capabilities,
5. select the required project components,
6. generate the source structure,
7. generate testing infrastructure,
8. generate documentation infrastructure,
9. generate required target/platform structures,
10. leave the project immediately recognizable as a standard Noctivue project.

The project's filesystem should therefore be a **materialized representation of its declared configuration**.

---

# 6. Canonical Source Space

The canonical Noctivue source directory is intended to be:

```text
lib/
```

`lib/` does **not** mean that the project is a library.

It means the package's Noctivue source/module space.

For example:

```text
my_project/
├── nestpkg.nvpm
├── nestpkg.lock
├── lib/
│   ├── main.nv
│   ├── user.nv
│   └── network/
│       └── client.nv
├── tests/
├── docs/
└── README.md
```

Applications, libraries, servers, command-line programs, WASM components, desktop software, embedded software, and other Noctivue programs can all use the same source convention.

The target does not need to be encoded in the source directory.

---

# 7. Target-Aware Project Generation

Noctivue projects may declare one or more targets in their package manifest.

The project generator and IDE should inspect that configuration when determining which target-specific structures are required.

For example:

```text
my_project/
├── nestpkg.nvpm
├── lib/
├── tests/
└── docs/
```

may represent a minimal project.

A multi-platform project may contain additional structures:

```text
my_project/
├── nestpkg.nvpm
├── lib/
├── tests/
├── docs/
├── windows/
├── linux/
└── wasm/
```

The exact target directories are determined by the Noctivue target system.

They should not be treated as separate project types.

Unnecessary platform structures should not be generated merely because they might be useful someday.

---

# 8. Noctide Project Creation

Noctide must use the same project-generation model as the `noct` CLI.

The architecture is:

```text
                    Noctivue Project Model
                              │
                       Project Generator
                         /             \
                        /               \
                     noct             Noctide
```

Noctide must not maintain a separate project-generation implementation that produces subtly different projects.

A project created through:

```text
noct create my_project
```

and a project created through:

```text
Noctide → New Project
```

must follow the same canonical project model.

The IDE may provide a graphical configuration experience, but the resulting project must be generated through the same authoritative machinery.

---

# 9. Core Noctide Capabilities

Noctide is intended to provide the following major capabilities.

## 9.1 Code Editor

* syntax highlighting,
* semantic highlighting,
* code completion,
* code navigation,
* symbol search,
* go-to-definition,
* go-to-declaration,
* find usages,
* rename refactoring,
* formatting,
* diagnostics,
* quick fixes,
* code folding,
* multiple files and views,
* project-wide search.

---

## 9.2 Semantic Language Intelligence

Noctide should understand the structure and meaning of Noctivue programs.

Examples include:

```text
user.name
```

where the IDE understands:

* what `user` is,
* its type,
* whether `name` exists,
* whether it is accessible,
* the type of `name`,
* and what operations are valid on the resulting value.

Semantic understanding should power completion, navigation, diagnostics, refactoring, documentation, and debugging.

---

## 9.3 Project Model

Noctide should understand:

* `.nvpm` manifests,
* lockfiles,
* modules,
* packages,
* dependencies,
* targets,
* capabilities,
* generated platform structures,
* source roots,
* tests,
* documentation,
* and build configuration.

The project model must remain consistent with the official Noctivue toolchain.

---

## 9.4 Build and Run

Noctide should provide integrated access to:

* build,
* run,
* clean,
* target selection,
* configuration,
* build diagnostics,
* build output,
* and runtime execution.

These operations should use the official Noctivue toolchain rather than implementing an independent build system.

---

## 9.5 Testing

Noctide should provide integrated support for the Noctivue testing system.

Capabilities should include:

* discovering tests,
* running individual tests,
* running test groups,
* running the complete test suite,
* displaying failures,
* navigating directly to failures,
* and debugging tests.

The underlying test execution remains part of the official Noctivue tooling.

---

## 9.6 Debugging

Noctide should eventually provide a professional debugger with:

* breakpoints,
* conditional breakpoints,
* stepping,
* call stacks,
* locals,
* watches,
* variable inspection,
* exception/error inspection,
* thread/task inspection,
* and target-aware debugging.

Debugger support should work across the targets supported by the Noctivue runtime/toolchain where technically possible.

---

## 9.7 Package Management

Noctide should provide a graphical interface over the official Noctivue package-management system.

It should understand:

* dependencies,
* versions,
* lockfiles,
* package integrity,
* package sources,
* target compatibility,
* dependency trust information,
* and package operations.

Noctide must not create a second package-management model.

---

## 9.8 Documentation

Noctide should integrate with Noctivue documentation facilities.

Developers should be able to:

* inspect documentation while coding,
* navigate symbols through documentation,
* generate documentation,
* preview documentation,
* and navigate package APIs.

---

## 9.9 Formatting and Linting

Noctide should integrate directly with the official:

```text
noct fmt
noct lint
```

tooling.

Formatting and linting rules should remain authoritative at the Noctivue toolchain level.

---

## 9.10 Integrated Terminal

Noctide should provide a terminal suitable for running:

```text
noct build
noct run
noct test
noct fmt
noct lint
noct doc
noct add
```

and other Noctivue commands.

The terminal complements IDE functionality rather than replacing the CLI.

---

# 10. Multi-Target Development

One of Noctide's important capabilities is awareness of Noctivue's multi-target model.

A project may target:

* native platforms,
* WASM,
* embedded systems,
* desktop platforms,
* and future Noctivue targets.

Noctide should understand the currently selected target and provide appropriate:

* diagnostics,
* APIs,
* completion,
* build configuration,
* dependency information,
* platform structures,
* and debugging facilities.

The target should be treated as **project configuration**, not as a completely different project identity.

---

# 11. Foreign Ecosystem Integration

Noctivue is intended to support controlled interoperability with external ecosystems.

Noctide should eventually understand dependencies involving mechanisms such as:

* native libraries,
* C interfaces,
* C++ interfaces,
* Rust components,
* WASM components,
* and other supported foreign runtimes.

The IDE should expose relevant information without forcing the developer to abandon the Noctivue project model.

Where applicable, Noctide should display:

* dependency origin,
* integration mechanism,
* target compatibility,
* trust/integrity information,
* and required tooling.

---

# 12. Version Control

Noctide should provide integrated Git functionality, including:

* repository status,
* change tracking,
* diffs,
* staging,
* commits,
* branches,
* history,
* conflict resolution,
* and navigation between source revisions.

Version-control functionality should integrate naturally with the Noctivue project model.

---

# 13. User Interface Philosophy

Noctide should feel like a **professional development environment**, not a text editor with panels attached.

The UI should be designed around developer workflows:

```text
Project
Code
Structure
Build
Run
Test
Debug
Documentation
Dependencies
Version Control
```

The interface should expose complexity progressively rather than presenting every Noctivue subsystem simultaneously.

A new developer should be able to open a project and begin coding without understanding the entire compiler/toolchain architecture.

An advanced developer should be able to access the deeper system when required.

---

# 14. Implementation Strategy

Noctide should evolve toward being implemented in Noctivue itself.

This does not require the first prototype to be entirely written in Noctivue.

The intended progression is:

```text
Early prototype
    ↓
Mixed implementation
    ↓
Increasing Noctivue implementation
    ↓
Predominantly Noctivue
    ↓
Noctide substantially implemented in Noctivue
```

This is both an engineering goal and a demonstration of Noctivue's maturity.

The IDE should eventually be capable of participating in its own development.

---

# 15. Milestone Roadmap

## M3 — Language Tooling Foundations

Establish the infrastructure Noctide will eventually consume:

* parser infrastructure,
* semantic analysis,
* diagnostics,
* symbol information,
* formatter,
* linter,
* testing tooling,
* LSP,
* package/project model foundations.

Noctide itself does not need to exist yet.

---

## M4 — Platform Foundation

Establish the deeper infrastructure required by the future IDE:

* stable compiler interfaces,
* runtime interfaces,
* package management,
* project generation,
* target model,
* documentation tooling,
* debugger foundations,
* cross-platform infrastructure.

The official `noct create` project-generation system becomes a foundational toolchain capability.

---

## M5 — Noctide Genesis

First functional Noctide generation.

Goals:

* open Noctivue projects,
* understand `.nvpm`,
* edit `.nv` files,
* semantic syntax highlighting,
* diagnostics,
* completion,
* navigation,
* formatting,
* project tree,
* build,
* run,
* test,
* basic documentation access.

Noctide becomes a usable Noctivue development environment.

---

## M6 — Noctide Eclipse

Professional development capabilities become substantially mature.

Goals:

* advanced semantic analysis,
* refactoring,
* package management,
* integrated testing,
* advanced diagnostics,
* debugger integration,
* Git integration,
* target awareness,
* improved project management,
* documentation workflows,
* improved UI/UX.

---

## M7 — Noctide Aurora

Noctide becomes a serious professional IDE for the broader Noctivue ecosystem.

Goals:

* multi-target development,
* advanced debugging,
* foreign ecosystem integration,
* WASM workflows,
* embedded workflows,
* sophisticated dependency management,
* advanced refactoring,
* performance tooling,
* deep compiler integration.

Noctide should be capable of handling large Noctivue codebases, including the Noctivue compiler/runtime ecosystem itself.

---

## M8 — Noctide Zenith

The mature Noctivue IDE.

Noctide should be capable of developing Noctivue itself from within Noctivue.

The intended development loop becomes:

```text
              Noctivue
                  │
                  ▼
             builds Noctide
                  │
                  ▼
               Noctide
                  │
                  ▼
           develops Noctivue
                  │
                  └──────────────┐
                                 ▼
                              Noctivue
```

---

# 16. M8 Acceptance Criteria

Noctide Zenith should be considered successful when a developer can:

1. Open the Noctivue repository in Noctide.
2. Understand the repository's project structure.
3. Navigate the compiler, runtime, standard library, CLI, LSP, tests, and examples.
4. Receive semantic diagnostics while editing Noctivue.
5. Navigate symbols and dependencies.
6. Refactor Noctivue code.
7. Build Noctivue.
8. Run Noctivue tests.
9. Debug Noctivue.
10. Work with Noctivue packages and dependencies.
11. Build Noctide.
12. Run Noctide.
13. Continue developing Noctivue using Noctide.

The final test is not merely whether Noctide can edit Noctivue.

The final test is whether **Noctide is itself a serious Noctivue development environment**.

---

# 17. Architectural Rules

The following principles are considered fundamental.

### Rule 1 — Noctide is Noctivue-specific

Noctide exists to provide the best possible development environment for Noctivue.

### Rule 2 — The compiler remains authoritative

Noctide must not independently redefine Noctivue semantics.

### Rule 3 — One project model

CLI-created and IDE-created projects must follow the same project model.

### Rule 4 — `.nvpm` is authoritative

Project configuration, targets, capabilities, and dependencies ultimately come from the official package/project model.

### Rule 5 — No artificial project species

Application, server, library, desktop, WASM, and embedded use cases are not fundamentally different Noctivue project types.

### Rule 6 — Shared source model

`lib/` provides the common Noctivue source/module space unless a future architectural decision establishes a better convention.

### Rule 7 — One toolchain

Noctide integrates with the official Noctivue compiler, package manager, formatter, linter, test system, documentation system, and debugger.

### Rule 8 — No duplicated project generation

Noctide and `noct create` must use the same project-generation architecture.

### Rule 9 — Progressive implementation

Noctide may begin with external or mixed implementation technologies, but its long-term direction is toward substantial Noctivue implementation.

### Rule 10 — Self-hosting is the destination

Noctide should ultimately be capable of developing the language and ecosystem that created it.

---

# 18. Final Product Definition

**Noctide is the official integrated development environment for Noctivue.**

It provides a unified environment for understanding, creating, editing, building, testing, debugging, documenting, and maintaining Noctivue software across the ecosystem's supported targets.

It does not introduce a second project model, package model, build system, or language implementation.

Instead, it presents the existing Noctivue ecosystem through a deeply integrated development environment.

Its defining long-term capability is:

> **Noctide can develop Noctivue, while itself being developed in Noctivue.**

That completes the intended Noctivue development loop:

```text
Language
   ↓
Compiler
   ↓
Toolchain
   ↓
Noctide
   ↓
Noctivue development
   ↓
Further evolution of the language
```

**Noctide Zenith is the point at which this loop becomes practical.**
