//! Parser unit tests — fixture parsing and BareDecl node emission.

use crate::ast::*;
use crate::diagnostics::DiagnosticSink;
use crate::lexer;
use crate::parser::parse;

// ─── helpers ────────────────────────────────────────────────────────────────

fn parse_clean(src: &str) -> Program {
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(src, &mut sink);
    assert!(!sink.has_errors(), "lex errors: {:?}", sink.diagnostics());
    let prog = parse(&tokens, &mut sink);
    assert!(!sink.has_errors(), "parse errors: {:?}", sink.diagnostics());
    prog
}

fn parse_with_errors(src: &str) -> (Program, DiagnosticSink) {
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(src, &mut sink);
    let prog = parse(&tokens, &mut sink);
    (prog, sink)
}

fn item_count(prog: &Program) -> usize {
    prog.items.len()
}

fn is_bare_decl(item: &Item) -> bool {
    matches!(item, Item::BareDecl(_))
}

fn bare_decl(item: &Item) -> &BareDecl {
    match item {
        Item::BareDecl(b) => b,
        other => panic!("expected BareDecl, got {:?}", other),
    }
}

// ─── 1. Bare declarations (Rule 3 — the core Phase 0/1 feature) ─────────────

#[test]
fn bare_decl_struct_shaped() {
    // A bare colon-block with only field declarations → BareDecl (not classified yet).
    let prog = parse_clean("User:\n    id: Int\n    name: String\n");
    assert_eq!(item_count(&prog), 1);
    assert!(is_bare_decl(&prog.items[0]), "expected BareDecl");
    let bd = bare_decl(&prog.items[0]);
    assert_eq!(bd.name, "User");
    assert!(bd.params.is_none());
    assert!(bd.return_ty.is_none());
    assert_eq!(bd.body.stmts.len(), 2, "expected 2 field stmts");
}

#[test]
fn bare_decl_function_shaped_with_return_type() {
    // Return type present → still BareDecl at parse time (resolver classifies).
    let prog = parse_clean("calculate(x: Int) -> Int:\n    x * 2\n");
    assert_eq!(item_count(&prog), 1);
    assert!(is_bare_decl(&prog.items[0]));
    let bd = bare_decl(&prog.items[0]);
    assert_eq!(bd.name, "calculate");
    assert!(bd.params.is_some());
    assert!(bd.return_ty.is_some());
}

#[test]
fn bare_decl_zero_params_no_return() {
    // No params, no return type — still BareDecl.
    let prog = parse_clean("Splash:\n    loading_screen()\n");
    assert_eq!(item_count(&prog), 1);
    assert!(is_bare_decl(&prog.items[0]));
    let bd = bare_decl(&prog.items[0]);
    assert_eq!(bd.name, "Splash");
    assert!(bd.params.is_none());
}

#[test]
fn bare_decl_empty_body() {
    // Empty body — valid; will classify as struct in resolver.
    let prog = parse_clean("Empty:\n    // nothing\n");
    assert_eq!(item_count(&prog), 1);
    assert!(is_bare_decl(&prog.items[0]));
}

#[test]
fn bare_decl_compact_form() {
    // Compact single-line body: `double(x: Int) -> Int: x * 2`
    let prog = parse_clean("double(x: Int) -> Int: x * 2\n");
    assert_eq!(item_count(&prog), 1);
    assert!(is_bare_decl(&prog.items[0]));
    let bd = bare_decl(&prog.items[0]);
    assert_eq!(bd.name, "double");
}

// ─── 2. Explicit declarations (Rule 2) ──────────────────────────────────────

#[test]
fn explicit_fn_keyword() {
    let prog = parse_clean("fn add(a: Int, b: Int) -> Int:\n    a + b\n");
    assert_eq!(item_count(&prog), 1);
    assert!(matches!(prog.items[0], Item::Function(_)));
    if let Item::Function(f) = &prog.items[0] {
        assert_eq!(f.name, "add");
        assert_eq!(f.params.len(), 2);
    }
}

#[test]
fn explicit_struct_keyword() {
    let prog = parse_clean("struct Point:\n    x: Float\n    y: Float\n");
    assert_eq!(item_count(&prog), 1);
    assert!(matches!(prog.items[0], Item::Struct(_)));
    if let Item::Struct(s) = &prog.items[0] {
        assert_eq!(s.name, "Point");
        assert_eq!(s.fields.len(), 2);
    }
}

