//! Derive expansion (Phase 5/M4, ADR-018).
//!
//! `derive Serialize for T:` / `derive Deserialize for T:` expand here,
//! at resolve time, into ordinary items — `to_json_<T>` /
//! `from_json_<T>` functions plus a marker `impl` block. Downstream
//! stages never know the items were synthesized: they typecheck,
//! lower, and run exactly like hand-written code.
//!
//! The expansion depends ONLY on builtins (`json_quote_builtin`,
//! `doc_*_builtin`, `to_int/float_builtin`, `list_append_builtin`,
//! `+` on strings) and on sibling synthesized functions — never on
//! `.nv` library code, so a bare file with just the target struct
//! derives cleanly. Two scope requirements fail loudly at typeck
//! (not here): `JsonDoc` must be in scope
//! (`encoding/json/document.nv`) and the marker trait must be
//! declared (`encoding/json/serialize.nv`).
//!
//! All synthesized nodes share the `derive` declaration's span, so
//! any downstream diagnostic points at the line the user wrote.
//! Generated bindings use the `__derive_` prefix, which hand-written
//! code should treat as reserved inside derived functions.
//!
//! Decode strictness (documented, ADR-018 §"missing fields"):
//! - Missing required fields fail naming `Type.field`.
//! - `Option<scalar>` maps any unreadable value (absent key,
//!   explicit null, mistype) to `None` — total by design.
//! - `Option<composite>` is strict once present: a present
//!   object/array with bad interior propagates the error.
//! - List elements are always strict (a present element with the
//!   wrong shape fails the whole decode).

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};

pub const SERIALIZE: &str = "Serialize";
pub const DESERIALIZE: &str = "Deserialize";

// ── Field classification ────────────────────────────────────────────────────

/// Scalar JSON leaf types (v1 closed set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    Int,
    Float,
    Bool,
    Char,
    String,
}

/// A struct field's shape for expansion purposes.
#[derive(Debug, Clone)]
enum FieldKind {
    Scalar(Scalar),
    /// Named struct type (must itself derive, same direction).
    Nested(String),
    Option(Box<FieldKind>),
    List(Box<FieldKind>),
    /// Anything else, with a human description for the error.
    Unsupported(String),
}

fn classify(ty: &TypeExpr) -> FieldKind {
    match ty {
        TypeExpr::Named(name, args, _) => match (name.as_str(), args.len()) {
            ("Int", 0) => FieldKind::Scalar(Scalar::Int),
            ("Float", 0) => FieldKind::Scalar(Scalar::Float),
            ("Bool", 0) => FieldKind::Scalar(Scalar::Bool),
            ("Char", 0) => FieldKind::Scalar(Scalar::Char),
            ("String", 0) => FieldKind::Scalar(Scalar::String),
            ("Option", 1) => FieldKind::Option(Box::new(classify(&args[0]))),
            ("Result", _) => FieldKind::Unsupported(type_expr_to_string(ty)),
            (other, 0) => FieldKind::Nested(other.to_string()),
            _ => FieldKind::Unsupported(type_expr_to_string(ty)),
        },
        TypeExpr::Collection(inner, _) => FieldKind::List(Box::new(classify(inner))),
        TypeExpr::Tuple(_, _) => FieldKind::Unsupported("tuple".to_string()),
        TypeExpr::Function(_, _, _) => FieldKind::Unsupported("function type".to_string()),
    }
}

/// Options compose with scalars, nested structs, and lists — but not
/// with options, and never as list elements (element decode has no
/// total expression shape for the double layer).
fn option_arg_ok(inner: &FieldKind) -> bool {
    matches!(
        inner,
        FieldKind::Scalar(_) | FieldKind::Nested(_) | FieldKind::List(_)
    )
}

fn list_elem_ok(inner: &FieldKind) -> bool {
    matches!(inner, FieldKind::Scalar(_) | FieldKind::Nested(_))
}

/// Rebuild a `TypeExpr` for a supported kind (accumulator annotations).
fn kind_type(kind: &FieldKind, span: &Span) -> Option<TypeExpr> {
    let named = |n: &str| TypeExpr::Named(n.to_string(), Vec::new(), span.clone());
    match kind {
        FieldKind::Scalar(Scalar::Int) => Some(named("Int")),
        FieldKind::Scalar(Scalar::Float) => Some(named("Float")),
        FieldKind::Scalar(Scalar::Bool) => Some(named("Bool")),
        FieldKind::Scalar(Scalar::Char) => Some(named("Char")),
        FieldKind::Scalar(Scalar::String) => Some(named("String")),
        FieldKind::Nested(n) => Some(named(n)),
        FieldKind::Option(inner) => Some(TypeExpr::Named(
            "Option".to_string(),
            vec![kind_type(inner, span)?],
            span.clone(),
        )),
        FieldKind::List(inner) => Some(TypeExpr::Collection(
            Box::new(kind_type(inner, span)?),
            span.clone(),
        )),
        FieldKind::Unsupported(_) => None,
    }
}

/// Direct nested-struct dependencies of a field type.
fn nested_deps(kind: &FieldKind, out: &mut Vec<String>) {
    match kind {
        FieldKind::Nested(n) => out.push(n.clone()),
        FieldKind::Option(inner) | FieldKind::List(inner) => nested_deps(inner, out),
        FieldKind::Scalar(_) | FieldKind::Unsupported(_) => {}
    }
}

// ── AST builders (all spans = the derive decl's) ────────────────────────────

