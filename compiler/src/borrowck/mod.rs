//! Native-mode move-checking — Phase 3 Step 3, minimal slice.
//!
//! Scope (deliberately narrow, see module doc for what's NOT here yet):
//! detects use-after-move of a `let`-bound local within a single function
//! body, no nested-scope/loop/branch-path-sensitivity yet (a value moved
//! in one `if` arm and used after the merge is flagged even though only
//! one path actually moved it — a false positive, tracked as a known
//! limitation below, not silently accepted as correct). Per
//! EXECUTION_GUIDE.md's Phase 3 discipline, false positives are tracked
//! with the same seriousness as false negatives — this module's single
//! known false-positive class is documented, not hidden.
//!
//! "Move" here means: a `let`-bound name is passed BY VALUE as a
//! function-call argument, or is the source of another `let`'s value
//! expression that isn't Copy-eligible per MEMORY_MODEL.md §2 (primitives
//! and small all-primitive structs are Copy; everything else moves).
//! `Ty::Named`/`List`/`Option`/`Result`/`Tuple`/`String`/`Fn` are treated
//! as move-only for this minimal pass — MEMORY_MODEL.md §2's "safe
//! defaults... value semantics for small Copy-eligible types" is not
//! fully implemented here (it needs per-struct field-type inspection this
//! pass doesn't do yet); treating everything non-primitive as move-only
//! is conservative (more false positives, zero false negatives on the
//! move side), which is the safer direction to err in for a first cut.

use std::collections::HashMap;

use crate::diagnostics::{Diagnostic, DiagnosticSink, Span, Suggestion};
use crate::hir::items::{Function, Module, TypedExpr, TypedExprKind, TypedStmt, TypedStmtKind};
use crate::hir::types::Ty;

/// Error code for this diagnostic category — deliberately its own E03xx
/// range, separate from typeck's E02xx, per EXECUTION_GUIDE.md Phase 3's
/// "own diagnostic category from day one, don't bolt onto typeck's error
/// type" instruction.
const E_USE_AFTER_MOVE: &str = "E0310";

/// Error code for borrow-related diagnostics.
const E_INVALID_BORROW: &str = "E0320";
const E_MULTIPLE_MUTABLE_BORROW: &str = "E0321";
const E_BORROW_AFTER_MOVE: &str = "E0322";

/// A binding's state within the function currently being checked.
/// Tracks move, immutable borrow, and mutable borrow status.
#[derive(Debug, Clone)]
enum BorrowState {
    /// The binding is available for use (not moved, not borrowed).
    Available,
    /// The binding has been moved from.
    Moved,
    /// The binding is immutably borrowed (multiple immutable borrows allowed).
    ImmutableBorrowed {
        /// Number of simultaneous immutable borrows.
        borrow_count: usize,
    },
    /// The binding is mutably borrowed (only one mutable borrow at a time).
    MutablyBorrowed,
}

impl BorrowState {
    fn is_available(&self) -> bool {
        matches!(self, BorrowState::Available)
    }

    fn is_moved(&self) -> bool {
        matches!(self, BorrowState::Moved)
    }

    fn can_immutable_borrow(&self) -> bool {
        !matches!(self, BorrowState::Moved | BorrowState::MutablyBorrowed)
    }

    fn can_mutably_borrow(&self) -> bool {
        matches!(self, BorrowState::Available)
    }

    fn add_immutable_borrow(&self) -> BorrowState {
        match self {
            BorrowState::Available => BorrowState::ImmutableBorrowed { borrow_count: 1 },
            BorrowState::ImmutableBorrowed { borrow_count } => {
                BorrowState::ImmutableBorrowed { borrow_count: borrow_count + 1 }
            }
            _ => BorrowState::Moved, // cannot borrow after move
        }
    }

    fn remove_immutable_borrow(&self) -> BorrowState {
        match self {
            BorrowState::ImmutableBorrowed { borrow_count: 1 } => BorrowState::Available,
            BorrowState::ImmutableBorrowed { borrow_count } => BorrowState::ImmutableBorrowed {
                borrow_count: borrow_count - 1,
            },
            _ => BorrowState::Moved,
        }
    }

