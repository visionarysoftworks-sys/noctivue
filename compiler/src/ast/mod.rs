//! Abstract Syntax Tree (AST) node types for Noctivue.
//!
//! The AST is the output of the parser and the input to the resolver.
//! It is **unresolved** — identifiers are not yet bound to their definitions,
//! and bare declarations (`BareDecl`) have not yet been classified as structs,
//! functions, or components.
//!
//! After name resolution (`resolver`) the AST is annotated and bare declarations
//! are replaced by their classified counterparts. After type checking (`typeck`)
//! the tree is lowered to HIR.
//!
//! ## Three density levels, one AST (ADR-008)
//!
//! Minimal, compact, and expanded surface forms all produce the same AST nodes.
//! The parser never distinguishes between them at the AST level; formatting
//! decisions are entirely the formatter's responsibility.

use crate::diagnostics::Span;

// ── Top-level ────────────────────────────────────────────────────────────────

/// A complete Noctivue source file.
#[derive(Debug, Clone)]
pub struct Program {
    pub imports: Vec<ImportDecl>,
    pub items: Vec<Item>,
}

impl Program {
    /// Returns an empty program (used as a stub before the parser is implemented).
    pub fn empty() -> Self {
        Program { imports: Vec::new(), items: Vec::new() }
    }
}

/// `import path [as name]`
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub span: Span,
}

// ── Top-level items ───────────────────────────────────────────────────────────

/// A top-level item in a source file.
#[derive(Debug, Clone)]
pub enum Item {
    /// Unclassified colon-block declaration.
    ///
    /// Produced by the parser when a declaration begins with a bare
    /// `Identifier [(params)] [-> Type] :` and has not yet been classified by
    /// the resolver. See COMPILER_ARCHITECTURE.md §4.
    BareDecl(BareDecl),

    /// Explicitly-keyworded function declaration (`fn …`).
    Function(FunctionDecl),
    /// Explicitly-keyworded struct declaration (`struct …`).
    Struct(StructDecl),
    /// Enum declaration (`enum …`). Always explicit.
    Enum(EnumDecl),
    /// Trait declaration (`trait …`). Always explicit.
    Trait(TraitDecl),
    /// Impl block (`impl …`). Always explicit.
    Impl(ImplBlock),
    /// Constant declaration (`const …`). Always explicit.
    Const(ConstDecl),
    /// Re-export (`export …`).
    Export(Box<Item>),
}

// ── Bare declaration (the key Phase 0 node) ───────────────────────────────────

/// An unclassified colon-block declaration.
///
/// The parser emits this for any `Identifier [(params)] [-> Type] :` block that
/// does not start with an explicit declaration keyword. The resolver will
/// classify it as one of:
///
/// - [`StructDecl`] — body is exclusively field declarations.
/// - [`FunctionDecl`] — return type present, or body contains statements.
/// - A component / builder call — body consists of nested calls that resolve to
///   functions or macros rather than types.
///
/// Classification is structural (body shape), never based on naming convention.
/// A cycle in the classification dependencies MUST produce a diagnostic
/// (`"declaration classification cycle"`) rather than a silent guess or
/// infinite recursion (COMPILER_ARCHITECTURE.md §4).
#[derive(Debug, Clone)]
pub struct BareDecl {
    pub name: String,
    pub params: Option<Vec<Param>>,
    pub return_ty: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
}

