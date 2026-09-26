//! Back-compat shim: the manifest + lockfile implementation now lives in
//! the shared `nestpkg` crate so both `noct` and `noctivue-lsp` use one
//! parser (single source of truth, no drift).
//!
//! All existing `crate::manifest::…` paths keep working through this
//! re-export. New code should depend on `nestpkg` directly.

pub use nestpkg::*;