#[test]
fn explicit_enum_keyword() {
    let prog = parse_clean("enum Direction:\n    North\n    South\n    East\n    West\n");
    assert_eq!(item_count(&prog), 1);
    assert!(matches!(prog.items[0], Item::Enum(_)));
    if let Item::Enum(e) = &prog.items[0] {
        assert_eq!(e.name, "Direction");
        assert_eq!(e.variants.len(), 4);
    }
}

#[test]
fn enum_with_payload() {
    let prog = parse_clean("enum Shape:\n    Circle(radius: Float)\n    Rectangle(width: Float, height: Float)\n");
    assert!(matches!(prog.items[0], Item::Enum(_)));
    if let Item::Enum(e) = &prog.items[0] {
        assert_eq!(e.variants.len(), 2);
        assert_eq!(e.variants[0].name, "Circle");
        assert_eq!(e.variants[0].fields.len(), 1);
        assert_eq!(e.variants[1].name, "Rectangle");
        assert_eq!(e.variants[1].fields.len(), 2);
    }
}

#[test]
fn const_decl() {
    let prog = parse_clean("const THEME_BG: String = \"#0b0c10\"\n");
    assert_eq!(item_count(&prog), 1);
    assert!(matches!(prog.items[0], Item::Const(_)));
    if let Item::Const(c) = &prog.items[0] {
        assert_eq!(c.name, "THEME_BG");
    }
}

// ─── 3. Import declarations ──────────────────────────────────────────────────

#[test]
fn import_simple() {
    let prog = parse_clean("import http\n");
    assert_eq!(prog.imports.len(), 1);
    assert_eq!(prog.imports[0].path, vec!["http"]);
}

#[test]
fn import_with_alias() {
    let prog = parse_clean("import std::io as io\n");
    assert_eq!(prog.imports[0].path, vec!["std", "io"]);
    assert_eq!(prog.imports[0].alias, Some("io".into()));
}

// ─── 4. Statements inside blocks ────────────────────────────────────────────

#[test]
fn let_stmt_in_function() {
    let prog = parse_clean("fn greet(name: String) -> String:\n    let msg = \"Hello!\"\n    msg\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            assert!(matches!(&b.stmts[0], Stmt::Let(_)));
        }
    }
}

#[test]
fn var_stmt() {
    let prog = parse_clean("fn count():\n    var n = 0\n    n\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            assert!(matches!(&b.stmts[0], Stmt::Var(_)));
        }
    }
}

#[test]
fn return_stmt() {
    let prog = parse_clean("fn zero() -> Int:\n    return 0\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            assert!(matches!(&b.stmts[0], Stmt::Return(_)));
        }
    }
}

#[test]
fn if_else_stmt() {
    let prog = parse_clean(
        "fn abs(x: Int) -> Int:\n    if x < 0:\n        return 0 - x\n    else:\n        return x\n",
    );
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            assert!(matches!(&b.stmts[0], Stmt::If(_)));
        }
    }
}

#[test]
fn for_stmt() {
    let prog = parse_clean("fn sum(items: [Int]):\n    for item in items:\n        item\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            assert!(matches!(&b.stmts[0], Stmt::For(_)));
        }
    }
}

#[test]
fn match_stmt() {
    let prog = parse_clean(
        "fn f(x: Int) -> Int:\n    match x:\n        0: 1\n        _: x\n",
    );
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            assert!(matches!(&b.stmts[0], Stmt::Match(_)));
            if let Stmt::Match(m) = &b.stmts[0] {
                assert_eq!(m.arms.len(), 2);
            }
        }
    }
}

// ─── 5. Expressions ─────────────────────────────────────────────────────────

#[test]
fn binary_arithmetic() {
    let prog = parse_clean("fn f() -> Int: 1 + 2 * 3\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Expr(e) = &f.body {
            assert!(matches!(e, Expr::BinOp(_)));
        }
    }
}

#[test]
fn try_operator() {
    let prog = parse_clean("fn f() -> Result<Int, String>:\n    let x = parse(s)?\n    x\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Block(b) = &f.body {
            if let Stmt::Let(l) = &b.stmts[0] {
                assert!(matches!(l.value, Expr::Try(_)));
            }
        }
    }
}

#[test]
fn member_access() {
    let prog = parse_clean("fn f() -> String: user.name\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Expr(e) = &f.body {
            assert!(matches!(e, Expr::Member(_)));
        }
    }
}

#[test]
fn call_with_named_arg() {
    let prog = parse_clean("fn f(): greet(name: \"Alice\")\n");
    if let Item::Function(f) = &prog.items[0] {
        if let FunctionBody::Expr(e) = &f.body {
            if let Expr::Call(c) = e {
                assert_eq!(c.args[0].label, Some("name".into()));
            }
        }
    }
}