struct Cx {
    span: Span,
    counter: usize,
}

impl Cx {
    fn new(span: Span) -> Self {
        Cx { span, counter: 0 }
    }

    fn fresh(&mut self, base: &str) -> String {
        let n = self.counter;
        self.counter += 1;
        format!("__derive_{base}_{n}")
    }

    fn ident(&self, name: &str) -> Expr {
        Expr::Ident(name.to_string(), self.span.clone())
    }

    fn lit_str(&self, s: &str) -> Expr {
        Expr::Literal(Literal::String(s.to_string()), self.span.clone())
    }

    fn lit_int(&self, n: i128) -> Expr {
        Expr::Literal(Literal::Int(n), self.span.clone())
    }

    fn lit_bool(&self, b: bool) -> Expr {
        Expr::Literal(Literal::Bool(b), self.span.clone())
    }

    fn call(&self, name: &str, args: Vec<Expr>) -> Expr {
        Expr::Call(CallExpr {
            callee: Box::new(self.ident(name)),
            args: args
                .into_iter()
                .map(|value| Arg {
                    label: None,
                    value,
                    span: self.span.clone(),
                })
                .collect(),
            trailing_block: None,
            span: self.span.clone(),
        })
    }

    fn member(&self, obj: Expr, field: &str) -> Expr {
        Expr::Member(MemberExpr {
            object: Box::new(obj),
            field: field.to_string(),
            span: self.span.clone(),
        })
    }

    fn add(&self, left: Expr, right: Expr) -> Expr {
        Expr::BinOp(BinOpExpr {
            op: BinOp::Add,
            left: Box::new(left),
            right: Box::new(right),
            span: self.span.clone(),
        })
    }

    fn eq(&self, left: Expr, right: Expr) -> Expr {
        Expr::BinOp(BinOpExpr {
            op: BinOp::Eq,
            left: Box::new(left),
            right: Box::new(right),
            span: self.span.clone(),
        })
    }

    fn lt(&self, left: Expr, right: Expr) -> Expr {
        Expr::BinOp(BinOpExpr {
            op: BinOp::Lt,
            left: Box::new(left),
            right: Box::new(right),
            span: self.span.clone(),
        })
    }

    fn ok(&self, v: Expr) -> Expr {
        self.call("Ok", vec![v])
    }

    fn err(&self, v: Expr) -> Expr {
        self.call("Err", vec![v])
    }

    fn some(&self, v: Expr) -> Expr {
        self.call("Some", vec![v])
    }

    fn none(&self) -> Expr {
        self.ident("None")
    }

    fn ret(&self, v: Expr) -> Stmt {
        Stmt::Return(ReturnStmt {
            value: Some(v),
            span: self.span.clone(),
        })
    }

    fn var(&self, name: String, ty: Option<TypeExpr>, value: Expr) -> Stmt {
        Stmt::Var(VarStmt {
            name,
            ty,
            value,
            span: self.span.clone(),
        })
    }

    fn assign(&self, name: &str, value: Expr) -> Stmt {
        Stmt::Assign(AssignStmt {
            target: self.ident(name),
            op: AssignOp::Eq,
            value,
            span: self.span.clone(),
        })
    }

    fn if_else(&self, cond: Expr, then_block: Vec<Stmt>, else_block: Vec<Stmt>) -> Stmt {
        Stmt::If(IfStmt {
            condition: cond,
            then_block: Block {
                stmts: then_block,
                span: self.span.clone(),
            },
            else_if_clauses: Vec::new(),
            else_block: Some(Block {
                stmts: else_block,
                span: self.span.clone(),
            }),
            span: self.span.clone(),
        })
    }

    fn while_loop(&self, cond: Expr, body: Vec<Stmt>) -> Stmt {
        Stmt::While(WhileStmt {
            condition: cond,
            body: Block {
                stmts: body,
                span: self.span.clone(),
            },
            span: self.span.clone(),
        })
    }

    fn for_loop(&self, binding: String, iterable: Expr, body: Vec<Stmt>) -> Stmt {
        Stmt::For(ForStmt {
            binding,
            iterable,
            body: Block {
                stmts: body,
                span: self.span.clone(),
            },
            span: self.span.clone(),
        })
    }

    fn ok_arm(&self, binding: &str, body: Vec<Stmt>) -> MatchArm {
        MatchArm {
            pattern: Pattern::Variant(
                "Ok".to_string(),
                vec![Pattern::Ident(binding.to_string(), self.span.clone())],
                self.span.clone(),
            ),
            guard: None,
            body: MatchBody::Block(Block {
                stmts: body,
                span: self.span.clone(),
            }),
            span: self.span.clone(),
        }
    }

    fn err_arm(&self, binding: Option<&str>, body: Vec<Stmt>) -> MatchArm {
        let pattern = match binding {
            Some(name) => Pattern::Variant(
                "Err".to_string(),
                vec![Pattern::Ident(name.to_string(), self.span.clone())],
                self.span.clone(),
            ),
            // Static-message sites bind nothing.
            None => Pattern::Variant(
                "Err".to_string(),
                vec![Pattern::Wildcard(self.span.clone())],
                self.span.clone(),
            ),
        };
        MatchArm {
            pattern,
            guard: None,
            body: MatchBody::Block(Block {
                stmts: body,
                span: self.span.clone(),
            }),
            span: self.span.clone(),
        }
    }

