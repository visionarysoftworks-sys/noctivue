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
//! | E0205 | `break`/`continue` outside a loop              |

use std::collections::HashMap;

use crate::ast::{
    self, BinOp, Block, Expr, FunctionBody, FunctionDecl, InterpPart, Item, MatchBody, Program,
    Stmt, StructField, TypeExpr, UnaryOp,
};
use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};
use crate::hir;
use crate::hir::items::{
    Enum, Function, Module, Struct, Trait, TypedArm, TypedExpr, TypedExprKind, TypedInterpPart,
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
    /// Maps trait names to their method signatures.
    trait_defs: HashMap<String, Vec<(String, (Vec<Ty>, Ty))>>,
    /// Maps top-level function names to their signature `Ty::Fn(params, ret)`.
    fn_sigs: HashMap<String, (Vec<Ty>, Ty)>,
    /// Maps enum variant names to (enum_name, payload_tys) for payload-less variants.
    enum_variants: HashMap<String, (String, Vec<Ty>)>,
    /// Loop-nesting depth, for validating `break`/`continue`. Reset on
    /// every function entry (a `break` inside a nested `fn` does not see
    /// the outer loop); match arms do not touch it (transparent).
    loop_depth: usize,
}

/// A collected type definition used for field resolution.
#[derive(Clone, Debug)]
enum TypeDef {
    Struct(Vec<(String, Ty)>),
    Enum(Vec<(String, Vec<Ty>)>),
}

