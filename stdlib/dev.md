# Noctivue Standard Library (`stdlib/`)

## Purpose

The `stdlib/` directory is the **official Noctivue Standard Library**.

It contains the libraries, modules, types, utilities, and common APIs that are shipped as part of the Noctivue language and are intended to be available to Noctivue programs.

The standard library exists to provide a consistent foundation for building Noctivue applications without requiring every project to reimplement common functionality.

The standard library is part of the Noctivue platform. It is not an example application, not a collection of third-party packages, and not a place for project-specific code.

---

## Core Principles

### 1. `stdlib/` belongs to the language

Everything placed under `stdlib/` must be considered part of the Noctivue ecosystem.

Changes to the standard library can affect:

- Noctivue programs
- the compiler
- the interpreter
- the native runtime
- the package manager
- documentation
- examples
- tests
- future versions of the language

Therefore, standard-library APIs must be designed deliberately and changed carefully.

---

### 2. The standard library must be Noctivue-native

The public API of the standard library should be written for Noctivue programmers.

Rust, C, C++, operating-system, or other implementation details may exist behind the library when required, but those implementation details must not unnecessarily leak into the Noctivue API.

For example, a native implementation may use Rust internally, while the Noctivue programmer should interact with a clean Noctivue abstraction.

Conceptually:

```text
Noctivue program
       ↓
Noctivue stdlib API
       ↓
runtime / native implementation
       ↓
operating system