// ── Declarations ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_ty: Option<TypeExpr>,
    pub body: FunctionBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TraitDecl {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub members: Vec<FunctionSig>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImplBlock {
    pub ty: TypeExpr,
    /// `impl Trait for Type` — the trait being implemented.
    pub for_trait: Option<TypeExpr>,
    pub methods: Vec<FunctionDecl>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub value: Expr,
    pub span: Span,
}

// ── Function helpers ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionSig {
    pub name: String,
    pub generic_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum FunctionBody {
    Block(Block),
    /// Single-expression body, e.g. `calculate(x: Int) -> Int: x * 2`
    Expr(Expr),
}

// ── Statements & blocks ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(LetStmt),
    Var(VarStmt),
    /// `state name = expr` — UI framework sugar (UI_SPEC.md).
    State(StateStmt),
    Assign(AssignStmt),
    Expr(Expr),
    Return(ReturnStmt),
    Break(BreakStmt),
    Continue(Span),
    If(IfStmt),
    While(WhileStmt),
    Loop(LoopStmt),
    For(ForStmt),
    Match(MatchStmt),
    /// Local function definition.
    Function(FunctionDecl),
    /// Local struct definition.
    Struct(StructDecl),
    /// A field-declaration shaped line `name: TypeExpr` inside a BareDecl body.
    /// This is what allows the resolver to detect struct-shaped bodies.
    /// Not a valid statement inside an explicit fn body — the resolver will
    /// emit an error if it appears there.
    BareField(FieldDecl),
}

#[derive(Debug, Clone)]
pub struct LetStmt {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct VarStmt {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StateStmt {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AssignStmt {
    pub target: Expr,
    pub op: AssignOp,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignOp { Eq, PlusEq, MinusEq, StarEq, SlashEq, PercentEq }

#[derive(Debug, Clone)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct BreakStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub condition: Expr,
    pub then_block: Block,
    pub else_if_clauses: Vec<(Expr, Block)>,
    pub else_block: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub condition: Expr,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct LoopStmt {
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ForStmt {
    pub binding: String,
    pub iterable: Expr,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchStmt {
    pub scrutinee: Expr,
    pub arms: Vec<MatchArm>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: MatchBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum MatchBody {
    Block(Block),
    Expr(Expr),
}

// ── Patterns ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard(Span),
    Ident(String, Span),
    Literal(Literal, Span),
    Variant(String, Vec<Pattern>, Span),
}

// ── Expressions ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Literal, Span),
    Ident(String, Span),
    Call(CallExpr),
    Member(MemberExpr),
    Index(IndexExpr),
    BinOp(BinOpExpr),
    UnaryOp(UnaryOpExpr),
    Try(TryExpr),
    Range(RangeExpr),
    StringInterp(StringInterpExpr),
}

#[derive(Debug, Clone)]
pub struct CallExpr {
    pub callee: Box<Expr>,
    pub args: Vec<Arg>,
    /// Optional trailing block, e.g. `button("Save"): save()`.
    pub trailing_block: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Arg {
    /// Named argument label, e.g. `title: "…"`.
    pub label: Option<String>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MemberExpr {
    pub object: Box<Expr>,
    pub field: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IndexExpr {
    pub object: Box<Expr>,
    pub index: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct BinOpExpr {
    pub op: BinOp,
    pub left: Box<Expr>,
    pub right: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    Range, RangeInclusive,
    Coalesce,
}

#[derive(Debug, Clone)]
pub struct UnaryOpExpr {
    pub op: UnaryOp,
    pub operand: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnaryOp { Neg, Not }

#[derive(Debug, Clone)]
pub struct TryExpr {
    pub expr: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RangeExpr {
    pub start: Box<Expr>,
    pub end: Box<Expr>,
    pub inclusive: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StringInterpExpr {
    /// Alternating string segments and interpolated expressions.
    pub parts: Vec<InterpPart>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Literal(String),
    Expr(Expr),
}

// ── Literals ─────────────────────────────────────────────────────────────────

/// A literal value as it appears in source (LANGUAGE_SPEC.md §5).
#[derive(Debug, Clone)]
pub enum Literal {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// A type expression as it appears in source (SYNTAX.md §10, `type_expr`).
#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// Named type, e.g. `Int`, `User`, `Option<T>`.
    Named(String, Vec<TypeExpr>, Span),
    /// Tuple type, e.g. `(Int, String)`.
    Tuple(Vec<TypeExpr>, Span),
    /// Collection type `[T]` (TYPE_SYSTEM.md §8).
    Collection(Box<TypeExpr>, Span),
    /// Function type `(A, B) -> C`.
    Function(Vec<TypeExpr>, Box<TypeExpr>, Span),
}

// ── Generics ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: String,
    pub bounds: Vec<String>,
    pub span: Span,
}