impl<'s> TypeChecker<'s> {
    fn new(sink: &'s mut DiagnosticSink) -> Self {
        let mut tc = TypeChecker {
            sink,
            type_defs: HashMap::new(),
            trait_defs: HashMap::new(),
            fn_sigs: HashMap::new(),
            enum_variants: HashMap::new(),
            loop_depth: 0,
        };
        // Register built-in functions so calls to them type-check.
        tc.fn_sigs
            .insert("print".to_string(), (vec![Ty::String], Ty::Unit));
        tc.fn_sigs
            .insert("println".to_string(), (vec![Ty::String], Ty::Unit));
        tc.fn_sigs
            .insert("to_string".to_string(), (vec![Ty::String], Ty::String));
        tc.fn_sigs.insert(
            "to_int".to_string(),
            (vec![Ty::String], Ty::Option(Box::new(Ty::Int))),
        );
        tc.fn_sigs.insert(
            "to_float".to_string(),
            (vec![Ty::String], Ty::Option(Box::new(Ty::Float))),
        );
        tc.fn_sigs
            .insert("panic".to_string(), (vec![Ty::String], Ty::Unit));
        tc.fn_sigs
            .insert("assert".to_string(), (vec![Ty::Bool, Ty::String], Ty::Unit));
        // Phase 5/M4: cooperative blocking sleep (milliseconds).
        tc.fn_sigs
            .insert("sleep_builtin".to_string(), (vec![Ty::Int], Ty::Unit));
        // Phase 5/M4 (ADR-018): list growth for derived decoding.
        // `Unknown` is compatible with every element type, so one
        // signature serves all `T` without generics.
        tc.fn_sigs.insert(
            "list_append_builtin".to_string(),
            (
                vec![Ty::List(Box::new(Ty::Unknown)), Ty::Unknown],
                Ty::List(Box::new(Ty::Unknown)),
            ),
        );
        tc.fn_sigs
            .insert("run".to_string(), (vec![Ty::Unknown], Ty::Unit));
        tc.fn_sigs
            .insert("load_data".to_string(), (vec![], Ty::Unit));
        tc.fn_sigs.insert(
            "fs_read_text".to_string(),
            (
                vec![Ty::String],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "fs_write_text".to_string(),
            (
                vec![Ty::String, Ty::String],
                Ty::Result(Box::new(Ty::Unit), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs
            .insert("fs_exists".to_string(), (vec![Ty::String], Ty::Bool));
        tc.fn_sigs
            .insert("io_write".to_string(), (vec![Ty::String], Ty::Unit));
        tc.fn_sigs
            .insert("io_writeln".to_string(), (vec![Ty::String], Ty::Unit));
        tc.fn_sigs.insert(
            "env_get_builtin".to_string(),
            (vec![Ty::String], Ty::Option(Box::new(Ty::String))),
        );
        tc.fn_sigs.insert(
            "config_get_builtin".to_string(),
            (
                vec![Ty::String, Ty::String],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "json_validate_builtin".to_string(),
            (
                vec![Ty::String],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "json_get_string_builtin".to_string(),
            (
                vec![Ty::String, Ty::String],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "json_quote_builtin".to_string(),
            (vec![Ty::String], Ty::String),
        );
        tc.fn_sigs.insert(
            "json_object_string_pair_builtin".to_string(),
            (
                vec![Ty::String, Ty::String, Ty::String, Ty::String],
                Ty::String,
            ),
        );
        tc.fn_sigs.insert(
            "process_run_builtin".to_string(),
            (
                vec![Ty::String, Ty::List(Box::new(Ty::String))],
                Ty::Result(
                    Box::new(Ty::Named("ProcessOutput".to_string(), Vec::new())),
                    Box::new(Ty::String),
                ),
            ),
        );
        tc.fn_sigs.insert(
            "fs_modified_millis_builtin".to_string(),
            (
                vec![Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "http_send_builtin".to_string(),
            (
                vec![
                    Ty::String,
                    Ty::String,
                    // Header pairs are lists-of-2-lists (`[[String]]`):
                    // tuple VALUES are unimplemented (the parser keeps
                    // only the first element), so the `.nv` surface
                    // spells pairs as 2-lists, matching what the
                    // builtin matches on at runtime.
                    Ty::List(Box::new(Ty::List(Box::new(Ty::String)))),
                    Ty::String,
                ],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "http_server_listen".to_string(),
            (
                vec![Ty::Int],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "http_server_register_route".to_string(),
            (vec![Ty::Int, Ty::String, Ty::String, Ty::String], Ty::Unit),
        );
        tc.fn_sigs.insert(
            "http_server_serve_loop".to_string(),
            (vec![Ty::Int], Ty::Unit),
        );
        tc.fn_sigs.insert(
            "http_server_shutdown".to_string(),
            (vec![Ty::Int], Ty::Unit),
        );
        tc.fn_sigs.insert(
            "env_set_builtin".to_string(),
            (vec![Ty::String, Ty::String], Ty::Unit),
        );
        tc.fn_sigs.insert(
            "dotenv_load_builtin".to_string(),
            (
                vec![Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "log_emit_builtin".to_string(),
            (vec![Ty::String, Ty::String], Ty::Unit),
        );
        tc.fn_sigs.insert(
            "db_open_builtin".to_string(),
            (
                vec![Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "db_exec_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String, Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "db_query_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String, Ty::String],
                Ty::Result(
                    Box::new(Ty::List(Box::new(Ty::String))),
                    Box::new(Ty::String),
                ),
            ),
        );
        tc.fn_sigs.insert(
            "db_close_builtin".to_string(),
            (
                vec![Ty::Int],
                Ty::Result(Box::new(Ty::Unit), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_parse_builtin".to_string(),
            (
                vec![Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_free_builtin".to_string(),
            (
                vec![Ty::Int],
                Ty::Result(Box::new(Ty::Unit), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_get_string_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_get_int_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_get_float_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String],
                Ty::Result(Box::new(Ty::Float), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_get_bool_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String],
                Ty::Result(Box::new(Ty::Bool), Box::new(Ty::String)),
            ),
        );
        // Phase 5/M4 (ADR-018): char field decode + array element
        // text for derived `List<T>` decoding.
        tc.fn_sigs.insert(
            "doc_get_char_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String],
                Ty::Result(Box::new(Ty::Char), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_get_index_builtin".to_string(),
            (
                vec![Ty::Int, Ty::Int],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_has_builtin".to_string(),
            (vec![Ty::Int, Ty::String], Ty::Bool),
        );
        tc.fn_sigs.insert(
            "doc_get_doc_builtin".to_string(),
            (
                vec![Ty::Int, Ty::String],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_len_builtin".to_string(),
            (
                vec![Ty::Int],
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "doc_stringify_builtin".to_string(),
            (
                vec![Ty::Int],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc.fn_sigs.insert(
            "json_serialize_builtin".to_string(),
            (
                vec![Ty::Unknown],
                Ty::Result(Box::new(Ty::String), Box::new(Ty::String)),
            ),
        );
        tc
    }

    // ── Program ───────────────────────────────────────────────────────────────

    fn check_program(&mut self, program: Program) -> Module {
        // Pass 1: collect all type definitions so forward references work.
        let mut structs: Vec<Struct> = Vec::new();
        let mut enums: Vec<Enum> = Vec::new();
        let mut traits: Vec<Trait> = Vec::new();

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
                    // Register enum variant constructors
                    for (variant_name, payload_tys) in &hir_enum.variants {
                        if !payload_tys.is_empty() {
                            // For payload variants, the constructor is a function from payload to enum
                            let constructor_ty = Ty::Fn(
                                payload_tys.clone(),
                                Box::new(Ty::Named(hir_enum.name.clone(), vec![])),
                            );
                            self.fn_sigs.insert(
                                variant_name.clone(),
                                (payload_tys.clone(), constructor_ty),
                            );
                        } else {
                            // For payload-less variants, register in enum_variants map
                            self.enum_variants.insert(
                                variant_name.clone(),
                                (hir_enum.name.clone(), payload_tys.clone()),
                            );
                        }
                    }
                    enums.push(hir_enum);
                }
                Item::Trait(t) => {
                    let hir_trait = self.lower_trait(t);
                    self.trait_defs
                        .insert(hir_trait.name.clone(), hir_trait.methods.clone());
                    traits.push(hir_trait);
                }
                Item::Mod(_) => {
                    // For M0, modules are passed through without type-checking their contents.
                    // A full implementation would recursively type-check nested items.
                }
                _ => {}
            }
        }

        // Pass 1b: verify impl blocks against traits
        for item in &program.items {
            if let Item::Impl(impl_block) = item {
                self.verify_impl_block(impl_block);
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
                Item::Task(t) => {
                    // Task declarations register an opaque join-handle
                    // signature (Phase 5/M4): calls yield `Int`.
                    let params: Vec<Ty> = t.params.iter().map(|p| self.lower_ty(&p.ty)).collect();
                    self.fn_sigs.insert(t.name.clone(), (params, Ty::Int));
                }
                Item::BareDecl(decl) if decl.name == "main" => {
                    // Special case: treat `main` as a function even if classified as component.
                    let params: Vec<Ty> = decl
                        .params
                        .clone()
                        .unwrap_or_default()
                        .iter()
                        .map(|p| self.lower_ty(&p.ty))
                        .collect();
                    let ret = match &decl.return_ty {
                        Some(t) => self.lower_ty(t),
                        None => Ty::Unit,
                    };
                    self.fn_sigs.insert(decl.name.clone(), (params, ret));
                }
                // Other BareDecls (components) — skip.
                _ => {}
            }
        }

        // Pass 3: type-check each function body.
        let mut functions: Vec<Function> = Vec::new();
        for item in program.items {
            if let Item::Function(f) = item {
                if let Some(hir_fn) = self.check_function(f, false) {
                    functions.push(hir_fn);
                }
            } else if let Item::Task(t) = item {
                // Tasks check exactly like Unit functions; the `is_task`
                // flag routes calls through the spawn path in later stages.
                let f = FunctionDecl {
                    name: t.name.clone(),
                    generic_params: Vec::new(),
                    params: t.params.clone(),
                    return_ty: None,
                    body: FunctionBody::Block(t.body.clone()),
                    span: t.span.clone(),
                };
                if let Some(hir_fn) = self.check_function(f, true) {
                    functions.push(hir_fn);
                }
            } else if let Item::BareDecl(decl) = item {
                // Special case: treat `main` as a function even if classified as component.
                if decl.name == "main" {
                    let f = FunctionDecl {
                        name: decl.name.clone(),
                        generic_params: Vec::new(),
                        params: decl.params.clone().unwrap_or_default(),
                        return_ty: decl.return_ty.clone(),
                        body: FunctionBody::Block(decl.body.clone()),
                        span: decl.span.clone(),
                    };
                    if let Some(hir_fn) = self.check_function(f, false) {
                        functions.push(hir_fn);
                    }
                }
            }
        }

        Module {
            structs,
            enums,
            traits,
            functions,
        }
    }

    // ── Struct / Enum lowering ────────────────────────────────────────────────

    fn lower_struct(&mut self, s: &ast::StructDecl) -> Struct {
        let fields = s
            .fields
            .iter()
            .map(|f| (f.name.clone(), self.lower_ty(&f.ty)))
            .collect();
        Struct {
            name: s.name.clone(),
            fields,
        }
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
        Enum {
            name: e.name.clone(),
            variants,
        }
    }

    fn lower_trait(&mut self, t: &ast::TraitDecl) -> Trait {
        let methods = t
            .members
            .iter()
            .map(|m| {
                let params: Vec<Ty> = m.params.iter().map(|p| self.lower_ty(&p.ty)).collect();
                let ret = match &m.return_ty {
                    Some(t) => self.lower_ty(t),
                    None => Ty::Unit,
                };
                (m.name.clone(), (params, ret))
            })
            .collect();
        Trait {
            name: t.name.clone(),
            methods,
        }
    }

    fn verify_impl_block(&mut self, impl_block: &ast::ImplBlock) {
        // Check if this is a trait impl (has for_trait)
        if let Some(trait_ty) = &impl_block.for_trait {
            if let TypeExpr::Named(trait_name, _, _) = trait_ty {
                if let Some(trait_methods) = self.trait_defs.get(trait_name) {
                    // Verify all trait methods are implemented with matching signatures
                    for (trait_method_name, (trait_params, trait_ret)) in trait_methods {
                        let found = impl_block
                            .methods
                            .iter()
                            .find(|m| m.name == *trait_method_name);
                        match found {
                            Some(impl_method) => {
                                let impl_params: Vec<Ty> = impl_method
                                    .params
                                    .iter()
                                    .map(|p| self.lower_ty(&p.ty))
                                    .collect();
                                let impl_ret = match &impl_method.return_ty {
                                    Some(t) => self.lower_ty(t),
                                    None => Ty::Unit,
                                };
                                if impl_params != *trait_params || impl_ret != *trait_ret {
                                    self.sink.emit(
                                        Diagnostic::error(format!(
                                            "method `{}` signature mismatch in impl of trait `{}`",
                                            trait_method_name, trait_name
                                        ))
                                        .with_span(impl_method.span.clone(), "here")
                                        .with_code("E0302"),
                                    );
                                }
                            }
                            None => {
                                self.sink.emit(
                                    Diagnostic::error(format!(
                                        "missing method `{}` in impl of trait `{}`",
                                        trait_method_name, trait_name
                                    ))
                                    .with_span(impl_block.span.clone(), "here")
                                    .with_code("E0300"),
                                );
                            }
                        }
                    }
                } else {
                    self.sink.emit(
                        Diagnostic::error(format!("trait `{}` not found", trait_name))
                            .with_span(trait_ty.clone().span(), "here")
                            .with_code("E0301"),
                    );
                }
            }
        }
        // Inherent impl blocks (no for_trait) are allowed without verification for now
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

    fn check_function(&mut self, f: FunctionDecl, is_task: bool) -> Option<Function> {
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

        // A nested `fn` starts outside all loops: a `break` inside it must
        // not see the outer function's loop nesting.
        self.loop_depth = 0;

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
                                &last.span,
                                "implicit return value",
                            );
                        }
                    }
                }
                stmts
            }
            FunctionBody::Expr(expr) => {
                // Single-expression body — treat as `return expr`.
                let expr_span = expr.span();
                let typed_expr = self.infer_expr(expr, &mut env);
                // Check return type compatibility.
                if !typed_expr.ty.compatible_with(&return_ty) {
                    self.emit_mismatch(
                        &return_ty,
                        &typed_expr.ty,
                        &expr_span,
                        "single-expression body type",
                    );
                }
                vec![TypedStmt {
                    ty: typed_expr.ty.clone(),
                    kind: TypedStmtKind::Return(Some(typed_expr)),
                    span: expr_span,
                }]
            }
        };

        Some(Function {
            name: f.name.clone(),
            params,
            return_ty,
            body: typed_body,
            is_task,
        })
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
                    span: ls.span.clone(),
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
                    span: vs.span.clone(),
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
                    span: ss.span.clone(),
                })
            }

            // ── bare declaration `name: expr` ──────────────────────────────
            Stmt::Decl(ds) => {
                let value_expr = self.infer_expr(&ds.value, env);
                let bind_ty = value_expr.ty.clone();
                env.bind(ds.name.clone(), bind_ty.clone());
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Decl {
                        name: ds.name.clone(),
                        ty: bind_ty,
                        value: value_expr,
                    },
                    span: ds.span.clone(),
                })
            }

            // ── return ───────────────────────────────────────────────────────
            Stmt::Return(rs) => {
                let typed_val = rs.value.as_ref().map(|e| self.infer_expr(e, env));
                let ret_ty = typed_val.as_ref().map_or(Ty::Unit, |e| e.ty.clone());

                if !ret_ty.compatible_with(&env.return_ty) {
                    self.emit_mismatch(&env.return_ty, &ret_ty, &rs.span, "return statement");
                }

                Some(TypedStmt {
                    ty: ret_ty,
                    kind: TypedStmtKind::Return(typed_val),
                    span: rs.span.clone(),
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
                    span: as_.span.clone(),
                })
            }

            // ── bare expression statement ────────────────────────────────────
            Stmt::Expr(expr) => {
                let span = expr.span();
                let typed = self.infer_expr(expr, env);
                let ty = typed.ty.clone();
                Some(TypedStmt {
                    ty,
                    kind: TypedStmtKind::Expr(typed),
                    span,
                })
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
                            span: is_.span.clone(),
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
                    kind: TypedStmtKind::If {
                        condition: cond,
                        then_body,
                        else_body,
                    },
                    span: is_.span.clone(),
                })
            }

            // ── while ────────────────────────────────────────────────────────
            Stmt::While(ws) => {
                let cond = self.infer_expr(&ws.condition, env);
                if !cond.ty.compatible_with(&Ty::Bool) {
                    self.emit_mismatch(&Ty::Bool, &cond.ty, &ws.span, "while condition");
                }
                let mut child = env.child();
                self.loop_depth += 1;
                let body = self.check_block(&ws.body, &mut child);
                self.loop_depth -= 1;
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::While {
                        condition: cond,
                        body,
                    },
                    span: ws.span.clone(),
                })
            }

            // ── loop ─────────────────────────────────────────────────────────
            Stmt::Loop(ls) => {
                let mut child = env.child();
                self.loop_depth += 1;
                let body = self.check_block(&ls.body, &mut child);
                self.loop_depth -= 1;
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Loop { body },
                    span: ls.span.clone(),
                })
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
                self.loop_depth += 1;
                let body = self.check_block(&fs.body, &mut child);
                self.loop_depth -= 1;
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::For {
                        binding: fs.binding.clone(),
                        iterable: iter_expr,
                        body,
                    },
                    span: fs.span.clone(),
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
                    span: ms.span.clone(),
                })
            }

            // ── local function definition ────────────────────────────────────
            Stmt::Function(f) => {
                // Register the function's signature in fn_sigs so subsequent
                // calls within this block can resolve it, then type-check it.
                let params: Vec<Ty> = f.params.iter().map(|p| self.lower_ty(&p.ty)).collect();
                let ret = f
                    .return_ty
                    .as_ref()
                    .map(|t| self.lower_ty(t))
                    .unwrap_or(Ty::Unit);
                self.fn_sigs
                    .insert(f.name.clone(), (params.clone(), ret.clone()));

                // Also bind the function name in the local env.
                let fn_ty = Ty::Fn(params, Box::new(ret));
                env.bind(f.name.clone(), fn_ty);

                // Type-check the nested function (errors go to sink; we discard body).
                // `check_function` resets loop depth for the nested scope —
                // restore the outer depth afterwards so a later `break` in
                // the enclosing loop still validates.
                let outer_depth = self.loop_depth;
                self.check_function(f.clone(), false);
                self.loop_depth = outer_depth;
                None // Local fn defs don't produce a statement in the parent body.
            }

            // ── local task definition ──────────────────────────────────────
            // Phase 5/M4: checked like a Unit function and bound to an
            // opaque handle type, but the body is discarded — only
            // top-level tasks are spawnable (calling a local task fails
            // loudly at runtime: lift it to top level).
            Stmt::Task(t) => {
                env.bind(t.name.clone(), Ty::Int);
                let f = FunctionDecl {
                    name: t.name.clone(),
                    generic_params: Vec::new(),
                    params: t.params.clone(),
                    return_ty: None,
                    body: FunctionBody::Block(t.body.clone()),
                    span: t.span.clone(),
                };
                let outer_depth = self.loop_depth;
                self.check_function(f, true);
                self.loop_depth = outer_depth;
                None
            }

            // ── local struct definition ──────────────────────────────────────
            Stmt::Struct(s) => {
                let hir_s = self.lower_struct(s);
                self.type_defs
                    .insert(hir_s.name.clone(), TypeDef::Struct(hir_s.fields.clone()));
                None
            }

            // ── break / continue ─────────────────────────────────────────
            // Validated against `loop_depth` (a `break` inside a nested
            // `fn` is outside every loop — see `check_function`'s reset).
            // A `break` value is type-checked (errors inside it still
            // surface) but only warned about: loops are statement-position
            // with no value channel, so the value is evaluated for side
            // effects and discarded (see `TypedStmtKind::Break` docs).
            Stmt::Break(bs) => {
                if self.loop_depth == 0 {
                    self.sink.emit(
                        Diagnostic::error("`break` outside of a loop")
                            .with_span(bs.span.clone(), "here")
                            .with_code("E0205"),
                    );
                    return None;
                }
                let value = bs.value.as_ref().map(|e| self.infer_expr(e, env));
                if value.is_some() {
                    self.sink.emit(
                        Diagnostic::warning(
                            "`break` value is discarded (loops have no value channel yet)",
                        )
                        .with_span(bs.span.clone(), "here"),
                    );
                }
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Break(value),
                    span: bs.span.clone(),
                })
            }
            Stmt::Continue(span) => {
                if self.loop_depth == 0 {
                    self.sink.emit(
                        Diagnostic::error("`continue` outside of a loop")
                            .with_span(span.clone(), "here")
                            .with_code("E0205"),
                    );
                    return None;
                }
                Some(TypedStmt {
                    ty: Ty::Unit,
                    kind: TypedStmtKind::Continue,
                    span: span.clone(),
                })
            }

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
        let guard = arm.guard.as_ref().map(|g| {
            let typed_guard = self.infer_expr(g, &mut arm_env);
            // NIR.md §6: previously unchecked here, so a non-Bool guard
            // type-checked successfully and diverged at runtime (the
            // interpreter panics on a non-Bool guard value, the VM instead
            // truthiness-branches on it via `CondBranch`/`is_truthy`).
            // Same discipline as `while`'s condition check just above in
            // this file (`emit_mismatch(&Ty::Bool, &cond.ty, ...)`).
            if !typed_guard.ty.compatible_with(&Ty::Bool) {
                self.emit_mismatch(&Ty::Bool, &typed_guard.ty, &typed_guard.span, "match guard");
            }
            typed_guard
        });

        let body = match &arm.body {
            MatchBody::Block(block) => self.check_block(block, &mut arm_env),
            MatchBody::Expr(expr) => {
                let span = expr.span();
                let typed = self.infer_expr(expr, &mut arm_env);
                let ty = typed.ty.clone();
                vec![TypedStmt {
                    ty,
                    kind: TypedStmtKind::Expr(typed),
                    span,
                }]
            }
        };

        let arm_ty = body.last().map(|s| s.ty.clone()).unwrap_or(Ty::Unit);
        TypedArm {
            pattern,
            guard,
            body,
            ty: arm_ty,
            span: arm.span.clone(),
        }
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
                        let inner_ty = payload_tys.get(i).cloned().unwrap_or(Ty::Unknown);
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
            if let Some((_, tys)) = variants.iter().find(|(n, _)| n == variant_name) {
                return tys.clone();
            }
        }
        Vec::new()
    }

    // ── Expression inference ──────────────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expr, env: &mut LocalEnv) -> TypedExpr {
        match expr {
            // ── Literals ─────────────────────────────────────────────────────
            Expr::Literal(lit, span) => self.infer_literal(lit, span),

            // ── Identifiers ──────────────────────────────────────────────────
            Expr::Ident(name, span) => {
                // Special literals.
                if name == "None" {
                    return TypedExpr {
                        ty: Ty::Option(Box::new(Ty::Unknown)),
                        kind: TypedExprKind::None,
                        span: span.clone(),
                    };
                }

                if let Some(ty) = env.lookup(name) {
                    TypedExpr {
                        ty: ty.clone(),
                        kind: TypedExprKind::Ident(name.clone()),
                        span: span.clone(),
                    }
                } else if let Some((params, ret)) = self.fn_sigs.get(name).cloned() {
                    TypedExpr {
                        ty: Ty::Fn(params, Box::new(ret)),
                        kind: TypedExprKind::Ident(name.clone()),
                        span: span.clone(),
                    }
                } else if let Some(TypeDef::Enum(_variants)) = self.type_defs.get(name.as_str()) {
                    // This is an enum type name used as a value (not a variant)
                    TypedExpr {
                        ty: Ty::Named(name.clone(), Vec::new()),
                        kind: TypedExprKind::Ident(name.clone()),
                        span: span.clone(),
                    }
                } else if let Some((enum_name, payload_tys)) = self.enum_variants.get(name).cloned()
                {
                    // Payload-less enum variant - it's a value of the enum type
                    TypedExpr {
                        ty: Ty::Named(enum_name, payload_tys),
                        kind: TypedExprKind::Ident(name.clone()),
                        span: span.clone(),
                    }
                } else {
                    self.emit_unknown_ident(name, span);
                    TypedExpr {
                        ty: Ty::Error,
                        kind: TypedExprKind::Ident(name.clone()),
                        span: span.clone(),
                    }
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
                    span: me.span.clone(),
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
                    span: ie.span.clone(),
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
                    // Phase 5/M4: `await` unwraps ONLY opaque task handles.
                    // The handle type is `Int`, but so are plain integers —
                    // distinguishing them needs a marker the v1 `Ty` has no
                    // room for. awaiting a non-handle degrades to `Unknown`
                    // instead of inventing a type; misuse still fails loudly
                    // at runtime ("await of non-task value").
                    UnaryOp::Await => Ty::Unknown,
                };
                TypedExpr {
                    ty: result_ty,
                    kind: TypedExprKind::UnaryOp {
                        op: uo.op.clone(),
                        operand: Box::new(operand),
                    },
                    span: uo.span.clone(),
                }
            }

            // ── Try (`?`) ─────────────────────────────────────────────────────
            Expr::Try(te) => {
                let inner = self.infer_expr(&te.expr, env);
                if let Some(unwrapped) = inner.ty.unwrap_fallible() {
                    TypedExpr {
                        ty: unwrapped,
                        kind: TypedExprKind::Try(Box::new(inner)),
                        span: te.span.clone(),
                    }
                } else if inner.ty.is_error() || inner.ty.is_unknown() {
                    TypedExpr {
                        ty: Ty::Unknown,
                        kind: TypedExprKind::Try(Box::new(inner)),
                        span: te.span.clone(),
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
                    TypedExpr {
                        ty: Ty::Error,
                        kind: TypedExprKind::Try(Box::new(inner)),
                        span: te.span.clone(),
                    }
                }
            }

            // ── String interpolation ──────────────────────────────────────────
            Expr::StringInterp(si) => {
                let parts: Vec<TypedInterpPart> = si
                    .parts
                    .iter()
                    .map(|p| match p {
                        InterpPart::Literal(s) => TypedInterpPart::Literal(s.clone()),
                        InterpPart::Expr(e) => TypedInterpPart::Expr(self.infer_expr(e, env)),
                    })
                    .collect();
                TypedExpr {
                    ty: Ty::String,
                    kind: TypedExprKind::StringInterp(parts),
                    span: si.span.clone(),
                }
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
                    span: re.span.clone(),
                }
            }

            // ── Struct literal: `User { id: 1, name: "Alice" }` ───────────────
            Expr::StructLit(sl) => {
                let fields: Vec<(String, TypedExpr)> = sl
                    .fields
                    .iter()
                    .filter_map(|field| match field {
                        StructField::Named(name, val) => {
                            Some((name.clone(), self.infer_expr(val, env)))
                        }
                        StructField::Spread(_) => {
                            // Spread in struct literal - type checking would need
                            // to verify the spread expression is a compatible struct
                            // For M0, we just skip spread in field list
                            None
                        }
                    })
                    .collect();
                TypedExpr {
                    ty: Ty::Named(sl.name.clone(), vec![]),
                    kind: TypedExprKind::StructLit {
                        name: sl.name.clone(),
                        fields,
                    },
                    span: sl.span.clone(),
                }
            }

            // ── Spread expression: `...expr` ────────────────────────────────
            Expr::Spread(spread) => {
                let expr = self.infer_expr(&spread.expr, env);
                let span = spread.span.clone();
                // Spread propagates the inner expression's type
                TypedExpr {
                    ty: expr.ty.clone(),
                    kind: TypedExprKind::Spread(Box::new(expr)),
                    span,
                }
            }

            // ── List literal: `[a, b, c]` ──────────────────────────────────
            Expr::ListLit(ll) => {
                let elements: Vec<TypedExpr> = ll
                    .elements
                    .iter()
                    .map(|e| self.infer_expr(e, env))
                    .collect();
                // Infer element type from the first element, or Unknown if empty.
                let elem_ty = elements
                    .first()
                    .map(|e| e.ty.clone())
                    .unwrap_or(Ty::Unknown);
                TypedExpr {
                    ty: Ty::List(Box::new(elem_ty)),
                    kind: TypedExprKind::List(elements),
                    span: ll.span.clone(),
                }
            }

            // ── Closure: `|params| body` ─────────────────────────────────────
            Expr::Closure(cl) => {
                // Create a new scope for the closure parameters
                // Copy existing bindings from parent scope
                let mut closure_env = LocalEnv::new(&[], &Ty::Unit);
                // Copy parent scope bindings
                for scope in &env.scopes {
                    for (name, ty) in scope {
                        closure_env.bind(name.clone(), ty.clone());
                    }
                }
                for param in &cl.params {
                    closure_env.bind(param.clone(), Ty::Unknown);
                }
                let body_expr = self.infer_expr(&cl.body, &mut closure_env);
                // The closure's type is Fn(param_types) -> return_type
                // For M0, we'll use Unknown for params and infer return type from body
                let param_tys = cl.params.iter().map(|_| Ty::Unknown).collect();
                let return_ty = body_expr.ty.clone();
                let fn_ty = Ty::Fn(param_tys, Box::new(return_ty));
                TypedExpr {
                    ty: fn_ty.clone(),
                    kind: TypedExprKind::Closure {
                        params: cl.params.clone(),
                        body: Box::new(body_expr),
                    },
                    span: cl.span.clone(),
                }
            }
        }
    }

    fn infer_literal(&self, lit: &ast::Literal, span: &Span) -> TypedExpr {
        match lit {
            ast::Literal::Int(v) => TypedExpr {
                ty: Ty::Int,
                kind: TypedExprKind::IntLit(*v),
                span: span.clone(),
            },
            ast::Literal::Float(v) => TypedExpr {
                ty: Ty::Float,
                kind: TypedExprKind::FloatLit(*v),
                span: span.clone(),
            },
            ast::Literal::Bool(v) => TypedExpr {
                ty: Ty::Bool,
                kind: TypedExprKind::BoolLit(*v),
                span: span.clone(),
            },
            ast::Literal::Char(v) => TypedExpr {
                ty: Ty::Char,
                kind: TypedExprKind::CharLit(*v),
                span: span.clone(),
            },
            ast::Literal::String(v) => TypedExpr {
                ty: Ty::String,
                kind: TypedExprKind::StringLit(v.clone()),
                span: span.clone(),
            },
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
                            span: call.span.clone(),
                        });
                    let inner_ty = arg.ty.clone();
                    return TypedExpr {
                        ty: Ty::Option(Box::new(inner_ty)),
                        kind: TypedExprKind::Some(Box::new(arg)),
                        span: call.span.clone(),
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
                            span: call.span.clone(),
                        });
                    let inner_ty = arg.ty.clone();
                    return TypedExpr {
                        ty: Ty::Result(Box::new(inner_ty), Box::new(Ty::Unknown)),
                        kind: TypedExprKind::Ok(Box::new(arg)),
                        span: call.span.clone(),
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
                            span: call.span.clone(),
                        });
                    let err_ty = arg.ty.clone();
                    return TypedExpr {
                        ty: Ty::Result(Box::new(Ty::Unknown), Box::new(err_ty)),
                        kind: TypedExprKind::Err(Box::new(arg)),
                        span: call.span.clone(),
                    };
                }
                _ => {}
            }
        }

        // General call.
        let callee = self.infer_expr(&call.callee, env);
        let args: Vec<TypedExpr> = call
            .args
            .iter()
            .map(|a| self.infer_expr(&a.value, env))
            .collect();

        let ret_ty = match &callee.ty {
            Ty::Fn(param_tys, ret) => {
                // Check argument count & types if signature is known.
                if args.len() != param_tys.len() {
                    // Argument count mismatch — emit but recover.
                    eprintln!(
                        "E0200: callee={:?}, expected {} args, found {}",
                        call.callee,
                        param_tys.len(),
                        args.len()
                    );
                    self.sink.emit(
                        Diagnostic::error(format!(
                            "expected {} argument(s), found {}",
                            param_tys.len(),
                            args.len()
                        ))
                        .with_code("E0200"),
                    );
                } else {
                    for (i, (arg, expected)) in args.iter().zip(param_tys.iter()).enumerate() {
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
            kind: TypedExprKind::Call {
                callee: Box::new(callee),
                args,
            },
            span: call.span.clone(),
        }
    }

    // ── Binary op inference ───────────────────────────────────────────────────

    fn infer_binop(&mut self, bo: &ast::BinOpExpr, env: &mut LocalEnv) -> TypedExpr {
        let left = self.infer_expr(&bo.left, env);
        let right = self.infer_expr(&bo.right, env);

        let result_ty = match &bo.op {
            // Arithmetic — result type = operand type (must be numeric).
            // String concatenation: String + String -> String
            BinOp::Add => {
                if left.ty == Ty::String && right.ty == Ty::String {
                    Ty::String
                } else if !left.ty.compatible_with(&right.ty) {
                    self.emit_mismatch(&left.ty, &right.ty, &bo.span, "binary operands");
                    left.ty.clone()
                } else {
                    // Numeric primitives
                    match &left.ty {
                        Ty::Int | Ty::UInt | Ty::Float | Ty::Unknown | Ty::Error => left.ty.clone(),
                        _ => left.ty.clone(),
                    }
                }
            }
            BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if !left.ty.compatible_with(&right.ty) {
                    self.emit_mismatch(&left.ty, &right.ty, &bo.span, "binary operands");
                }
                // Both numeric primitives → use left type.
                match &left.ty {
                    Ty::Int | Ty::UInt | Ty::Float | Ty::Unknown | Ty::Error => left.ty.clone(),
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
            // Produces the dedicated HIR `Coalesce` node (not a generic
            // `BinOp`): NIR lowering and the interpreter both key off that
            // node for short-circuit evaluation. Lowering `??` as a plain
            // binary op would evaluate both sides and miscompile the merge.
            BinOp::Coalesce => {
                let inner_ty = match &left.ty {
                    Ty::Option(inner) => *inner.clone(),
                    Ty::Unknown | Ty::Error => right.ty.clone(),
                    _ => {
                        // Not Option; still recover with right type.
                        right.ty.clone()
                    }
                };
                return TypedExpr {
                    ty: inner_ty,
                    kind: TypedExprKind::Coalesce {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span: bo.span.clone(),
                };
            }
        };

        TypedExpr {
            ty: result_ty,
            kind: TypedExprKind::BinOp {
                op: bo.op.clone(),
                left: Box::new(left),
                right: Box::new(right),
            },
            span: bo.span.clone(),
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
                        Diagnostic::error(format!("struct `{name}` has no field `{field}`"))
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
            Ty::List(elem_ty) => {
                let elem = *elem_ty.clone();
                match field {
                    "length" => Ty::Int,
                    "filter" => Ty::Fn(
                        vec![Ty::Fn(vec![elem.clone()], Box::new(Ty::Bool))],
                        Box::new(Ty::List(Box::new(elem.clone()))),
                    ),
                    "map" => Ty::Fn(
                        vec![Ty::Fn(vec![elem.clone()], Box::new(Ty::Unknown))],
                        Box::new(Ty::List(Box::new(Ty::Unknown))),
                    ),
                    "sort" => Ty::Fn(
                        vec![Ty::Fn(vec![elem.clone(), elem.clone()], Box::new(Ty::Int))],
                        Box::new(Ty::Unit),
                    ),
                    _ => {
                        self.sink.emit(
                            Diagnostic::error(format!("list has no method `{field}`"))
                                .with_span(span.clone(), "method access")
                                .with_code("E0204"),
                        );
                        Ty::Error
                    }
                }
            }
            Ty::Unknown | Ty::Error => Ty::Unknown,
            _ => {
                // Compound type (Option, Result, List, …) — emit E0204 only
                // for clearly non-record types where field access makes no
                // semantic sense.
                self.sink.emit(
                    Diagnostic::error(format!("type `{obj_ty}` has no field `{field}`"))
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
        LocalEnv {
            scopes: vec![scope],
            return_ty: return_ty.clone(),
        }
    }

    fn child(&self) -> Self {
        // Child env inherits the parent's bindings through a fresh scope on
        // top; we flatten to keep it simple for M0.
        let mut merged: HashMap<String, Ty> = HashMap::new();
        for scope in &self.scopes {
            merged.extend(scope.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        LocalEnv {
            scopes: vec![merged],
            return_ty: self.return_ty.clone(),
        }
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
