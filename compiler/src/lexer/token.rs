//! Token definitions for the Noctivue lexer.
//!
//! This module lists every terminal the lexer can produce, including the
//! synthesised INDENT/DEDENT tokens from the offside-rule pass.
//! See LANGUAGE_SPEC.md §6–7 and SYNTAX.md §10 for the normative terminal set.

/// Every terminal the Noctivue lexer can produce.
///
/// Keyword variants are listed explicitly (not as `Ident("if")`) so that the
/// parser's keyword table is a pattern match rather than a string comparison,
/// and so that reserved-but-unassigned words (LANGUAGE_SPEC.md §7.4) are
/// unambiguously tokenised.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // ── Synthesised structural tokens ───────────────────────────────────────
    /// Synthesised when indentation increases (offside rule).
    Indent,
    /// Synthesised when indentation returns to a previous level.
    Dedent,
    /// Physical or logical newline that terminates a statement.
    Newline,

    // ── Literals ────────────────────────────────────────────────────────────
    /// Integer literal, e.g. `42`, `0xFF`, `1_000`.
    IntLit(i128),
    /// Float literal, e.g. `3.14`, `2.0e10`.
    FloatLit(f64),
    /// Boolean literal.
    BoolLit(bool),
    /// Character literal, e.g. `'a'`.
    CharLit(char),
    /// Plain string literal (no interpolation), e.g. `"hello"`.
    StringLit(String),
    /// Start of an interpolated string segment `"…{`.
    InterpStart(String),
    /// Middle segment of an interpolated string `}…{`.
    InterpMiddle(String),
    /// End segment of an interpolated string `}…"`.
    InterpEnd(String),

    // ── Identifiers ─────────────────────────────────────────────────────────
    Ident(String),

    // ── Core keywords (LANGUAGE_SPEC.md §7.1) ───────────────────────────────
    As, Async, Await, Break, Const, Continue,
    Else, Export, False, For, If, Import,
    In, Let, Loop, Match, Mod, Return, True,
    Use, Var, While,

    // ── Type/system keywords (§7.2) ─────────────────────────────────────────
    Enum, Fn, Impl, Struct, Trait, Type, Unsafe,

    // ── Memory/concurrency keywords (§7.3) ──────────────────────────────────
    Owned, Borrow, Managed, Weak, Unowned, Task,

    // ── Reserved-for-future-use keywords (§7.4) ─────────────────────────────
    Actor, Defer, Extern, Macro, Native, Operator,
    Protocol, Reflect, Spawn, Static, Where, Yield,

    // ── Operators & punctuation (LANGUAGE_SPEC.md §6) ───────────────────────
    Plus, Minus, Star, Slash, Percent,
    EqEq, BangEq, Lt, LtEq, Gt, GtEq,
    AmpAmp, PipePipe, Bang,
    Eq, PlusEq, MinusEq, StarEq, SlashEq, PercentEq,
    DotDot, DotDotEq, DotDotDot,
    Question, QuestionQuestion,
    Arrow,                    // `->`
    Dot, ColonColon,          // `.`  `::`
    Colon, Comma, Semi,
    Pipe,                     // `|` for closures
    LParen, RParen,
    LBracket, RBracket,
    LBrace, RBrace,

    // ── Comments ────────────────────────────────────────────────────────────
    /// `// …` line comment (usually skipped, kept for doc-comment extraction).
    LineComment(String),
    /// `/* … */` block comment.
    BlockComment(String),
    /// `/// …` doc comment (attached to the following declaration).
    DocComment(String),
    /// `//! …` module-level doc comment.
    ModDocComment(String),

    // ── End of file ─────────────────────────────────────────────────────────
    Eof,
}