    fn match_result(
        &self,
        scrutinee: Expr,
        ok_binding: &str,
        ok_body: Vec<Stmt>,
        err_binding: Option<&str>,
        err_body: Vec<Stmt>,
    ) -> Stmt {
        Stmt::Match(MatchStmt {
            scrutinee,
            arms: vec![
                self.ok_arm(ok_binding, ok_body),
                self.err_arm(err_binding, err_body),
            ],
            span: self.span.clone(),
        })
    }

    /// `match SCRUT { Some(BIND): <body>  None: <body> }` — total by
    /// construction. (The `None` arm is a bare-Ident pattern, which is
    /// exactly how hand-written `None:` arms parse.)
    fn match_option(
        &self,
        scrutinee: Expr,
        some_binding: &str,
        some_body: Vec<Stmt>,
        none_body: Vec<Stmt>,
    ) -> Stmt {
        Stmt::Match(MatchStmt {
            scrutinee,
            arms: vec![
                MatchArm {
                    pattern: Pattern::Variant(
                        "Some".to_string(),
                        vec![Pattern::Ident(some_binding.to_string(), self.span.clone())],
                        self.span.clone(),
                    ),
                    guard: None,
                    body: MatchBody::Block(Block {
                        stmts: some_body,
                        span: self.span.clone(),
                    }),
                    span: self.span.clone(),
                },
                MatchArm {
                    pattern: Pattern::Ident("None".to_string(), self.span.clone()),
                    guard: None,
                    body: MatchBody::Block(Block {
                        stmts: none_body,
                        span: self.span.clone(),
                    }),
                    span: self.span.clone(),
                },
            ],
            span: self.span.clone(),
        })
    }

    /// `"Type.field: " + ERR` — every decode failure names its type
    /// and field (ADR-018), with the accessor's message appended.
    fn field_err(&self, tname: &str, fname: &str, err_ident: &str) -> Expr {
        self.err(self.add(
            self.lit_str(&format!("{tname}.{fname}: ")),
            self.ident(err_ident),
        ))
    }

    /// `"{EXPR}"` — renders any scalar via Display (whole floats
    /// cross as integers on the wire — ADR-018, blessed by
    /// `doc_get_float` coercion on the way back).
    fn interp(&self, e: Expr) -> Expr {
        Expr::StringInterp(StringInterpExpr {
            parts: vec![InterpPart::Expr(e)],
            span: self.span.clone(),
        })
    }

    fn json_doc_lit(&self, id_expr: Expr) -> Expr {
        Expr::StructLit(StructLitExpr {
            name: "JsonDoc".to_string(),
            fields: vec![StructField::Named("id".to_string(), id_expr)],
            span: self.span.clone(),
        })
    }
}

// ── Encode (to_json) ────────────────────────────────────────────────────────

/// Encode expression for an inline value (direct fields and option
/// payloads). Lists need loops and never reach this path.
fn encode_value(cx: &Cx, value: Expr, kind: &FieldKind) -> Expr {
    match kind {
        FieldKind::Scalar(Scalar::String) => cx.call("json_quote_builtin", vec![value]),
        FieldKind::Scalar(Scalar::Char) => cx.call("json_quote_builtin", vec![cx.interp(value)]),
        FieldKind::Scalar(_) => cx.interp(value),
        FieldKind::Nested(inner) => cx.call(&format!("to_json_{inner}"), vec![value]),
        _ => cx.call(
            "panic",
            vec![cx.lit_str("derive internal error: non-inline field in expression position")],
        ),
    }
}

/// Element encode for list bodies (options rejected up front).
fn encode_elem(cx: &Cx, elem: Expr, kind: &FieldKind) -> Expr {
    match kind {
        FieldKind::Scalar(Scalar::String) => cx.call("json_quote_builtin", vec![elem]),
        FieldKind::Scalar(Scalar::Char) => cx.call("json_quote_builtin", vec![cx.interp(elem)]),
        FieldKind::Scalar(_) => cx.interp(elem),
        FieldKind::Nested(inner) => cx.call(&format!("to_json_{inner}"), vec![elem]),
        _ => cx.call(
            "panic",
            vec![cx.lit_str("derive internal error: non-inline element in expression position")],
        ),
    }
}

/// Statements encoding a list value: loop into a bracketed,
/// comma-separated accumulator, then append the `"key":<arr>` pair
/// to `out`. Used for direct list fields and `Some` payloads alike.
fn emit_list_encode(
    cx: &mut Cx,
    access: Expr,
    elem: &FieldKind,
    key: Expr,
) -> Vec<Stmt> {
    let arr = cx.fresh("arr");
    let flag = cx.fresh("first");
    let x = cx.fresh("x");
    let loop_body = vec![
        cx.if_else(
            cx.ident(&flag),
            vec![cx.assign(&flag, cx.lit_bool(false))],
            vec![cx.assign(&arr, cx.add(cx.ident(&arr), cx.lit_str(",")))],
        ),
        cx.assign(&arr, cx.add(cx.ident(&arr), encode_elem(cx, cx.ident(&x), elem))),
    ];
    vec![
        cx.var(arr.clone(), None, cx.lit_str("[")),
        cx.var(flag.clone(), None, cx.lit_bool(true)),
        cx.for_loop(x, access, loop_body),
        cx.assign(
            "out",
            cx.add(
                cx.ident("out"),
                cx.add(key, cx.add(cx.ident(&arr), cx.lit_str("]"))),
            ),
        ),
    ]
}