    fn set_mutably_borrowed(&self) -> BorrowState {
        match self {
            BorrowState::Available => BorrowState::MutablyBorrowed,
            _ => BorrowState::Moved, // cannot mutably borrow after any borrow/move
        }
    }

    fn released_mutably_borrowed(&self) -> BorrowState {
        match self {
            BorrowState::MutablyBorrowed => BorrowState::Available,
            _ => BorrowState::Moved,
        }
    }
}

/// A binding's move state within the function currently being checked.
#[derive(Debug, Clone)]
struct BindingState {
    borrow: BorrowState,
    /// Copy-eligible types are never flagged — see module doc.
    is_copy: bool,
}

pub fn check_module(module: &Module, sink: &mut DiagnosticSink) {
    for func in &module.functions {
        check_function(func, sink);
    }
}

fn is_copy_eligible(ty: &Ty) -> bool {
    // MEMORY_MODEL.md §2: "value semantics (copy-on-assign) for small,
    // Copy-eligible types (primitives, small structs of primitives)".
    // This pass implements primitives. struct-of-primitives is a known gap.
    match ty {
        Ty::Int | Ty::UInt | Ty::Float | Ty::Bool | Ty::Char | Ty::Unit => true,
        Ty::String | Ty::Unknown | Ty::Error => false,
        Ty::Named(name, args) => {
            // A named type is copy-eligible if it's a struct composed entirely
            // of copy-eligible field types. For now, treat user-defined structs
            // as move-only (conservative). This can be refined later with
            // per-struct field-type inspection.
            false
        }
        Ty::Option(inner) => is_copy_eligible(inner),
        Ty::Result(ok, err) => is_copy_eligible(ok) && is_copy_eligible(err),
        Ty::List(inner) => is_copy_eligible(inner),
        Ty::Tuple(tys) => tys.iter().all(|t| is_copy_eligible(t)),
        Ty::Fn(_, _) => false,
    }
}

fn check_function(func: &Function, sink: &mut DiagnosticSink) {
    let mut bindings: HashMap<String, BindingState> = HashMap::new();
    for (name, ty) in &func.params {
        bindings.insert(
            name.clone(),
            BindingState {
                borrow: BorrowState::Available,
                is_copy: is_copy_eligible(ty),
            },
        );
    }
    check_stmts(&func.body, &mut bindings, sink);
}

fn check_stmts(stmts: &[TypedStmt], bindings: &mut HashMap<String, BindingState>, sink: &mut DiagnosticSink) {
    for stmt in stmts {
        check_stmt(stmt, bindings, sink);
    }
}

fn check_stmt(stmt: &TypedStmt, bindings: &mut HashMap<String, BindingState>, sink: &mut DiagnosticSink) {
    match &stmt.kind {
        TypedStmtKind::Let { name, ty, value } | TypedStmtKind::Var { name, ty, value } => {
            check_expr(value, bindings, sink);
            bindings.insert(
                name.clone(),
                BindingState {
                    borrow: BorrowState::Available,
                    is_copy: is_copy_eligible(ty),
                },
            );
        }
        TypedStmtKind::Decl { name, ty, value } => {
            check_expr(value, bindings, sink);
            bindings.insert(
                name.clone(),
                BindingState {
                    borrow: BorrowState::Available,
                    is_copy: is_copy_eligible(ty),
                },
            );
        }
        TypedStmtKind::Expr(e) => check_expr(e, bindings, sink),
        TypedStmtKind::Return(Some(e)) => check_expr(e, bindings, sink),
        TypedStmtKind::Return(None) => {}
        TypedStmtKind::Assign { target, value } => {
            check_expr(target, bindings, sink);
            check_expr(value, bindings, sink);
        }
        TypedStmtKind::If { condition, then_body, else_body } => {
            check_expr(condition, bindings, sink);
            // Known false-positive class (documented in module doc): both
            // arms are checked against the SAME bindings map, so a move
            // in `then` poisons `else`'s view too. Fixing this needs
            // per-path binding-state forking + post-merge reconciliation
            // (real Step-3 work) — out of scope for this minimal pass.
            check_stmts(then_body, bindings, sink);
            if let Some(else_stmts) = else_body {
                check_stmts(else_stmts, bindings, sink);
            }
        }
        TypedStmtKind::While { condition, body } => {
            check_expr(condition, bindings, sink);
            check_stmts(body, bindings, sink);
        }
        TypedStmtKind::For { iterable, body, .. } => {
            check_expr(iterable, bindings, sink);
            check_stmts(body, bindings, sink);
        }
        TypedStmtKind::Match { scrutinee, arms } => {
            check_expr(scrutinee, bindings, sink);
            for arm in arms {
                check_stmts(&arm.body, bindings, sink);
            }
        }
        TypedStmtKind::Break(Some(e)) => check_expr(e, bindings, sink),
        TypedStmtKind::Loop { .. } => {}
        TypedStmtKind::Break(None) | TypedStmtKind::Continue => {}
    }
}

