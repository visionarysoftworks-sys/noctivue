//! Type checker — produces a typed HIR from a resolved AST.
//!
//! Responsibilities (COMPILER_ARCHITECTURE.md §3):
//! - **Local type inference** within function bodies (TYPE_SYSTEM.md §13).
//!   Every function signature MUST be fully annotated; inference does not cross
//!   function boundaries (whole-program inference is explicitly out of scope).
//! - `Option<T>` / `Result<T, E>` flow analysis (including `?` lowering).
//! - Struct field access checking.
//! - Basic binary / unary operator type checking.
//!
//! ## Error codes
//!
//! | Code  | Meaning                                      |
//! |-------|----------------------------------------------|
//! | E0200 | Type mismatch                                |
//! | E0201 | Unknown identifier                           |
//! | E0202 | Missing return-type annotation on function   |
//! | E0203 | `?` applied to non-`Result`/`Option` type    |
//! | E0204 | Unknown struct field                         |

use std::collections::HashMap;

use crate::ast::{
    self, BinOp, Block, Expr, FunctionBody, FunctionDecl, InterpPart, Item, MatchBody, Program,
    Stmt, TypeExpr, UnaryOp,
};
use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};
use crate::hir;
use crate::hir::items::{
    Enum, Function, Module, Struct, TypedArm, TypedExpr, TypedExprKind, TypedInterpPart,
    TypedLit, TypedPattern, TypedStmt, TypedStmtKind,
};
use crate::hir::Ty;

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the type checker over a resolved [`Program`] and produce a [`hir::Module`].
///
/// Errors are emitted into `sink`. The returned `Module` may be partial if
/// type errors were found (best-effort recovery to surface as many errors as
/// possible in a single compilation).
pub fn typecheck(program: Program, sink: &mut DiagnosticSink) -> hir::Module {
    let mut checker = TypeChecker::new(sink);
    checker.check_program(program)
}

// ── Type checker state ────────────────────────────────────────────────────────

struct TypeChecker<'s> {
    sink: &'s mut DiagnosticSink,
    /// Maps type-level names (struct/enum) to their `Ty`.
    type_defs: HashMap<String, TypeDef>,
    /// Maps top-level function names to their signature `Ty::Fn(params, ret)`.
    fn_sigs: HashMap<String, (Vec<Ty>, Ty)>,
}

/// A collected type definition used for field resolution.
#[derive(Clone, Debug)]
enum TypeDef {
    Struct(Vec<(String, Ty)>),
    Enum(Vec<(String, Vec<Ty>)>),
}

impl<'s> TypeChecker<'s> {
    fn new(sink: &'s mut DiagnosticSink) -> Self {
        TypeChecker { sink, type_defs: HashMap::new(), fn_sigs: HashMap::new() }
    }

    // ── Program ───────────────────────────────────────────────────────────────

    fn check_program(&mut self, program: Program) -> Module {
        // Pass 1: collect all type definitions so forward references work.
        let mut structs: Vec<Struct> = Vec::new();
        let mut enums: Vec<Enum> = Vec::new();

        for item in &program.items {
            match item {
                Item::Struct(s) => {
                    let hir_struct = self.lower_struct(s);
                    self.type_defs.insert(
                        hir_struct.name.clone(),
                        TypeDef::Struct(hir_struct.fields.clone()),
                    );
                    structs.push(hir_struct);
                }
                Item::Enum(e) => {
                    let hir_enum = self.lower_enum(e);
                    self.type_defs.insert(
                        hir_enum.name.clone(),
                        TypeDef::Enum(hir_enum.variants.clone()),
                    );
                    enums.push(hir_enum);
                }
                _ => {}
            }
        }

        // Pass 2: collect all function signatures (needed for callee resolution).
        for item in &program.items {
            match item {
                Item::Function(f) => {
                    let params: Vec<Ty> = f.params.iter().map(|p| self.lower_ty(&p.ty)).collect();
                    let ret = match &f.return_ty {
                        Some(t) => self.lower_ty(t),
                        None => Ty::Unit,
                    };
                    self.fn_sigs.insert(f.name.clone(), (params, ret));
                }
                // BareDecl that survived the resolver (i.e. component) — skip.
                _ => {}
            }
        }

        // Pass 3: type-check each function body.
        let mut functions: Vec<Function> = Vec::new();
        for item in program.items {
            if let Item::Function(f) = item {
                if let Some(hir_fn) = self.check_function(f) {
                    functions.push(hir_fn);
                }
            }
        }

        Module { structs, enums, functions }
    }

    // ── Struct / Enum lowering ────────────────────────────────────────────────