fn build_to_json(tname: &str, fields: &[(String, FieldKind)], span: &Span) -> FunctionDecl {
    let mut cx = Cx::new(span.clone());
    let mut stmts: Vec<Stmt> = vec![
        cx.var("out".to_string(), None, cx.lit_str("{")),
        cx.var("first".to_string(), None, cx.lit_bool(true)),
    ];
    // Separator: first pair bare, the rest comma-led, so no trailing
    // comma can ever form.
    for (fname, kind) in fields {
        let access = cx.member(cx.ident("v"), fname);
        let key = cx.lit_str(&format!("\"{fname}\":"));
        stmts.push(cx.if_else(
            cx.ident("first"),
            vec![cx.assign("first", cx.lit_bool(false))],
            vec![cx.assign("out", cx.add(cx.ident("out"), cx.lit_str(",")))],
        ));
        match kind {
            FieldKind::Option(inner) if option_arg_ok(inner) => {
                let t = cx.fresh("opt");
                let t_expr = cx.ident(&t);
                let pair_some = match &**inner {
                    // Lists need their loop even inside `Some`.
                    FieldKind::List(elem) if list_elem_ok(elem) => {
                        emit_list_encode(&mut cx, t_expr, elem, key.clone())
                    }
                    _ => vec![cx.assign(
                        "out",
                        cx.add(
                            cx.ident("out"),
                            cx.add(key.clone(), encode_value(&cx, t_expr, inner)),
                        ),
                    )],
                };
                let pair_none = cx.assign(
                    "out",
                    cx.add(cx.ident("out"), cx.add(key, cx.lit_str("null"))),
                );
                stmts.push(Stmt::Match(MatchStmt {
                    scrutinee: access,
                    arms: vec![
                        MatchArm {
                            pattern: Pattern::Variant(
                                "Some".to_string(),
                                vec![Pattern::Ident(t, cx.span.clone())],
                                cx.span.clone(),
                            ),
                            guard: None,
                            body: MatchBody::Block(Block {
                                stmts: pair_some,
                                span: cx.span.clone(),
                            }),
                            span: cx.span.clone(),
                        },
                        MatchArm {
                            pattern: Pattern::Ident("None".to_string(), cx.span.clone()),
                            guard: None,
                            body: MatchBody::Block(Block {
                                stmts: vec![pair_none],
                                span: cx.span.clone(),
                            }),
                            span: cx.span.clone(),
                        },
                    ],
                    span: cx.span.clone(),
                }));
            }
            FieldKind::List(inner) if list_elem_ok(inner) => {
                let mut list_stmts = emit_list_encode(&mut cx, access, inner, key);
                stmts.append(&mut list_stmts);
            }
            _ => {
                stmts.push(cx.assign(
                    "out",
                    cx.add(cx.ident("out"), cx.add(key, encode_value(&cx, access, kind))),
                ));
            }
        }
    }
    stmts.push(cx.assign("out", cx.add(cx.ident("out"), cx.lit_str("}"))));
    stmts.push(Stmt::Expr(cx.ident("out")));
    FunctionDecl {
        name: format!("to_json_{tname}"),
        generic_params: Vec::new(),
        params: vec![Param {
            name: "v".to_string(),
            ty: TypeExpr::Named(tname.to_string(), Vec::new(), span.clone()),
            default: None,
            span: span.clone(),
        }],
        return_ty: Some(TypeExpr::Named("String".to_string(), Vec::new(), span.clone())),
        body: FunctionBody::Block(Block {
            stmts,
            span: span.clone(),
        }),
        span: span.clone(),
    }
}

// ── Decode (from_json) ──────────────────────────────────────────────────────
//
// Flat temp sequence for scalars/options/lists (failures `return
// Err` early), then a nesting pyramid for required nested structs
// (which have no default value to pre-declare), with the struct
// literal innermost.

/// `doc_get_<X>_builtin(doc.id, "field")`.
fn accessor(cx: &Cx, builtin: &str, fname: &str) -> Expr {
    cx.call(
        builtin,
        vec![cx.member(cx.ident("doc"), "id"), cx.lit_str(fname)],
    )
}

fn scalar_accessor(kind: &Scalar) -> &'static str {
    match kind {
        Scalar::Int => "doc_get_int_builtin",
        Scalar::Float => "doc_get_float_builtin",
        Scalar::Bool => "doc_get_bool_builtin",
        Scalar::Char => "doc_get_char_builtin",
        Scalar::String => "doc_get_string_builtin",
    }
}

fn scalar_default(kind: &Scalar, cx: &Cx) -> Expr {
    match kind {
        Scalar::Int => cx.lit_int(0),
        Scalar::Float => Expr::Literal(Literal::Float(0.0), cx.span.clone()),
        Scalar::Bool => cx.lit_bool(false),
        Scalar::Char => Expr::Literal(Literal::Char(' '), cx.span.clone()),
        Scalar::String => cx.lit_str(""),
    }
}