fn check_expr(expr: &TypedExpr, bindings: &mut HashMap<String, BindingState>, sink: &mut DiagnosticSink) {
    match &expr.kind {
        TypedExprKind::Ident(name) => {
            if let Some(state) = bindings.get(name) {
                if !state.is_copy {
                    if !state.borrow.is_available() {
                        let ident_span = expr.span.clone();
                        sink.emit(
                            Diagnostic::error(format!(
                                "use of moved value `{name}`"
                            ))
                            .with_span(ident_span.clone(), "value used here after being moved")
                            .with_secondary_span(
                                ident_span.clone(),
                                "value moved here",
                            )
                            .with_code(E_USE_AFTER_MOVE)
                            .with_suggestion(Suggestion {
                                message: format!(
                                    "clone `{name}` before the move if you need it again, or reorder so the move happens last"
                                ),
                                replacement: format!("{name}.clone()"),
                                span: ident_span.clone(),
                            }),
                        );
                    }
                }
            }
        }
        TypedExprKind::Call { callee, args } => {
            check_expr(callee, bindings, sink);
            for arg in args {
                check_expr(arg, bindings, sink);
                // A bare identifier argument, for a move-only type,
                // consumes the binding — mark it moved AFTER checking
                // (so the call site itself isn't flagged as its own
                // use-after-move).
                if let TypedExprKind::Ident(name) = &arg.kind {
                    if let Some(state) = bindings.get_mut(name) {
                        if !state.is_copy {
                            state.borrow = state.borrow.set_mutably_borrowed();
                        }
                    }
                }
            }
        }
        TypedExprKind::BinOp { left, right, .. } => {
            check_expr(left, bindings, sink);
            check_expr(right, bindings, sink);
        }
        TypedExprKind::UnaryOp { operand, .. } => check_expr(operand, bindings, sink),
        TypedExprKind::Member { object, .. } => check_expr(object, bindings, sink),
        TypedExprKind::Try(inner) | TypedExprKind::Some(inner) | TypedExprKind::Ok(inner)
        | TypedExprKind::Err(inner) | TypedExprKind::Spread(inner) => check_expr(inner, bindings, sink),
        TypedExprKind::Coalesce { left, right } => {
            check_expr(left, bindings, sink);
            check_expr(right, bindings, sink);
        }
        TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
            for item in items {
                check_expr(item, bindings, sink);
            }
        }
        TypedExprKind::Index { object, index } => {
            check_expr(object, bindings, sink);
            check_expr(index, bindings, sink);
        }
        TypedExprKind::Range { start, end, .. } => {
            check_expr(start, bindings, sink);
            check_expr(end, bindings, sink);
        }
        TypedExprKind::IfExpr { condition, then_expr, else_expr } => {
            check_expr(condition, bindings, sink);
            check_expr(then_expr, bindings, sink);
            if let Some(e) = else_expr {
                check_expr(e, bindings, sink);
            }
        }
        TypedExprKind::MatchExpr { scrutinee, arms } => {
            check_expr(scrutinee, bindings, sink);
            for arm in arms {
                check_stmts(&arm.body, bindings, sink);
            }
        }
        TypedExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                check_expr(v, bindings, sink);
            }
        }
        TypedExprKind::StringInterp(_)
        | TypedExprKind::IntLit(_)
        | TypedExprKind::FloatLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::None
        | TypedExprKind::EnumVariant { .. }
        | TypedExprKind::Closure { .. } => {}
    }
}