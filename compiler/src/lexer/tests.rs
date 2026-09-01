//! Lexer unit tests — fixture tokenisation and INDENT/DEDENT synthesis.

use super::{lex, Token};
use crate::diagnostics::DiagnosticSink;

// ─── helpers ────────────────────────────────────────────────────────────────

fn tokens(src: &str) -> (Vec<Token>, DiagnosticSink) {
    let mut sink = DiagnosticSink::new();
    let spanned = lex(src, &mut sink);
    let toks = spanned.into_iter().map(|s| s.node).collect();
    (toks, sink)
}

fn clean(src: &str) -> Vec<Token> {
    let (toks, sink) = tokens(src);
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());
    toks
}

fn has_error(src: &str, code: &str) -> bool {
    let (_, sink) = tokens(src);
    sink.diagnostics()
        .iter()
        .any(|d| d.code.as_deref() == Some(code))
}

// Strip Newline/Indent/Dedent/Eof noise for focused checks.
fn meaningful(src: &str) -> Vec<Token> {
    clean(src)
        .into_iter()
        .filter(|t| !matches!(t, Token::Newline | Token::Indent | Token::Dedent | Token::Eof))
        .collect()
}

// ─── 1. Identifiers & keywords ──────────────────────────────────────────────

#[test]
fn bare_identifier() {
    assert_eq!(meaningful("hello"), vec![Token::Ident("hello".into())]);
}

#[test]
fn underscore_ident() {
    assert_eq!(meaningful("_x"), vec![Token::Ident("_x".into())]);
}

#[test]
fn non_latin_ident() {
    // LANGUAGE_SPEC.md §1 — XID_Start/XID_Continue identifiers
    assert_eq!(meaningful("café"), vec![Token::Ident("café".into())]);
    assert_eq!(meaningful("привет"), vec![Token::Ident("привет".into())]);
}

#[test]
fn all_core_keywords() {
    use Token::*;
    let pairs: &[(&str, Token)] = &[
        ("as", As), ("async", Async), ("await", Await), ("break", Break),
        ("const", Const), ("continue", Continue), ("else", Else),
        ("export", Export), ("for", For), ("if", If), ("import", Import),
        ("in", In), ("let", Let), ("loop", Loop), ("match", Match),
        ("return", Return), ("var", Var), ("while", While),
    ];
    for (src, expected) in pairs {
        assert_eq!(meaningful(src), vec![expected.clone()], "keyword `{src}`");
    }
}

#[test]
fn bool_literals_from_keywords() {
    assert_eq!(meaningful("true"), vec![Token::BoolLit(true)]);
    assert_eq!(meaningful("false"), vec![Token::BoolLit(false)]);
}

#[test]
fn type_system_keywords() {
    use Token::*;
    let pairs: &[(&str, Token)] = &[
        ("enum", Enum), ("fn", Fn), ("impl", Impl), ("struct", Struct),
        ("trait", Trait), ("type", Type), ("unsafe", Unsafe),
    ];
    for (src, expected) in pairs {
        assert_eq!(meaningful(src), vec![expected.clone()], "keyword `{src}`");
    }
}

#[test]
fn memory_keywords() {
    use Token::*;
    let pairs: &[(&str, Token)] = &[
        ("owned", Owned), ("borrow", Borrow), ("managed", Managed),
        ("weak", Weak), ("unowned", Unowned), ("task", Task),
    ];
    for (src, expected) in pairs {
        assert_eq!(meaningful(src), vec![expected.clone()], "keyword `{src}`");
    }
}

// ─── 2. Integer literals ────────────────────────────────────────────────────

#[test]
fn integer_decimal() {
    assert_eq!(meaningful("42"), vec![Token::IntLit(42)]);
    assert_eq!(meaningful("1_000_000"), vec![Token::IntLit(1_000_000)]);
}

#[test]
fn integer_hex() {
    assert_eq!(meaningful("0xFF"), vec![Token::IntLit(255)]);
    assert_eq!(meaningful("0x1A"), vec![Token::IntLit(26)]);
}

#[test]
fn integer_octal() {
    assert_eq!(meaningful("0o17"), vec![Token::IntLit(0o17)]);
}

#[test]
fn integer_binary() {
    assert_eq!(meaningful("0b1010"), vec![Token::IntLit(0b1010)]);
}

// ─── 3. Float literals ──────────────────────────────────────────────────────

#[test]
fn float_basic() {
    assert_eq!(meaningful("3.14"), vec![Token::FloatLit(3.14)]);
}