/// Element decode statements inside a list loop: decode `__t` and
/// append to `acc`, or `return Err` naming the field. Elements are
/// always strict (a present element with the wrong shape fails).
fn decode_elem(
    cx: &mut Cx,
    tname: &str,
    fname: &str,
    elem: &FieldKind,
    acc: &str,
    t: &str,
) -> Vec<Stmt> {
    let append =
        |cx: &Cx, v: Expr| cx.assign(acc, cx.call("list_append_builtin", vec![cx.ident(acc), v]));
    // Element failures are static strings (the enclosing Err arm
    // binds no value): still naming `Type.field`, per ADR-018.
    let bad = |cx: &Cx, what: &str| {
        cx.ret(cx.err(cx.lit_str(&format!("{tname}.{fname}: element is not {what}"))))
    };
    match elem {
        FieldKind::Scalar(Scalar::String) => {
            // Index elements arrive unquoted (doc_get_index_builtin).
            vec![append(cx, cx.ident(t))]
        }
        FieldKind::Scalar(Scalar::Int) => {
            let e = cx.fresh("e");
            vec![cx.match_option(
                cx.call("to_int", vec![cx.ident(t)]),
                &e,
                vec![append(cx, cx.ident(&e))],
                vec![bad(cx, "an int")],
            )]
        }
        FieldKind::Scalar(Scalar::Float) => {
            let e = cx.fresh("e");
            vec![cx.match_option(
                cx.call("to_float", vec![cx.ident(t)]),
                &e,
                vec![append(cx, cx.ident(&e))],
                vec![bad(cx, "a float")],
            )]
        }
        FieldKind::Scalar(Scalar::Bool) => {
            let on_true = vec![append(cx, cx.lit_bool(true))];
            let on_false = vec![append(cx, cx.lit_bool(false))];
            vec![cx.if_else(
                cx.eq(cx.ident(t), cx.lit_str("true")),
                on_true,
                vec![cx.if_else(
                    cx.eq(cx.ident(t), cx.lit_str("false")),
                    on_false,
                    vec![bad(cx, "a bool")],
                )],
            )]
        }
        FieldKind::Scalar(Scalar::Char) => {
            // One-character text decodes; anything else is loud.
            vec![cx.if_else(
                cx.eq(cx.member(cx.ident(t), "length"), cx.lit_int(1)),
                vec![append(
                    cx,
                    Expr::Index(IndexExpr {
                        object: Box::new(cx.ident(t)),
                        index: Box::new(cx.lit_int(0)),
                        span: cx.span.clone(),
                    }),
                )],
                vec![bad(cx, "a char")],
            )]
        }
        FieldKind::Nested(sub) => {
            let h = cx.fresh("h");
            let e = cx.fresh("e");
            let e2 = e.clone();
            let sub_call = cx.call(
                &format!("from_json_{sub}"),
                vec![cx.json_doc_lit(cx.ident(&h))],
            );
            vec![cx.match_result(
                cx.call("doc_parse_builtin", vec![cx.ident(t)]),
                &h,
                vec![cx.match_result(
                    sub_call,
                    &e,
                    vec![append(cx, cx.ident(&e2))],
                    Some(&e2),
                    vec![cx.ret(cx.field_err(tname, fname, &e2))],
                )],
                None,
                vec![bad(cx, "an object")],
            )]
        }
        _ => Vec::new(),
    }
}

/// The shared list-decode core: length probe + indexed loop appending
/// to `acc`. Callers wrap it in the `doc_get_doc` match and decide
/// where the accumulator lands.
fn decode_list_body(
    cx: &mut Cx,
    tname: &str,
    fname: &str,
    elem: &FieldKind,
    arr: &str,
    acc: &str,
) -> Vec<Stmt> {
    let n = cx.fresh("n");
    let i = cx.fresh("i");
    let t = cx.fresh("t");
    let e = cx.fresh("e");
    let mut while_body = decode_elem(cx, tname, fname, elem, acc, &t);
    while_body.push(cx.assign(&i, cx.add(cx.ident(&i), cx.lit_int(1))));
    vec![cx.match_result(
        cx.call("doc_len_builtin", vec![cx.ident(arr)]),
        &n,
        {
            let mut stmts = vec![cx.var(i.clone(), None, cx.lit_int(0))];
            stmts.push(cx.while_loop(
                cx.lt(cx.ident(&i), cx.ident(&n)),
                vec![cx.match_result(
                    cx.call(
                        "doc_get_index_builtin",
                        vec![cx.ident(arr), cx.ident(&i)],
                    ),
                    &t,
                    while_body,
                    Some(&e),
                    vec![cx.ret(cx.field_err(tname, fname, &e))],
                )],
            ));
            stmts
        },
        Some(&e),
        vec![cx.ret(cx.field_err(tname, fname, &e))],
    )]
}