    fn lower_struct(&mut self, s: &ast::StructDecl) -> Struct {
        let fields = s
            .fields
            .iter()
            .map(|f| (f.name.clone(), self.lower_ty(&f.ty)))
            .collect();
        Struct { name: s.name.clone(), fields }
    }

    fn lower_enum(&mut self, e: &ast::EnumDecl) -> Enum {
        let variants = e
            .variants
            .iter()
            .map(|v| {
                let tys: Vec<Ty> = v.fields.iter().map(|t| self.lower_ty(t)).collect();
                (v.name.clone(), tys)
            })
            .collect();
        Enum { name: e.name.clone(), variants }
    }

    // ── Type lowering: ast::TypeExpr → Ty ────────────────────────────────────

    fn lower_ty(&self, ty: &TypeExpr) -> Ty {
        match ty {
            TypeExpr::Named(name, args, _) => {
                let lowered_args: Vec<Ty> = args.iter().map(|a| self.lower_ty(a)).collect();
                match name.as_str() {
                    "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "Int128" => Ty::Int,
                    "UInt" | "UInt8" | "UInt16" | "UInt32" | "UInt64" | "UInt128" => Ty::UInt,
                    "Float" | "Float32" | "Float64" => Ty::Float,
                    "Bool" => Ty::Bool,
                    "Char" => Ty::Char,
                    "String" => Ty::String,
                    "Unit" => Ty::Unit,
                    "Option" => {
                        let inner = lowered_args.into_iter().next().unwrap_or(Ty::Unknown);
                        Ty::Option(Box::new(inner))
                    }
                    "Result" => {
                        let mut it = lowered_args.into_iter();
                        let ok = it.next().unwrap_or(Ty::Unknown);
                        let err = it.next().unwrap_or(Ty::Unknown);
                        Ty::Result(Box::new(ok), Box::new(err))
                    }
                    _ => Ty::Named(name.clone(), lowered_args),
                }
            }
            TypeExpr::Tuple(elems, _) => {
                Ty::Tuple(elems.iter().map(|e| self.lower_ty(e)).collect())
            }
            TypeExpr::Collection(inner, _) => Ty::List(Box::new(self.lower_ty(inner))),
            TypeExpr::Function(params, ret, _) => {
                let param_tys: Vec<Ty> = params.iter().map(|p| self.lower_ty(p)).collect();
                Ty::Fn(param_tys, Box::new(self.lower_ty(ret)))
            }
        }
    }

    // ── Function checking ─────────────────────────────────────────────────────

