//! Typed HIR item definitions — the output of the type checker.
//!
//! Every expression and binding carries a resolved [`Ty`]; every identifier
//! has been confirmed to exist in some scope (or `Ty::Error` has been emitted).

use super::types::Ty;

// ── Module ────────────────────────────────────────────────────────────────────

/// A type-checked, resolved module.  This is the top-level output of
/// [`crate::typeck::typecheck`].
#[derive(Debug, Clone)]
pub struct Module {
    pub structs: Vec<Struct>,
    pub enums: Vec<Enum>,
    pub traits: Vec<Trait>,
    pub functions: Vec<Function>,
}

impl Module {
    pub fn empty() -> Self {
        Module { structs: Vec::new(), enums: Vec::new(), traits: Vec::new(), functions: Vec::new() }
    }
}

// ── Nominal types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Struct {
    pub name: String,
    /// `(field_name, field_type)` pairs in declaration order.
    pub fields: Vec<(String, Ty)>,
}

#[derive(Debug, Clone)]
pub struct Enum {
    pub name: String,
    /// `(variant_name, payload_types)` pairs.
    pub variants: Vec<(String, Vec<Ty>)>,
}

#[derive(Debug, Clone)]
pub struct Trait {
    pub name: String,
    /// `(method_name, (params, return_ty))` pairs.
    pub methods: Vec<(String, (Vec<Ty>, Ty))>,
}

// ── Functions ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub return_ty: Ty,
    pub body: Vec<TypedStmt>,
}

// ── Statements ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypedStmt {
    pub kind: TypedStmtKind,
    /// The statement's "type" — generally `Unit` for statements that produce
    /// no value, but used for expression-statements and for diagnostic recovery.
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum TypedStmtKind {
    /// `let name [: T] = value`
    Let { name: String, ty: Ty, value: TypedExpr },
    /// `var name [: T] = value`
    Var { name: String, ty: Ty, value: TypedExpr },
    /// `name: value` (bare declaration without type annotation)
    Decl { name: String, ty: Ty, value: TypedExpr },
    /// A bare expression statement.
    Expr(TypedExpr),
    /// `return [expr]`
    Return(Option<TypedExpr>),
    /// `name = value` (reassignment)
    Assign { target: TypedExpr, value: TypedExpr },
    /// `if … { … } [else { … }]`
    If {
        condition: TypedExpr,
        then_body: Vec<TypedStmt>,
        else_body: Option<Vec<TypedStmt>>,
    },
    /// `while condition { … }`
    While { condition: TypedExpr, body: Vec<TypedStmt> },
    /// `loop { … }`
    Loop { body: Vec<TypedStmt> },
    /// `for binding in iterable { … }`
    For { binding: String, iterable: TypedExpr, body: Vec<TypedStmt> },
    /// `match scrutinee { … }`
    Match { scrutinee: TypedExpr, arms: Vec<TypedArm> },
}

// ── Match arms ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypedArm {
    pub pattern: TypedPattern,
    pub guard: Option<TypedExpr>,
    pub body: Vec<TypedStmt>,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum TypedPattern {
    Wildcard,
    Ident(String),
    Literal(TypedLit),
    Variant(String, Vec<TypedPattern>),
}

#[derive(Debug, Clone)]
pub enum TypedLit {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
}

// ── Expressions ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypedExpr {
    pub kind: TypedExprKind,
    /// The resolved type of this expression.
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum TypedExprKind {
    IntLit(i128),
    FloatLit(f64),
    BoolLit(bool),
    CharLit(char),
    StringLit(String),
    /// `None` literal.
    None,
    /// `Some(expr)` constructor.
    Some(Box<TypedExpr>),
    /// `Ok(expr)` constructor.
    Ok(Box<TypedExpr>),
    /// `Err(expr)` constructor.
    Err(Box<TypedExpr>),
    /// Resolved identifier.
    Ident(String),
    Call {
        callee: Box<TypedExpr>,
        args: Vec<TypedExpr>,
    },
    BinOp {
        op: crate::ast::BinOp,
        left: Box<TypedExpr>,
        right: Box<TypedExpr>,
    },
    UnaryOp {
        op: crate::ast::UnaryOp,
        operand: Box<TypedExpr>,
    },
    Member {
        object: Box<TypedExpr>,
        field: String,
    },
    /// `expr?` — propagates `Result`/`Option` errors.
    Try(Box<TypedExpr>),
    /// `a ?? b` — coalescing for `Option`.
    Coalesce {
        left: Box<TypedExpr>,
        right: Box<TypedExpr>,
    },
    /// String interpolation — always has type `String`.
    StringInterp(Vec<TypedInterpPart>),
    /// Tuple literal.
    Tuple(Vec<TypedExpr>),
    /// List literal `[e1, e2, …]`.
    List(Vec<TypedExpr>),
    /// Index expression `a[i]`.
    Index {
        object: Box<TypedExpr>,
        index: Box<TypedExpr>,
    },
    /// Range expression `a..b` or `a..=b`.
    Range {
        start: Box<TypedExpr>,
        end: Box<TypedExpr>,
        inclusive: bool,
    },
    /// `if cond { e1 } else { e2 }` used as an expression.
    IfExpr {
        condition: Box<TypedExpr>,
        then_expr: Box<TypedExpr>,
        else_expr: Option<Box<TypedExpr>>,
    },
    /// A match expression (when used in expression position).
    MatchExpr {
        scrutinee: Box<TypedExpr>,
        arms: Vec<TypedArm>,
    },
    /// Enum variant constructor, e.g. `Direction::North` (resolved).
    EnumVariant { enum_name: String, variant: String },
    /// Struct literal `Name { field: val, … }`.
    StructLit { name: String, fields: Vec<(String, TypedExpr)> },
    /// Spread expression `...expr`
    Spread(Box<TypedExpr>),
    /// Closure: `|params| body`
    Closure { params: Vec<String>, body: Box<TypedExpr> },
}

#[derive(Debug, Clone)]
pub enum TypedInterpPart {
    Literal(String),
    Expr(TypedExpr),
}