/// Statements decoding a PRESENT option value into `tmp`
/// (`None`-on-absence is handled by the caller).
fn decode_present(
    cx: &mut Cx,
    tname: &str,
    fname: &str,
    inner: &FieldKind,
    tmp: &str,
) -> Vec<Stmt> {
    let e = cx.fresh("e");
    match inner {
        // Present-but-unreadable scalars (including explicit null)
        // decode to None — total, documented.
        FieldKind::Scalar(s) => {
            let t = cx.fresh("t");
            vec![cx.match_result(
                accessor(cx, scalar_accessor(s), fname),
                &t,
                vec![cx.assign(tmp, cx.some(cx.ident(&t)))],
                None,
                vec![cx.assign(tmp, cx.none())],
            )]
        }
        // Present composites are strict: a present object/array with
        // bad interior propagates the error.
        FieldKind::Nested(sub) => {
            let d = cx.fresh("doc");
            let t = cx.fresh("t");
            vec![cx.match_result(
                cx.call(
                    "doc_get_doc_builtin",
                    vec![cx.member(cx.ident("doc"), "id"), cx.lit_str(fname)],
                ),
                &d,
                vec![cx.match_result(
                    cx.call(
                        &format!("from_json_{sub}"),
                        vec![cx.json_doc_lit(cx.ident(&d))],
                    ),
                    &t,
                    vec![cx.assign(tmp, cx.some(cx.ident(&t)))],
                    Some(&e),
                    vec![cx.ret(cx.field_err(tname, fname, &e))],
                )],
                None,
                vec![cx.assign(tmp, cx.none())],
            )]
        }
        FieldKind::List(elem) if list_elem_ok(elem) => {
            let ety = kind_type(elem, &cx.span.clone()).expect("checked kind");
            let arr = cx.fresh("arr");
            let acc = cx.fresh("acc");
            let mut ok_stmts = vec![cx.var(
                acc.clone(),
                Some(TypeExpr::Collection(Box::new(ety), cx.span.clone())),
                Expr::ListLit(ListLitExpr {
                    elements: Vec::new(),
                    span: cx.span.clone(),
                }),
            )];
            ok_stmts.extend(decode_list_body(cx, tname, fname, elem, &arr, &acc));
            ok_stmts.push(cx.assign(tmp, cx.some(cx.ident(&acc))));
            vec![cx.match_result(
                cx.call(
                    "doc_get_doc_builtin",
                    vec![cx.member(cx.ident("doc"), "id"), cx.lit_str(fname)],
                ),
                &arr,
                ok_stmts,
                None,
                vec![cx.assign(tmp, cx.none())],
            )]
        }
        _ => Vec::new(),
    }
}

/// Flat decode of one non-nested field into temp `tmp`.
fn decode_flat(
    cx: &mut Cx,
    tname: &str,
    fname: &str,
    kind: &FieldKind,
    tmp: &str,
    stmts: &mut Vec<Stmt>,
) {
    let e = cx.fresh("e");
    match kind {
        FieldKind::Scalar(s) => {
            stmts.push(cx.var(tmp.to_string(), None, scalar_default(s, cx)));
            let t = cx.fresh("t");
            stmts.push(cx.match_result(
                accessor(cx, scalar_accessor(s), fname),
                &t,
                vec![cx.assign(tmp, cx.ident(&t))],
                Some(&e),
                vec![cx.ret(cx.field_err(tname, fname, &e))],
            ));
        }
        FieldKind::Option(inner) if option_arg_ok(inner) => {
            stmts.push(cx.var(tmp.to_string(), None, cx.none()));
            let present = decode_present(cx, tname, fname, inner, tmp);
            stmts.push(cx.if_else(
                cx.call(
                    "doc_has_builtin",
                    vec![cx.member(cx.ident("doc"), "id"), cx.lit_str(fname)],
                ),
                present,
                vec![cx.assign(tmp, cx.none())],
            ));
        }
        FieldKind::List(inner) if list_elem_ok(inner) => {
            let ety = kind_type(inner, &cx.span.clone()).expect("checked kind");
            let arr = cx.fresh("arr");
            stmts.push(cx.var(
                tmp.to_string(),
                Some(TypeExpr::Collection(Box::new(ety), cx.span.clone())),
                Expr::ListLit(ListLitExpr {
                    elements: Vec::new(),
                    span: cx.span.clone(),
                }),
            ));
            let list_body = decode_list_body(cx, tname, fname, inner, &arr, tmp);
            stmts.push(cx.match_result(
                cx.call(
                    "doc_get_doc_builtin",
                    vec![cx.member(cx.ident("doc"), "id"), cx.lit_str(fname)],
                ),
                &arr,
                list_body,
                Some(&e),
                vec![cx.ret(cx.field_err(tname, fname, &e))],
            ));
        }
        _ => {}
    }
}

fn build_from_json(tname: &str, fields: &[(String, FieldKind)], span: &Span) -> FunctionDecl {
    let mut cx = Cx::new(span.clone());
    // One temp per field (bindings for the final literal).
    let mut tmps: HashMap<String, String> = HashMap::new();
    for (fname, _) in fields {
        tmps.insert(fname.clone(), cx.fresh("v"));
    }
    let mut flat: Vec<Stmt> = Vec::new();
    let mut nested: Vec<(String, String)> = Vec::new();
    for (fname, kind) in fields {
        let tmp = tmps[fname].clone();
        match kind {
            FieldKind::Nested(sub) => nested.push((fname.clone(), sub.clone())),
            _ => decode_flat(&mut cx, tname, fname, kind, &tmp, &mut flat),
        }
    }
    // Struct literal in declaration order.
    let lit = Expr::StructLit(StructLitExpr {
        name: tname.to_string(),
        fields: fields
            .iter()
            .map(|(fname, _)| StructField::Named(fname.clone(), cx.ident(&tmps[fname])))
            .collect(),
        span: span.clone(),
    });
    // Nest required structs; the literal sits innermost.
    let mut tail: Vec<Stmt> = vec![cx.ret(cx.ok(lit))];
    for (fname, sub) in nested.iter().rev() {
        let d = cx.fresh("doc");
        let e_inner = cx.fresh("e");
        let e_outer = cx.fresh("e");
        let tmp = tmps[fname].clone();
        tail = vec![cx.match_result(
            cx.call(
                "doc_get_doc_builtin",
                vec![cx.member(cx.ident("doc"), "id"), cx.lit_str(fname)],
            ),
            &d,
            vec![cx.match_result(
                cx.call(
                    &format!("from_json_{sub}"),
                    vec![cx.json_doc_lit(cx.ident(&d))],
                ),
                &tmp,
                tail,
                Some(&e_inner),
                vec![cx.ret(cx.field_err(tname, fname, &e_inner))],
            )],
            Some(&e_outer),
            vec![cx.ret(cx.field_err(tname, fname, &e_outer))],
        )];
    }
    flat.extend(tail);
    FunctionDecl {
        name: format!("from_json_{tname}"),
        generic_params: Vec::new(),
        params: vec![Param {
            name: "doc".to_string(),
            ty: TypeExpr::Named("JsonDoc".to_string(), Vec::new(), span.clone()),
            default: None,
            span: span.clone(),
        }],
        return_ty: Some(TypeExpr::Named(
            "Result".to_string(),
            vec![
                TypeExpr::Named(tname.to_string(), Vec::new(), span.clone()),
                TypeExpr::Named("String".to_string(), Vec::new(), span.clone()),
            ],
            span.clone(),
        )),
        body: FunctionBody::Block(Block {
            stmts: flat,
            span: span.clone(),
        }),
        span: span.clone(),
    }
}