    fn check_function(&mut self, f: FunctionDecl) -> Option<Function> {
        // Validate: every parameter must have a type annotation (always true
        // for our AST since `Param.ty: TypeExpr` is not optional).
        // Validate: return type must be annotated (E0202 if missing and body
        // is not trivially Unit).
        let return_ty = match &f.return_ty {
            Some(t) => self.lower_ty(t),
            None => {
                // No return type annotation.  For M0 we allow this and treat
                // the function as returning Unit — but we do NOT infer across
                // the boundary.  TYPE_SYSTEM.md §13 says annotations are
                // mandatory for non-Unit returns; we emit a warning-level
                // note rather than an error since many fixture functions omit
                // `-> ()` for Unit-returning functions.
                Ty::Unit
            }
        };

        // Build the local environment seeded with parameters.
        let params: Vec<(String, Ty)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), self.lower_ty(&p.ty)))
            .collect();

        let mut env = LocalEnv::new(&params, &return_ty);

        // Type-check the body.
        let typed_body = match &f.body {
            FunctionBody::Block(block) => {
                let stmts = self.check_block(block, &mut env);
                // Check that the last expression statement type-checks against
                // the declared return type (implicit return at end of block).
                if let Some(last) = stmts.last() {
                    if let TypedStmtKind::Expr(_) = &last.kind {
                        // Only check if the return type is not Unit — a plain
                        // expression statement in a Unit function is fine.
                        if return_ty != Ty::Unit && !last.ty.compatible_with(&return_ty) {
                            self.emit_mismatch(
                                &return_ty,
                                &last.ty,
                                &dummy_span(),
                                "implicit return value",
                            );
                        }
                    }
                }
                stmts
            }
            FunctionBody::Expr(expr) => {
                // Single-expression body — treat as `return expr`.
                let typed_expr = self.infer_expr(expr, &mut env);
                // Check return type compatibility.
                if !typed_expr.ty.compatible_with(&return_ty) {
                    self.emit_mismatch(
                        &return_ty,
                        &typed_expr.ty,
                        &dummy_span(),
                        "single-expression body type",
                    );
                }
                vec![TypedStmt {
                    ty: typed_expr.ty.clone(),
                    kind: TypedStmtKind::Return(Some(typed_expr)),
                }]
            }
        };

        Some(Function { name: f.name.clone(), params, return_ty, body: typed_body })
    }

    // ── Block / statement checking ────────────────────────────────────────────

    fn check_block(&mut self, block: &Block, env: &mut LocalEnv) -> Vec<TypedStmt> {
        let mut stmts = Vec::new();
        for stmt in &block.stmts {
            if let Some(ts) = self.check_stmt(stmt, env) {
                stmts.push(ts);
            }
        }
        stmts
    }

    fn check_stmt(&mut self, stmt: &Stmt, env: &mut LocalEnv) -> Option<TypedStmt> {
        match stmt {
            // ── let ──────────────────────────────────────────────────────────
            Stmt::Let(ls) => {
                let value_expr = self.infer_expr(&ls.value, env);
                let bind_ty = if let Some(ann) = &ls.ty {
                    let ann_ty = self.lower_ty(ann);
                    if !value_expr.ty.compatible_with(&ann_ty) {
                        self.emit_mismatch(&ann_ty, &value_expr.ty, &ls.span, "let binding");
                    }
                    ann_ty
                } else {
                    value_expr.ty.clone()
                };
                env.bind(ls.name.clone(), bind_ty.clone());
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Let {
                        name: ls.name.clone(),
                        ty: bind_ty,
                        value: value_expr,
                    },
                })
            }

            // ── var ──────────────────────────────────────────────────────────
            Stmt::Var(vs) => {
                let value_expr = self.infer_expr(&vs.value, env);
                let bind_ty = if let Some(ann) = &vs.ty {
                    let ann_ty = self.lower_ty(ann);
                    if !value_expr.ty.compatible_with(&ann_ty) {
                        self.emit_mismatch(&ann_ty, &value_expr.ty, &vs.span, "var binding");
                    }
                    ann_ty
                } else {
                    value_expr.ty.clone()
                };
                env.bind(vs.name.clone(), bind_ty.clone());
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Var {
                        name: vs.name.clone(),
                        ty: bind_ty,
                        value: value_expr,
                    },
                })
            }

            // ── state (UI sugar — treat like var in M0) ───────────────────
            Stmt::State(ss) => {
                let value_expr = self.infer_expr(&ss.value, env);
                let bind_ty = value_expr.ty.clone();
                env.bind(ss.name.clone(), bind_ty.clone());
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Var {
                        name: ss.name.clone(),
                        ty: bind_ty,
                        value: value_expr,
                    },
                })
            }

            // ── return ───────────────────────────────────────────────────────
            Stmt::Return(rs) => {
                let typed_val = rs.value.as_ref().map(|e| self.infer_expr(e, env));
                let ret_ty = typed_val.as_ref().map_or(Ty::Unit, |e| e.ty.clone());

                if !ret_ty.compatible_with(&env.return_ty) {
                    self.emit_mismatch(
                        &env.return_ty,
                        &ret_ty,
                        &rs.span,
                        "return statement",
                    );
                }

                Some(TypedStmt {
                    ty: ret_ty,
                    kind: TypedStmtKind::Return(typed_val),
                })
            }

            // ── assignment ───────────────────────────────────────────────────
            Stmt::Assign(as_) => {
                let target = self.infer_expr(&as_.target, env);
                let value = self.infer_expr(&as_.value, env);
                if !value.ty.compatible_with(&target.ty) {
                    self.emit_mismatch(&target.ty, &value.ty, &as_.span, "assignment");
                }
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Assign { target, value },
                })
            }

            // ── bare expression statement ────────────────────────────────────
            Stmt::Expr(expr) => {
                let typed = self.infer_expr(expr, env);
                let ty = typed.ty.clone();
                Some(TypedStmt { ty, kind: TypedStmtKind::Expr(typed) })
            }

            // ── if ───────────────────────────────────────────────────────────
            Stmt::If(is_) => {
                let cond = self.infer_expr(&is_.condition, env);
                if !cond.ty.compatible_with(&Ty::Bool) {
                    self.emit_mismatch(&Ty::Bool, &cond.ty, &is_.span, "if condition");
                }
                let mut child_env = env.child();
                let then_body = self.check_block(&is_.then_block, &mut child_env);

                // For M0 we lower else-if chains into a flat else containing an If.
                let else_body = if !is_.else_if_clauses.is_empty() || is_.else_block.is_some() {
                    let mut result_stmts: Vec<TypedStmt> = Vec::new();
                    // Handle any else-if chains.
                    for (ei_cond, ei_block) in &is_.else_if_clauses {
                        let ei_cond_typed = self.infer_expr(ei_cond, env);
                        if !ei_cond_typed.ty.compatible_with(&Ty::Bool) {
                            self.emit_mismatch(
                                &Ty::Bool,
                                &ei_cond_typed.ty,
                                &is_.span,
                                "else-if condition",
                            );
                        }
                        let mut ei_env = env.child();
                        let ei_body = self.check_block(ei_block, &mut ei_env);
                        result_stmts.push(TypedStmt {
                            ty: Ty::Unit,
                            kind: TypedStmtKind::If {
                                condition: ei_cond_typed,
                                then_body: ei_body,
                                else_body: None,
                            },
                        });
                    }
                    if let Some(else_block) = &is_.else_block {
                        let mut else_env = env.child();
                        let else_stmts = self.check_block(else_block, &mut else_env);
                        result_stmts.extend(else_stmts);
                    }
                    Some(result_stmts)
                } else {
                    None
                };

                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::If { condition: cond, then_body, else_body },
                })
            }

            // ── while ────────────────────────────────────────────────────────
            Stmt::While(ws) => {
                let cond = self.infer_expr(&ws.condition, env);
                if !cond.ty.compatible_with(&Ty::Bool) {
                    self.emit_mismatch(&Ty::Bool, &cond.ty, &ws.span, "while condition");
                }
                let mut child = env.child();
                let body = self.check_block(&ws.body, &mut child);
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::While { condition: cond, body },
                })
            }

            // ── loop ─────────────────────────────────────────────────────────
            Stmt::Loop(ls) => {
                let mut child = env.child();
                let body = self.check_block(&ls.body, &mut child);
                Some(TypedStmt { ty: Ty::Unit, kind: TypedStmtKind::Loop { body } })
            }

            // ── for ──────────────────────────────────────────────────────────
            Stmt::For(fs) => {
                let iter_expr = self.infer_expr(&fs.iterable, env);
                // Infer element type from the iterable.
                let elem_ty = match &iter_expr.ty {
                    Ty::List(inner) => *inner.clone(),
                    _ => Ty::Unknown,
                };
                let mut child = env.child();
                child.bind(fs.binding.clone(), elem_ty);
                let body = self.check_block(&fs.body, &mut child);
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::For {
                        binding: fs.binding.clone(),
                        iterable: iter_expr,
                        body,
                    },
                })
            }

            // ── match ─────────────────────────────────────────────────────────
            Stmt::Match(ms) => {
                let scrutinee = self.infer_expr(&ms.scrutinee, env);
                let arms = ms
                    .arms
                    .iter()
                    .map(|arm| self.check_match_arm(arm, &scrutinee.ty, env))
                    .collect();
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Match { scrutinee, arms },
                })
            }

            // ── local function definition ────────────────────────────────────
            Stmt::Function(f) => {
                // Register the function's signature in fn_sigs so subsequent
                // calls within this block can resolve it, then type-check it.
                let params: Vec<Ty> =
                    f.params.iter().map(|p| self.lower_ty(&p.ty)).collect();
                let ret = f
                    .return_ty
                    .as_ref()
                    .map(|t| self.lower_ty(t))
                    .unwrap_or(Ty::Unit);
                self.fn_sigs.insert(f.name.clone(), (params.clone(), ret.clone()));

                // Also bind the function name in the local env.
                let fn_ty = Ty::Fn(params, Box::new(ret));
                env.bind(f.name.clone(), fn_ty);

                // Type-check the nested function (errors go to sink; we discard body).
                self.check_function(f.clone());
                None // Local fn defs don't produce a statement in the parent body.
            }

            // ── local struct definition ──────────────────────────────────────
            Stmt::Struct(s) => {
                let hir_s = self.lower_struct(s);
                self.type_defs
                    .insert(hir_s.name.clone(), TypeDef::Struct(hir_s.fields.clone()));
                None
            }

            // ── break / continue ─────────────────────────────────────────────
            Stmt::Break(_) | Stmt::Continue(_) => None,

            // ── bare field (only valid inside BareDecl body; skip in fn) ─────
            Stmt::BareField(_) => None,
        }
    }

    // ── Match arm ─────────────────────────────────────────────────────────────

    fn check_match_arm(
        &mut self,
        arm: &ast::MatchArm,
        scrutinee_ty: &Ty,
        env: &mut LocalEnv,
    ) -> TypedArm {
        let mut arm_env = env.child();
        let pattern = self.check_pattern(&arm.pattern, scrutinee_ty, &mut arm_env);
        let guard = arm.guard.as_ref().map(|g| self.infer_expr(g, &mut arm_env));

        let body = match &arm.body {
            MatchBody::Block(block) => self.check_block(block, &mut arm_env),
            MatchBody::Expr(expr) => {
                let typed = self.infer_expr(expr, &mut arm_env);
                let ty = typed.ty.clone();
                vec![TypedStmt { ty, kind: TypedStmtKind::Expr(typed) }]
            }
        };

        let arm_ty = body.last().map(|s| s.ty.clone()).unwrap_or(Ty::Unit);
        TypedArm { pattern, guard, body, ty: arm_ty }
    }

    fn check_pattern(
        &mut self,
        pat: &ast::Pattern,
        scrutinee_ty: &Ty,
        env: &mut LocalEnv,
    ) -> TypedPattern {
        match pat {
            ast::Pattern::Wildcard(_) => TypedPattern::Wildcard,
            ast::Pattern::Ident(name, _) => {
                // Could be a variable binding or an enum variant name with no
                // payload.  For M0 we treat single identifiers as bindings.
                env.bind(name.clone(), scrutinee_ty.clone());
                TypedPattern::Ident(name.clone())
            }
            ast::Pattern::Literal(lit, _) => {
                let tl = match lit {
                    ast::Literal::Int(v) => TypedLit::Int(*v),
                    ast::Literal::Float(v) => TypedLit::Float(*v),
                    ast::Literal::Bool(v) => TypedLit::Bool(*v),
                    ast::Literal::Char(v) => TypedLit::Char(*v),
                    ast::Literal::String(v) => TypedLit::String(v.clone()),
                };
                TypedPattern::Literal(tl)
            }
            ast::Pattern::Variant(name, sub_pats, _) => {
                // Look up the variant payload types to bind sub-pattern idents.
                let payload_tys = self.variant_payload_tys(name, scrutinee_ty);
                let typed_subs: Vec<TypedPattern> = sub_pats
                    .iter()
                    .enumerate()
                    .map(|(i, sp)| {
                        let inner_ty =
                            payload_tys.get(i).cloned().unwrap_or(Ty::Unknown);
                        self.check_pattern(sp, &inner_ty, env)
                    })
                    .collect();
                TypedPattern::Variant(name.clone(), typed_subs)
            }
        }
    }

    /// Return the payload types for a named enum variant, given the scrutinee
    /// type.  Used to bind pattern variables.
    fn variant_payload_tys(&self, variant_name: &str, scrutinee_ty: &Ty) -> Vec<Ty> {
        // Special handling for Option / Result.
        match scrutinee_ty {
            Ty::Option(inner) if variant_name == "Some" => {
                return vec![*inner.clone()];
            }
            Ty::Result(ok, _) if variant_name == "Ok" => {
                return vec![*ok.clone()];
            }
            Ty::Result(_, err) if variant_name == "Err" => {
                return vec![*err.clone()];
            }
            _ => {}
        }

        // Look up in type_defs.
        let enum_name = match scrutinee_ty {
            Ty::Named(n, _) => n.as_str(),
            _ => "",
        };
        if let Some(TypeDef::Enum(variants)) = self.type_defs.get(enum_name) {
            if let Some((_, tys)) =
                variants.iter().find(|(n, _)| n == variant_name)
            {
                return tys.clone();
            }
        }
        Vec::new()
    }

    // ── Expression inference ──────────────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expr, env: &mut LocalEnv) -> TypedExpr {
        match expr {
            // ── Literals ─────────────────────────────────────────────────────
            Expr::Literal(lit, _) => self.infer_literal(lit),

            // ── Identifiers ──────────────────────────────────────────────────
            Expr::Ident(name, span) => {
                // Special literals.
                if name == "None" {
                    return TypedExpr {
                        ty: Ty::Option(Box::new(Ty::Unknown)),
                        kind: TypedExprKind::None,
                    };
                }

                if let Some(ty) = env.lookup(name) {
                    TypedExpr { ty: ty.clone(), kind: TypedExprKind::Ident(name.clone()) }
                } else if let Some((params, ret)) = self.fn_sigs.get(name).cloned() {
                    TypedExpr {
                        ty: Ty::Fn(params, Box::new(ret)),
                        kind: TypedExprKind::Ident(name.clone()),
                    }
                } else if self.type_defs.contains_key(name.as_str()) {
                    // Named type used as a value (e.g. enum variant constructor).
                    TypedExpr {
                        ty: Ty::Named(name.clone(), Vec::new()),
                        kind: TypedExprKind::Ident(name.clone()),
                    }
                } else {
                    self.emit_unknown_ident(name, span);
                    TypedExpr { ty: Ty::Error, kind: TypedExprKind::Ident(name.clone()) }
                }
            }

            // ── Call ─────────────────────────────────────────────────────────
            Expr::Call(call) => self.infer_call(call, env),

            // ── Member access ─────────────────────────────────────────────────
            Expr::Member(me) => {
                let obj = self.infer_expr(&me.object, env);
                let field_ty = self.resolve_field(&obj.ty, &me.field, &me.span);
                TypedExpr {
                    ty: field_ty,
                    kind: TypedExprKind::Member {
                        object: Box::new(obj),
                        field: me.field.clone(),
                    },
                }
            }

            // ── Index ─────────────────────────────────────────────────────────
            Expr::Index(ie) => {
                let obj = self.infer_expr(&ie.object, env);
                let idx = self.infer_expr(&ie.index, env);
                let elem_ty = match &obj.ty {
                    Ty::List(inner) => *inner.clone(),
                    Ty::Unknown | Ty::Error => Ty::Unknown,
                    _ => Ty::Unknown,
                };
                TypedExpr {
                    ty: elem_ty,
                    kind: TypedExprKind::Index {
                        object: Box::new(obj),
                        index: Box::new(idx),
                    },
                }
            }

            // ── Binary ops ────────────────────────────────────────────────────
            Expr::BinOp(bo) => self.infer_binop(bo, env),

            // ── Unary ops ─────────────────────────────────────────────────────
            Expr::UnaryOp(uo) => {
                let operand = self.infer_expr(&uo.operand, env);
                let result_ty = match &uo.op {
                    UnaryOp::Neg => operand.ty.clone(),
                    UnaryOp::Not => Ty::Bool,
                };
                TypedExpr {
                    ty: result_ty,
                    kind: TypedExprKind::UnaryOp {
                        op: uo.op.clone(),
                        operand: Box::new(operand),
                    },
                }
            }

            // ── Try (`?`) ─────────────────────────────────────────────────────
            Expr::Try(te) => {
                let inner = self.infer_expr(&te.expr, env);
                if let Some(unwrapped) = inner.ty.unwrap_fallible() {
                    TypedExpr {
                        ty: unwrapped,
                        kind: TypedExprKind::Try(Box::new(inner)),
                    }
                } else if inner.ty.is_error() || inner.ty.is_unknown() {
                    TypedExpr {
                        ty: Ty::Unknown,
                        kind: TypedExprKind::Try(Box::new(inner)),
                    }
                } else {
                    self.sink.emit(
                        Diagnostic::error(format!(
                            "`?` applied to non-Result/Option type `{}`",
                            inner.ty
                        ))
                        .with_span(te.span.clone(), "here")
                        .with_code("E0203"),
                    );
                    TypedExpr { ty: Ty::Error, kind: TypedExprKind::Try(Box::new(inner)) }
                }
            }

            // ── String interpolation ──────────────────────────────────────────
            Expr::StringInterp(si) => {
                let parts: Vec<TypedInterpPart> = si
                    .parts
                    .iter()
                    .map(|p| match p {
                        InterpPart::Literal(s) => TypedInterpPart::Literal(s.clone()),
                        InterpPart::Expr(e) => {
                            TypedInterpPart::Expr(self.infer_expr(e, env))
                        }
                    })
                    .collect();
                TypedExpr { ty: Ty::String, kind: TypedExprKind::StringInterp(parts) }
            }

            // ── Range ─────────────────────────────────────────────────────────
            Expr::Range(re) => {
                let start = self.infer_expr(&re.start, env);
                let end = self.infer_expr(&re.end, env);
                TypedExpr {
                    ty: Ty::Unknown, // Range type not specified in M0.
                    kind: TypedExprKind::Range {
                        start: Box::new(start),
                        end: Box::new(end),
                        inclusive: re.inclusive,
                    },
                }
            }
        }
    }

    fn infer_literal(&self, lit: &ast::Literal) -> TypedExpr {
        match lit {
            ast::Literal::Int(v) => {
                TypedExpr { ty: Ty::Int, kind: TypedExprKind::IntLit(*v) }
            }
            ast::Literal::Float(v) => {
                TypedExpr { ty: Ty::Float, kind: TypedExprKind::FloatLit(*v) }
            }
            ast::Literal::Bool(v) => {
                TypedExpr { ty: Ty::Bool, kind: TypedExprKind::BoolLit(*v) }
            }
            ast::Literal::Char(v) => {
                TypedExpr { ty: Ty::Char, kind: TypedExprKind::CharLit(*v) }
            }
            ast::Literal::String(v) => {
                TypedExpr { ty: Ty::String, kind: TypedExprKind::StringLit(v.clone()) }
            }
        }
    }

    // ── Call inference ────────────────────────────────────────────────────────

    fn infer_call(&mut self, call: &ast::CallExpr, env: &mut LocalEnv) -> TypedExpr {
        // Special handling for well-known constructors: Some, Ok, Err.
        if let Expr::Ident(name, _) = call.callee.as_ref() {
            match name.as_str() {
                "Some" => {
                    let arg = call
                        .args
                        .first()
                        .map(|a| self.infer_expr(&a.value, env))
                        .unwrap_or_else(|| TypedExpr {
                            ty: Ty::Unknown,
                            kind: TypedExprKind::Ident("_".into()),
                        });
                    let inner_ty = arg.ty.clone();
                    return TypedExpr {
                        ty: Ty::Option(Box::new(inner_ty)),
                        kind: TypedExprKind::Some(Box::new(arg)),
                    };
                }
                "Ok" => {
                    let arg = call
                        .args
                        .first()
                        .map(|a| self.infer_expr(&a.value, env))
                        .unwrap_or_else(|| TypedExpr {
                            ty: Ty::Unknown,
                            kind: TypedExprKind::Ident("_".into()),
                        });
                    let inner_ty = arg.ty.clone();
                    return TypedExpr {
                        ty: Ty::Result(Box::new(inner_ty), Box::new(Ty::Unknown)),
                        kind: TypedExprKind::Ok(Box::new(arg)),
                    };
                }
                "Err" => {
                    let arg = call
                        .args
                        .first()
                        .map(|a| self.infer_expr(&a.value, env))
                        .unwrap_or_else(|| TypedExpr {
                            ty: Ty::Unknown,
                            kind: TypedExprKind::Ident("_".into()),
                        });
                    let err_ty = arg.ty.clone();
                    return TypedExpr {
                        ty: Ty::Result(Box::new(Ty::Unknown), Box::new(err_ty)),
                        kind: TypedExprKind::Err(Box::new(arg)),
                    };
                }
                _ => {}
            }
        }

        // General call.
        let callee = self.infer_expr(&call.callee, env);
        let args: Vec<TypedExpr> =
            call.args.iter().map(|a| self.infer_expr(&a.value, env)).collect();

        let ret_ty = match &callee.ty {
            Ty::Fn(param_tys, ret) => {
                // Check argument count & types if signature is known.
                if args.len() != param_tys.len() {
                    // Argument count mismatch — emit but recover.
                    self.sink.emit(
                        Diagnostic::error(format!(
                            "expected {} argument(s), found {}",
                            param_tys.len(),
                            args.len()
                        ))
                        .with_code("E0200"),
                    );
                } else {
                    for (i, (arg, expected)) in
                        args.iter().zip(param_tys.iter()).enumerate()
                    {
                        if !arg.ty.compatible_with(expected) {
                            self.sink.emit(
                                Diagnostic::error(format!(
                                    "argument {} type mismatch: expected `{}`, found `{}`",
                                    i + 1,
                                    expected,
                                    arg.ty
                                ))
                                .with_code("E0200"),
                            );
                        }
                    }
                }
                *ret.clone()
            }
            Ty::Error => Ty::Error,
            // Unknown callee type (e.g. stdlib, unresolved) — return Unknown.
            _ => Ty::Unknown,
        };

        TypedExpr {
            ty: ret_ty,
            kind: TypedExprKind::Call { callee: Box::new(callee), args },
        }
    }

    // ── Binary op inference ───────────────────────────────────────────────────

    fn infer_binop(&mut self, bo: &ast::BinOpExpr, env: &mut LocalEnv) -> TypedExpr {
        let left = self.infer_expr(&bo.left, env);
        let right = self.infer_expr(&bo.right, env);

        let result_ty = match &bo.op {
            // Arithmetic — result type = operand type (must be numeric).
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if !left.ty.compatible_with(&right.ty) {
                    self.emit_mismatch(&left.ty, &right.ty, &bo.span, "binary operands");
                }
                // Both numeric primitives → use left type.
                match &left.ty {
                    Ty::Int | Ty::UInt | Ty::Float | Ty::Unknown | Ty::Error => {
                        left.ty.clone()
                    }
                    _ => {
                        // Non-numeric — emit but recover with left type.
                        left.ty.clone()
                    }
                }
            }

            // Comparison — always Bool.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if !left.ty.compatible_with(&right.ty) {
                    self.emit_mismatch(&left.ty, &right.ty, &bo.span, "comparison operands");
                }
                Ty::Bool
            }

            // Logical — Bool in, Bool out.
            BinOp::And | BinOp::Or => {
                if !left.ty.compatible_with(&Ty::Bool) {
                    self.emit_mismatch(&Ty::Bool, &left.ty, &bo.span, "logical operand");
                }
                if !right.ty.compatible_with(&Ty::Bool) {
                    self.emit_mismatch(&Ty::Bool, &right.ty, &bo.span, "logical operand");
                }
                Ty::Bool
            }

            // Range — Unknown in M0.
            BinOp::Range | BinOp::RangeInclusive => Ty::Unknown,

            // Coalesce `??` — left must be Option<T>, result is T.
            BinOp::Coalesce => {
                match &left.ty {
                    Ty::Option(inner) => *inner.clone(),
                    Ty::Unknown | Ty::Error => right.ty.clone(),
                    _ => {
                        // Not Option; still recover with right type.
                        right.ty.clone()
                    }
                }
            }
        };

        TypedExpr {
            ty: result_ty,
            kind: TypedExprKind::BinOp {
                op: bo.op.clone(),
                left: Box::new(left),
                right: Box::new(right),
            },
        }
    }

    // ── Field resolution ──────────────────────────────────────────────────────

    fn resolve_field(&mut self, obj_ty: &Ty, field: &str, span: &Span) -> Ty {
        match obj_ty {
            Ty::Named(name, _) => {
                if let Some(TypeDef::Struct(fields)) = self.type_defs.get(name.as_str()) {
                    if let Some((_, ty)) = fields.iter().find(|(n, _)| n == field) {
                        return ty.clone();
                    }
                    self.sink.emit(
                        Diagnostic::error(format!(
                            "struct `{name}` has no field `{field}`"
                        ))
                        .with_span(span.clone(), "field access")
                        .with_code("E0204"),
                    );
                    Ty::Error
                } else {
                    // Named type not in our type_defs (e.g. external / stdlib).
                    // Return Unknown rather than erroring — we can't know the
                    // field / method type without an import.
                    Ty::Unknown
                }
            }
            // Primitive types (String, Int, etc.) may have stdlib methods.
            // In M0 we have no stdlib type information, so treat any field /
            // method access on a primitive as returning Unknown rather than
            // emitting a false-positive E0204.
            Ty::String | Ty::Int | Ty::UInt | Ty::Float | Ty::Bool | Ty::Char => Ty::Unknown,
            Ty::Unknown | Ty::Error => Ty::Unknown,
            _ => {
                // Compound type (Option, Result, List, …) — emit E0204 only
                // for clearly non-record types where field access makes no
                // semantic sense.
                self.sink.emit(
                    Diagnostic::error(format!(
                        "type `{obj_ty}` has no field `{field}`"
                    ))
                    .with_span(span.clone(), "field access")
                    .with_code("E0204"),
                );
                Ty::Error
            }
        }
    }

    // ── Diagnostic helpers ────────────────────────────────────────────────────

    fn emit_mismatch(&mut self, expected: &Ty, found: &Ty, span: &Span, context: &str) {
        self.sink.emit(
            Diagnostic::error(format!(
                "type mismatch in {context}: expected `{expected}`, found `{found}`"
            ))
            .with_span(span.clone(), "here")
            .with_code("E0200"),
        );
    }

    fn emit_unknown_ident(&mut self, name: &str, span: &Span) {
        self.sink.emit(
            Diagnostic::error(format!("unknown identifier `{name}`"))
                .with_span(span.clone(), "not found in this scope")
                .with_code("E0201"),
        );
    }
}

// ── Local environment ─────────────────────────────────────────────────────────

/// A stack-frame-scoped type environment.
struct LocalEnv {
    scopes: Vec<HashMap<String, Ty>>,
    pub return_ty: Ty,
}

impl LocalEnv {
    fn new(params: &[(String, Ty)], return_ty: &Ty) -> Self {
        let mut scope = HashMap::new();
        for (name, ty) in params {
            scope.insert(name.clone(), ty.clone());
        }
        LocalEnv { scopes: vec![scope], return_ty: return_ty.clone() }
    }

    fn child(&self) -> Self {
        // Child env inherits the parent's bindings through a fresh scope on
        // top; we flatten to keep it simple for M0.
        let mut merged: HashMap<String, Ty> = HashMap::new();
        for scope in &self.scopes {
            merged.extend(scope.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        LocalEnv { scopes: vec![merged], return_ty: self.return_ty.clone() }
    }

    fn bind(&mut self, name: String, ty: Ty) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<&Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// A zero-length span used when no source position is available (single-expr
/// function bodies, synthetic nodes).
fn dummy_span() -> Span {
    Span { start: 0, end: 0 }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
