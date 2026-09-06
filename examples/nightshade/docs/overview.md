# Nightshade overview

Nightshade is a deliberately small example application rather than a
framework. The current slice models projects and tasks, exposes pure
service functions, and prints a dashboard summary.

The source demonstrates the conventions used by Noctivue today:

- `struct` and `enum` declarations for data
- `fn name(args) -> ReturnType:` for explicit functions
- four-space indentation for blocks
- immutable updates that return a new struct
- `Option<T>` and `match` for lookups

The folders are organized by responsibility, but imports are not wired
up in the current compiler. Run files together in dependency order until
module resolution is implemented.