fn marker_impl(trait_name: &str, target: &str, span: &Span) -> Item {
    Item::Impl(ImplBlock {
        ty: TypeExpr::Named(target.to_string(), Vec::new(), span.clone()),
        for_trait: Some(TypeExpr::Named(trait_name.to_string(), Vec::new(), span.clone())),
        methods: Vec::new(),
        span: span.clone(),
    })
}

// ── Top-level driver ────────────────────────────────────────────────────────

struct ValidDerive {
    trait_name: String,
    target: String,
    fields: Vec<(String, FieldKind)>,
    span: Span,
}

fn check_kind(
    kind: &FieldKind,
    tname: &str,
    fname: &str,
    dir_trait: &str,
    dir_set: &HashSet<String>,
    structs: &HashSet<String>,
    sink: &mut DiagnosticSink,
    span: &Span,
) -> bool {
    match kind {
        FieldKind::Scalar(_) => true,
        FieldKind::Nested(x) => {
            if !structs.contains(x) {
                sink.emit(
                    Diagnostic::error(format!(
                        "unsupported derive field `{tname}.{fname}`: `{x}` is not a struct"
                    ))
                    .with_span(span.clone(), "here")
                    .with_code("E0325"),
                );
                false
            } else if !dir_set.contains(x) {
                sink.emit(
                    Diagnostic::error(format!(
                        "cannot derive {dir_trait} for `{tname}`: field `{fname}` has type `{x}`, which does not derive {dir_trait}"
                    ))
                    .with_span(span.clone(), "here")
                    .with_code("E0326"),
                );
                false
            } else {
                true
            }
        }
        FieldKind::Option(inner) => {
            if !option_arg_ok(inner) {
                sink.emit(
                    Diagnostic::error(format!(
                        "unsupported derive field `{tname}.{fname}`: nested options are not supported"
                    ))
                    .with_span(span.clone(), "here")
                    .with_code("E0325"),
                );
                false
            } else {
                check_kind(inner, tname, fname, dir_trait, dir_set, structs, sink, span)
            }
        }
        FieldKind::List(inner) => {
            if !list_elem_ok(inner) {
                sink.emit(
                    Diagnostic::error(format!(
                        "unsupported derive field `{tname}.{fname}`: only scalars and nested structs decode as list elements"
                    ))
                    .with_span(span.clone(), "here")
                    .with_code("E0325"),
                );
                false
            } else {
                check_kind(inner, tname, fname, dir_trait, dir_set, structs, sink, span)
            }
        }
        FieldKind::Unsupported(desc) => {
            sink.emit(
                Diagnostic::error(format!(
                    "unsupported derive field `{tname}.{fname}`: `{desc}` has no JSON mapping"
                ))
                .with_span(span.clone(), "here")
                .with_code("E0325"),
            );
            false
        }
    }
}