#[test]
fn float_exponent() {
    assert_eq!(meaningful("2.0e10"), vec![Token::FloatLit(2.0e10)]);
}

#[test]
fn float_underscore() {
    assert_eq!(meaningful("1_000.5"), vec![Token::FloatLit(1000.5)]);
}

// ─── 4. Character literals ──────────────────────────────────────────────────

#[test]
fn char_simple() {
    assert_eq!(meaningful("'a'"), vec![Token::CharLit('a')]);
}

#[test]
fn char_newline_escape() {
    assert_eq!(meaningful("'\\n'"), vec![Token::CharLit('\n')]);
}

#[test]
fn char_unicode_escape() {
    assert_eq!(meaningful("'\\u{1F600}'"), vec![Token::CharLit('\u{1F600}')]);
}

// ─── 5. String literals ─────────────────────────────────────────────────────

#[test]
fn plain_string() {
    assert_eq!(meaningful("\"hello\""), vec![Token::StringLit("hello".into())]);
}

#[test]
fn string_escape() {
    assert_eq!(meaningful("\"a\\nb\""), vec![Token::StringLit("a\nb".into())]);
}

#[test]
fn string_escaped_brace() {
    // \{ and \} produce literal braces, not interpolation
    assert_eq!(meaningful("\"a\\{b\\}\""), vec![Token::StringLit("a{b}".into())]);
}

#[test]
fn multiline_string() {
    let src = "\"\"\"hello\nworld\"\"\"";
    assert_eq!(meaningful(src), vec![Token::StringLit("hello\nworld".into())]);
}

// ─── 6. String interpolation ────────────────────────────────────────────────

#[test]
fn simple_interpolation() {
    // "Hello, {name}!" → InterpStart("Hello, "), Ident("name"), InterpEnd("!")
    let toks = meaningful("\"Hello, {name}!\"");
    assert_eq!(
        toks,
        vec![
            Token::InterpStart("Hello, ".into()),
            Token::Ident("name".into()),
            Token::InterpEnd("!".into()),
        ]
    );
}

#[test]
fn interpolation_no_suffix() {
    // "{x}" → InterpStart(""), Ident("x"), InterpEnd("")
    let toks = meaningful("\"{x}\"");
    assert_eq!(
        toks,
        vec![
            Token::InterpStart("".into()),
            Token::Ident("x".into()),
            Token::InterpEnd("".into()),
        ]
    );
}

#[test]
fn multiple_interpolations() {
    // "{a} and {b}" → InterpStart(""), a, InterpMiddle(" and "), b, InterpEnd("")
    let toks = meaningful("\"{a} and {b}\"");
    assert_eq!(
        toks,
        vec![
            Token::InterpStart("".into()),
            Token::Ident("a".into()),
            Token::InterpMiddle(" and ".into()),
            Token::Ident("b".into()),
            Token::InterpEnd("".into()),
        ]
    );
}

#[test]
fn nested_interpolation_with_string() {
    // "outer {f("inner")} done"
    let src = r#""outer {f("inner")} done""#;
    let toks = meaningful(src);
    // InterpStart("outer "), Ident("f"), LParen, StringLit("inner"), RParen, InterpEnd(" done")
    assert!(matches!(toks[0], Token::InterpStart(ref s) if s == "outer "));
    assert!(matches!(toks[1], Token::Ident(ref s) if s == "f"));
    assert_eq!(toks[2], Token::LParen);
    assert_eq!(toks[3], Token::StringLit("inner".into()));
    assert_eq!(toks[4], Token::RParen);
    assert!(matches!(toks[5], Token::InterpEnd(ref s) if s == " done"));
}

// ─── 7. Comments ────────────────────────────────────────────────────────────

#[test]
fn line_comment() {
    let toks = clean("// hello");
    assert!(toks.iter().any(|t| matches!(t, Token::LineComment(s) if s.contains("hello"))));
}

#[test]
fn doc_comment() {
    let toks = clean("/// docs");
    assert!(toks.iter().any(|t| matches!(t, Token::DocComment(s) if s.contains("docs"))));
}

#[test]
fn mod_doc_comment() {
    let toks = clean("//! module docs");
    assert!(toks.iter().any(|t| matches!(t, Token::ModDocComment(s) if s.contains("module docs"))));
}

#[test]
fn block_comment() {
    let toks = clean("/* block */");
    assert!(toks.iter().any(|t| matches!(t, Token::BlockComment(s) if s.contains("block"))));
}