// ─── 6. Parsing fixture files ────────────────────────────────────────────────

#[test]
fn parse_structs_fixture() {
    let src = include_str!("../../../tests/fixtures/structs/basic.nv");
    let prog = parse_clean(src);
    // All top-level items in this file are bare-decl structs or explicit structs.
    assert!(!prog.items.is_empty(), "expected items");
}

#[test]
fn parse_functions_fixture() {
    let src = include_str!("../../../tests/fixtures/functions/basic.nv");
    let prog = parse_clean(src);
    assert!(!prog.items.is_empty(), "expected items");
}

#[test]
fn parse_enums_fixture() {
    let src = include_str!("../../../tests/fixtures/enums_match/basic.nv");
    let prog = parse_clean(src);
    assert!(!prog.items.is_empty(), "expected items");
}

#[test]
fn parse_result_option_fixture() {
    let src = include_str!("../../../tests/fixtures/result_option/basic.nv");
    let prog = parse_clean(src);
    assert!(!prog.items.is_empty(), "expected items");
}

// ─── 7. Ambiguous_decls fixtures — BareDecl emitted correctly ───────────────

#[test]
fn ambiguous_struct_minimal_is_bare_decl() {
    let src = include_str!("../../../tests/fixtures/ambiguous_decls/struct_minimal.nv");
    let prog = parse_clean(src);
    let bare: Vec<_> = prog.items.iter().filter(|i| is_bare_decl(i)).collect();
    assert!(!bare.is_empty(), "expected at least one BareDecl");
    // The BareDecl for Item should have no params and no return type.
    let bd = bare_decl(bare[0]);
    assert_eq!(bd.name, "Item");
    assert!(bd.params.is_none());
    assert!(bd.return_ty.is_none());
}

#[test]
fn ambiguous_function_minimal_is_bare_decl() {
    let src = include_str!("../../../tests/fixtures/ambiguous_decls/function_minimal.nv");
    let prog = parse_clean(src);
    let bare: Vec<_> = prog.items.iter().filter(|i| is_bare_decl(i)).collect();
    assert!(!bare.is_empty(), "expected BareDecl");
    // `identity` has a return type — still BareDecl at parse time.
    let bd = bare_decl(bare[0]);
    assert_eq!(bd.name, "identity");
    assert!(bd.return_ty.is_some(), "expected return type in BareDecl");
}

#[test]
fn ambiguous_cycle_fixtures_parse_without_errors() {
    // The classification cycle is a resolver-level error, not a parse-level error.
    // All three cycle fixtures must parse cleanly (producing BareDecls).
    for (name, src) in [
        ("cycle_self", include_str!("../../../tests/fixtures/ambiguous_decls/cycle_self.nv")),
        ("cycle_direct", include_str!("../../../tests/fixtures/ambiguous_decls/cycle_direct.nv")),
        ("cycle_indirect", include_str!("../../../tests/fixtures/ambiguous_decls/cycle_indirect.nv")),
    ] {
        let (prog, sink) = parse_with_errors(src);
        assert!(
            !sink.has_errors(),
            "fixture `{name}` should parse without errors (cycles are resolver errors): {:?}",
            sink.diagnostics()
        );
        let bare: Vec<_> = prog.items.iter().filter(|i| is_bare_decl(i)).collect();
        assert!(!bare.is_empty(), "fixture `{name}` expected BareDecl items");
    }
}


// ─── 8. Targeted: inline if-else ────────────────────────────────────────────

#[test]
fn inline_if_else_single_line() {
    let src = "fn f(x: String) -> String:\n    if x.starts_with(\"+\"): \"up\" else: \"down\"\n";
    let prog = parse_clean(src);
    assert_eq!(item_count(&prog), 1);
}


#[test]
fn minimal_if_else() {
    // Bare if-else in a function body, indented
    let src = "fn f() -> Int:\n    if true: 1 else: 2\n";
    let (prog, sink) = parse_with_errors(src);
    assert!(!sink.has_errors(), "errors: {:?}", sink.diagnostics());
    let _ = prog;
}


#[test]
fn if_else_call_condition() {
    let src = "fn f(s: String) -> String:\n    if s.starts_with(\"+\"): \"up\" else: \"down\"\n";
    let (prog, sink) = parse_with_errors(src);
    assert!(!sink.has_errors(), "errors: {:?}", sink.diagnostics());
    let _ = prog;
}


#[test]
fn parse_dashboard_nonui_fixture() {
    let src = include_str!("../../../tests/fixtures/dashboard_nonui.nv");
    let prog = parse_clean(src);
    assert!(!prog.items.is_empty(), "expected items");
}