/// Expand all `Item::Derive`s. Invalid derives emit diagnostics and
/// are dropped; valid ones append synthesized functions + a marker
/// impl, and the `Derive` items themselves are consumed (downstream
/// stages only ever see ordinary items).
pub fn expand_derives(program: Program, sink: &mut DiagnosticSink) -> Program {
    // Tables over the classified program.
    let mut structs: HashMap<String, (Vec<FieldDecl>, Span, bool)> = HashMap::new();
    let mut struct_names: HashSet<String> = HashSet::new();
    let mut enums: HashSet<String> = HashSet::new();
    let mut bare: HashSet<String> = HashSet::new();
    let mut traits: HashSet<String> = HashSet::new();
    let mut derives: Vec<DeriveDecl> = Vec::new();
    for item in &program.items {
        match item {
            Item::Struct(s) => {
                struct_names.insert(s.name.clone());
                structs.insert(
                    s.name.clone(),
                    (s.fields.clone(), s.span.clone(), !s.generic_params.is_empty()),
                );
            }
            Item::Enum(e) => {
                enums.insert(e.name.clone());
            }
            Item::Trait(t) => {
                traits.insert(t.name.clone());
            }
            Item::BareDecl(d) => {
                bare.insert(d.name.clone());
            }
            Item::Derive(d) => derives.push(d.clone()),
            _ => {}
        }
    }

    // Phase A: validate each derive (trait, target, duplicates).
    let mut ser_valid: HashSet<String> = HashSet::new();
    let mut deser_valid: HashSet<String> = HashSet::new();
    let mut valid: Vec<ValidDerive> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for d in &derives {
        if d.trait_name != SERIALIZE && d.trait_name != DESERIALIZE {
            sink.emit(
                Diagnostic::error(format!(
                    "unknown derive trait `{}`: only `Serialize` and `Deserialize` can be derived",
                    d.trait_name
                ))
                .with_span(d.span.clone(), "here")
                .with_code("E0320"),
            );
            continue;
        }
        if enums.contains(&d.target) {
            sink.emit(
                Diagnostic::error(format!(
                    "cannot derive {} for `{}`: enum targets are not supported yet",
                    d.trait_name, d.target
                ))
                .with_span(d.span.clone(), "here")
                .with_code("E0321"),
            );
            continue;
        }
        let (fields, _span, generic) = match structs.get(&d.target) {
            Some(entry) => entry,
            None => {
                let hint = if bare.contains(&d.target) {
                    format!("`{}` is not a struct (unclassified declaration)", d.target)
                } else {
                    format!("unknown type `{}`", d.target)
                };
                sink.emit(
                    Diagnostic::error(format!(
                        "cannot derive {}: {hint}",
                        d.trait_name
                    ))
                    .with_span(d.span.clone(), "here")
                    .with_code("E0322"),
                );
                continue;
            }
        };
        if *generic {
            sink.emit(
                Diagnostic::error(format!(
                    "cannot derive {} for `{}`: generic structs are not supported yet",
                    d.trait_name, d.target
                ))
                .with_span(d.span.clone(), "here")
                .with_code("E0323"),
            );
            continue;
        }
        if !seen.insert((d.trait_name.clone(), d.target.clone())) {
            sink.emit(
                Diagnostic::error(format!(
                    "duplicate derive: {} for `{}` is already derived",
                    d.trait_name, d.target
                ))
                .with_span(d.span.clone(), "here")
                .with_code("E0324"),
            );
            continue;
        }
        // Scope requirements (ADR-018): the marker trait must be
        // declared (`encoding/json/serialize.nv`), and Deserialize
        // additionally needs `JsonDoc` (`encoding/json/document.nv`).
        // Loud here, naming the file — not a bare `trait not found`
        // or a silent `Unknown` downstream.
        if !traits.contains(&d.trait_name) {
            sink.emit(
                Diagnostic::error(format!(
                    "cannot derive {} for `{}`: trait `{}` is not in scope (include encoding/json/serialize.nv)",
                    d.trait_name, d.target, d.trait_name
                ))
                .with_span(d.span.clone(), "here")
                .with_code("E0328"),
            );
            continue;
        }
        if d.trait_name == DESERIALIZE && !struct_names.contains("JsonDoc") {
            sink.emit(
                Diagnostic::error(format!(
                    "cannot derive Deserialize for `{}`: type `JsonDoc` is not in scope (include encoding/json/document.nv)",
                    d.target
                ))
                .with_span(d.span.clone(), "here")
                .with_code("E0328"),
            );
            continue;
        }
        if d.trait_name == SERIALIZE {
            ser_valid.insert(d.target.clone());
        } else {
            deser_valid.insert(d.target.clone());
        }
        valid.push(ValidDerive {
            trait_name: d.trait_name.clone(),
            target: d.target.clone(),
            fields: fields
                .iter()
                .map(|f| (f.name.clone(), classify(&f.ty)))
                .collect(),
            span: d.span.clone(),
        });
    }

    // Phase B: check field coverage per derive.
    let mut failed: HashSet<(String, String)> = HashSet::new();
    for v in &valid {
        let dir_set = if v.trait_name == SERIALIZE {
            &ser_valid
        } else {
            &deser_valid
        };
        let mut ok = true;
        for (fname, kind) in &v.fields {
            if !check_kind(kind, &v.target, fname, &v.trait_name, dir_set, &struct_names, sink, &v.span) {
                ok = false;
            }
        }
        if !ok {
            failed.insert((v.trait_name.clone(), v.target.clone()));
        }
    }

    // Phase B2: chained failures — a derive whose (transitive) nested
    // dependencies failed cannot synthesize callable code. Iterate to
    // a fixpoint so chains of any length report once each.
    loop {
        let mut progressed = false;
        for v in &valid {
            let key = (v.trait_name.clone(), v.target.clone());
            if failed.contains(&key) {
                continue;
            }
            let mut deps = Vec::new();
            for (_, kind) in &v.fields {
                nested_deps(kind, &mut deps);
            }
            if let Some(dep) = deps
                .iter()
                .find(|d| failed.contains(&(v.trait_name.clone(), (*d).clone())))
            {
                sink.emit(
                    Diagnostic::error(format!(
                        "cannot derive {} for `{}`: field type `{dep}` failed to derive (see above)",
                        v.trait_name, v.target
                    ))
                    .with_span(v.span.clone(), "here")
                    .with_code("E0327"),
                );
                failed.insert(key);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    // Phase C: synthesize survivors in file order; consume derives.
    let mut items: Vec<Item> = program
        .items
        .into_iter()
        .filter(|item| !matches!(item, Item::Derive(_)))
        .collect();
    for v in &valid {
        if failed.contains(&(v.trait_name.clone(), v.target.clone())) {
            continue;
        }
        if v.trait_name == SERIALIZE {
            items.push(Item::Function(build_to_json(&v.target, &v.fields, &v.span)));
        } else {
            items.push(Item::Function(build_from_json(&v.target, &v.fields, &v.span)));
        }
        items.push(marker_impl(&v.trait_name, &v.target, &v.span));
    }
    Program {
        imports: program.imports,
        items,
    }
}