// ─── 8. Operators & punctuation ─────────────────────────────────────────────

#[test]
fn arrow_token() {
    assert_eq!(meaningful("->"), vec![Token::Arrow]);
}

#[test]
fn double_colon() {
    assert_eq!(meaningful("::"), vec![Token::ColonColon]);
}

#[test]
fn range_tokens() {
    assert_eq!(meaningful(".."), vec![Token::DotDot]);
    assert_eq!(meaningful("..="), vec![Token::DotDotEq]);
}

#[test]
fn question_tokens() {
    assert_eq!(meaningful("?"), vec![Token::Question]);
    assert_eq!(meaningful("??"), vec![Token::QuestionQuestion]);
}

#[test]
fn compound_assignment() {
    use Token::*;
    assert_eq!(meaningful("+="), vec![PlusEq]);
    assert_eq!(meaningful("-="), vec![MinusEq]);
    assert_eq!(meaningful("*="), vec![StarEq]);
    assert_eq!(meaningful("/="), vec![SlashEq]);
    assert_eq!(meaningful("%="), vec![PercentEq]);
}

// ─── 9. INDENT/DEDENT synthesis ─────────────────────────────────────────────

#[test]
fn indent_dedent_basic() {
    let src = "if x:\n    y\nz";
    let (toks, sink) = tokens(src);
    assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    // Expected sequence (structural only):
    // If, Ident(x), Colon, Newline, Indent, Ident(y), Newline, Dedent, Ident(z), Newline, Eof
    let structural: Vec<_> = toks
        .iter()
        .filter(|t| {
            matches!(
                t,
                Token::Indent | Token::Dedent | Token::Newline | Token::Eof
                    | Token::If | Token::Ident(_) | Token::Colon
            )
        })
        .collect();
    assert!(
        structural.contains(&&Token::Indent),
        "expected Indent in {structural:?}"
    );
    assert!(
        structural.contains(&&Token::Dedent),
        "expected Dedent in {structural:?}"
    );
}

#[test]
fn blank_lines_dont_affect_indent() {
    let src = "if x:\n    y\n\n    z\nw";
    let (toks, sink) = tokens(src);
    assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    // Only one Indent and one Dedent expected — blank line between y and z
    // must not cause a spurious Dedent/Indent pair.
    let indents = toks.iter().filter(|t| **t == Token::Indent).count();
    let dedents = toks.iter().filter(|t| **t == Token::Dedent).count();
    assert_eq!(indents, 1, "expected 1 Indent, got {indents}");
    assert_eq!(dedents, 1, "expected 1 Dedent, got {dedents}");
}

#[test]
fn comment_lines_dont_affect_indent() {
    let src = "if x:\n    y\n    // comment\n    z\nw";
    let (toks, sink) = tokens(src);
    assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    let indents = toks.iter().filter(|t| **t == Token::Indent).count();
    let dedents = toks.iter().filter(|t| **t == Token::Dedent).count();
    assert_eq!(indents, 1, "1 Indent; got {indents}");
    assert_eq!(dedents, 1, "1 Dedent; got {dedents}");
}

#[test]
fn brace_suspends_indent_tracking() {
    // Inside { } no INDENT/DEDENT should be emitted regardless of whitespace.
    let src = "x: {\n    y\n    z\n}";
    let (toks, sink) = tokens(src);
    assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    let indents = toks.iter().filter(|t| **t == Token::Indent).count();
    let dedents = toks.iter().filter(|t| **t == Token::Dedent).count();
    assert_eq!(indents, 0, "no Indent inside braces; got {indents}");
    assert_eq!(dedents, 0, "no Dedent inside braces; got {dedents}");
}

// ─── 10. Error cases ────────────────────────────────────────────────────────

#[test]
fn tab_mixing_is_error() {
    // Tab mixed with spaces in indentation must produce E0002.
    let src = "if x:\n\t    y"; // tab then spaces
    assert!(has_error(src, "E0002"), "expected E0002 for tab mixing");
}

#[test]
fn mismatched_dedent_is_error() {
    // Dedent to a level that doesn't match any enclosing level → E0003.
    let src = "if x:\n    if y:\n        z\n  w"; // 2-space dedent, not 0 or 4
    assert!(has_error(src, "E0003"), "expected E0003 for mismatched dedent");
}

#[test]
fn unrecognised_character_is_error() {
    // `@` is not a valid Noctivue character.
    assert!(has_error("@", "E0001"), "expected E0001 for `@`");
}
