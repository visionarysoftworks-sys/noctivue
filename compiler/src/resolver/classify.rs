//! Declaration classification for bare colon-block declarations.
//!
//! This module implements the structural body-shape analysis that the resolver
//! uses to decide what kind of declaration a [`crate::ast::BareDecl`] is.
//! See COMPILER_ARCHITECTURE.md §4 for the normative algorithm.
//!
//! ## Classification rules (structural, not convention-based)
//!
//! A `BareDecl` is classified as:
//!
//! | Kind | Body shape |
//! |------|------------|
//! | **struct** | body consists exclusively of `field_decl`-shaped lines (`identifier: type_expr`) with no call expressions. |
//! | **function** | return type (`-> Type`) is present, OR the body contains `return`/`let`/`var`/other statement forms rather than field declarations. |
//! | **component (builder call)** | body consists of nested calls (`identifier(...):` / `identifier(...)` patterns) that resolve to functions or macros rather than types. |
//!
//! ## Cycle detection
//!
//! Some classifications are *transitive*: determining whether `Splash:` is a
//! function or a component depends on knowing what `loading_screen` classifies
//! as. A genuine cycle (A → B → A) MUST be detected and reported with a
//! `"declaration classification cycle"` diagnostic rather than causing
//! unbounded recursion (EXECUTION_GUIDE.md Phase 0 precision points).
//!
//! Three cycle shapes to validate (EXECUTION_GUIDE.md Phase 0):
//! - Direct: A → B → A
//! - Indirect: A → B → C → A
//! - Self-reference: A → A

use crate::ast::{BareDecl, StructDecl, FunctionDecl, Item};
use crate::diagnostics::{Diagnostic, DiagnosticSink};

/// The outcome of classifying a [`BareDecl`].
#[derive(Debug, Clone)]
pub enum Classification {
    Struct(StructDecl),
    Function(FunctionDecl),
    /// A component/builder-call desugaring. The inner `Item` is the
    /// desugared representation (pending desugaring design — DECISIONS.md Issue 2).
    Component(Box<Item>),
    /// Classification could not complete because a dependency cycle was
    /// detected. A diagnostic has already been emitted.
    Cycle,
}

/// Classify a single [`BareDecl`] using structural body-shape analysis.
///
/// `pending` is the set of declarations currently being classified (used for
/// cycle detection). If `decl.name` is already in `pending`, the resolver has
/// detected a self-reference or indirect cycle and MUST emit a diagnostic.
pub fn classify(
    decl: &BareDecl,
    pending: &std::collections::HashSet<String>,
    sink: &mut DiagnosticSink,
) -> Classification {
    // Cycle detection: if this declaration is already in the pending set,
    // we have a classification cycle.
    if pending.contains(&decl.name) {
        sink.emit(
            Diagnostic::error(format!(
                "declaration classification cycle: `{}` depends on its own classification",
                decl.name
            ))
            .with_span(decl.span.clone(), "cycle detected here")
            .with_code("E0010"),
        );
        return Classification::Cycle;
    }

    // TODO (Phase 1): implement structural classification.
    // Stub: everything is classified as a function so the workspace compiles.
    let _ = sink;

    // Minimal stub — returns an empty FunctionDecl to satisfy the type system.
    Classification::Function(FunctionDecl {
        name: decl.name.clone(),
        generic_params: Vec::new(),
        params: decl.params.clone().unwrap_or_default(),
        return_ty: decl.return_ty.clone(),
        body: crate::ast::FunctionBody::Block(decl.body.clone()),
        span: decl.span.clone(),
    })
}
