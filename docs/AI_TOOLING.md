# AI_TOOLING.md — AI-Native Compiler Interfaces

**Status:** Principle Confirmed (ADR-011); exact command/schema surface Proposed.

## 1. Goal

Noctivue is designed for AI coding agents from the start, not retrofitted
later. The compiler exposes machine-readable information as a first-
class toolchain capability, so agents can inspect, reason about, and
safely modify code without screen-scraping human-oriented CLI output.

## 2. Initial Interfaces (M0/M1 priority)

```text
noct ast --json           structured AST for a file/package
noct diagnostics --json   structured compiler diagnostics (errors, warnings,
                           suggested fixes) — see COMPILER_ARCHITECTURE.md §6
```

## 3. Later Interfaces (post-M1)

```text
noct symbols --json       symbol table / scope information
noct type --json          type information for a position/expression
noct ir --json            NIR dump in structured form
noct format --json        formatter operations as structured edits
                           (rather than only whole-file text replacement)
```

## 4. Design Requirements

- **No screen-scraping required:** every `--json` interface **MUST**
  carry the same information a human-facing report shows, structured,
  not a JSON-wrapped string blob of the human text.
- **Precise source locations:** diagnostics and AST nodes **MUST**
  carry file/line/column/byte-offset spans sufficient for an agent to
  apply an edit without re-parsing surrounding context to find it.
- **Stable-enough schemas:** JSON schemas for these interfaces are
  versioned independently of language version, since tooling consumers
  (including AI agents) update on a different cadence than language
  releases.
- **Composability with diagnostics:** where the compiler can suggest a
  concrete fix (e.g., a borrow-check error with a clear resolution), the
  structured diagnostic **SHOULD** include the suggested edit in the
  same machine-readable form used by `noct format --json`, not as free text.

## 5. Non-Goals

This is not a proposal for the compiler itself to embed a model or
perform AI-specific transformations — it is strictly about exposing
existing compiler-internal information (AST, types, diagnostics, IR) in
a form that's equally useful to any external tool, human-facing IDE, or
AI agent.
