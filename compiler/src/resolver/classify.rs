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

use std::collections::HashMap;

use crate::ast::{BareDecl, Expr, FieldDecl, FunctionBody, FunctionDecl, Stmt, StructDecl};
use crate::diagnostics::{Diagnostic, DiagnosticSink};

/// The outcome of classifying a [`BareDecl`].
#[derive(Debug, Clone)]
pub enum Classification {
    Struct(StructDecl),
    Function(FunctionDecl),
    /// A component/builder-call body — kept as BareDecl for now, pending
    /// full desugaring design (DECISIONS.md Issue 2).
    Component,
    /// Classification could not complete because a dependency cycle was
    /// detected. A diagnostic has already been emitted.
    Cycle,
}

/// Tracks the state of each BareDecl during the classification pass.
#[derive(Debug, Clone)]
pub enum ClassificationStatus {
    /// Currently being classified — in the pending set.
    Pending,
    /// Already classified — result available.
    Known(Classification),
}

/// Extract the simple identifier name from a callee expression, if any.
/// We only look up simple `Ident` callees; member/index calls are treated
/// as builtins (not in all_decls) and thus assumed to be functions.
fn callee_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        _ => None,
    }
}

/// Classify a single [`BareDecl`] using structural body-shape analysis.
///
/// `all_decls` contains all bare declarations in the program (used for
/// transitive classification of callees). `status` tracks the current
/// classification state of each declaration (used for cycle detection and
/// memoisation).
///
/// If a classification cycle is detected, E0010 is emitted into `sink` and
/// [`Classification::Cycle`] is returned without looping infinitely.
pub fn classify(
    decl: &BareDecl,
    all_decls: &HashMap<String, BareDecl>,
    status: &mut HashMap<String, ClassificationStatus>,
    sink: &mut DiagnosticSink,
) -> Classification {
    // ── Memoisation ──────────────────────────────────────────────────────────
    // If we already classified this declaration, return the cached result.
    if let Some(ClassificationStatus::Known(cached)) = status.get(&decl.name) {
        return cached.clone();
    }

    // ── Cycle detection ──────────────────────────────────────────────────────
    // If this declaration is currently being classified, we have a cycle.
    if let Some(ClassificationStatus::Pending) = status.get(&decl.name) {
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

    // Mark this declaration as Pending before we recurse.
    status.insert(decl.name.clone(), ClassificationStatus::Pending);

    let result = classify_body(decl, all_decls, status, sink);

    // Cache the result and remove from pending.
    status.insert(decl.name.clone(), ClassificationStatus::Known(result.clone()));

    result
}

/// Classify the body of a [`BareDecl`] once it has been marked Pending.
fn classify_body(
    decl: &BareDecl,
    all_decls: &HashMap<String, BareDecl>,
    status: &mut HashMap<String, ClassificationStatus>,
    sink: &mut DiagnosticSink,
) -> Classification {
    // ── Rule 1: return type present → function immediately ───────────────────
    if decl.return_ty.is_some() {
        return Classification::Function(bare_to_function(decl));
    }

    let stmts = &decl.body.stmts;

    // ── Rule 2: empty body or all BareField → struct ──────────────────────────
    if stmts.is_empty() || stmts.iter().all(|s| matches!(s, Stmt::BareField(_))) {
        return Classification::Struct(bare_to_struct(decl));
    }

    // ── Rule 3: any explicitly non-field statement → function ─────────────────
    if stmts.iter().any(|s| is_function_statement(s)) {
        return Classification::Function(bare_to_function(decl));
    }

    // ── Rule 4: all stmts are Expr(Call(_)) → potential component ─────────────
    if stmts.iter().all(|s| is_call_expr(s)) {
        // Check each callee to see if it resolves to a function/macro.
        for stmt in stmts {
            if let Stmt::Expr(Expr::Call(call)) = stmt {
                let callee = &*call.callee;

                if let Some(name) = callee_name(callee) {
                    if let Some(callee_decl) = all_decls.get(name) {
                        // Check for cycle before recursing.
                        if let Some(ClassificationStatus::Pending) = status.get(name) {
                            // Cycle detected: the callee is currently being classified.
                            sink.emit(
                                Diagnostic::error(format!(
                                    "declaration classification cycle: `{}` depends on its own classification",
                                    name
                                ))
                                .with_span(callee_decl.span.clone(), "cycle detected here")
                                .with_code("E0010"),
                            );
                            // Update status of THIS decl — it cycled.
                            return Classification::Cycle;
                        }

                        // Recursively classify the callee.
                        let callee_class =
                            classify(callee_decl, all_decls, status, sink);

                        match callee_class {
                            Classification::Cycle => {
                                // Propagate cycle up.
                                return Classification::Cycle;
                            }
                            Classification::Struct(_) => {
                                // A call to a struct constructor — this is a
                                // function body (it's building/calling a struct),
                                // not a component. Treat as function.
                                return Classification::Function(bare_to_function(decl));
                            }
                            Classification::Function(_) | Classification::Component => {
                                // Callee is a function or component — component rule holds.
                                // Continue checking remaining stmts.
                            }
                        }
                    }
                    // else: callee not in all_decls → import/builtin → treat as function
                    // → component rule holds; continue.
                }
                // else: non-simple callee (member access, etc.) → treat as function call
                // → component rule holds; continue.
            }
        }

        // All callees resolved to functions/macros → component.
        return Classification::Component;
    }

    // ── Rule 5 / 6: mixed or non-call Expr → function ────────────────────────
    Classification::Function(bare_to_function(decl))
}

/// Returns true if `stmt` is one of the statement kinds that unambiguously
/// marks the enclosing declaration as a function.
fn is_function_statement(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Let(_)
            | Stmt::Var(_)
            | Stmt::Return(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::If(_)
            | Stmt::While(_)
            | Stmt::Loop(_)
            | Stmt::For(_)
            | Stmt::Match(_)
            | Stmt::Function(_)
            | Stmt::Struct(_)
            | Stmt::State(_)
            | Stmt::Assign(_)
            | Stmt::Decl(_)
    )
}

/// Returns true if `stmt` is a call expression statement `Stmt::Expr(Expr::Call(_))`.
fn is_call_expr(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Expr(Expr::Call(_)))
}

/// Produce a [`FunctionDecl`] from a [`BareDecl`].
fn bare_to_function(decl: &BareDecl) -> FunctionDecl {
    FunctionDecl {
        name: decl.name.clone(),
        generic_params: Vec::new(),
        params: decl.params.clone().unwrap_or_default(),
        return_ty: decl.return_ty.clone(),
        body: FunctionBody::Block(decl.body.clone()),
        span: decl.span.clone(),
    }
}

/// Produce a [`StructDecl`] from a [`BareDecl`] by extracting `BareField` stmts.
fn bare_to_struct(decl: &BareDecl) -> StructDecl {
    let fields: Vec<FieldDecl> = decl
        .body
        .stmts
        .iter()
        .filter_map(|s| {
            if let Stmt::BareField(fd) = s {
                Some(fd.clone())
            } else {
                None
            }
        })
        .collect();

    StructDecl {
        name: decl.name.clone(),
        generic_params: Vec::new(),
        fields,
        span: decl.span.clone(),
    }
}


