//! Type-checker unit tests.
//!
//! Tests cover all fixture files referenced in the Phase 1 spec:
//! - `structs/basic.nv`          — struct lowering, field types
//! - `functions/basic.nv`        — function checking, return types, bindings
//! - `enums_match/basic.nv`      — enum lowering, match statements
//! - `result_option/basic.nv`    — Option/Result, `?`, coalescing

use std::path::Path;

use crate::diagnostics::DiagnosticSink;
use crate::hir::{self, Ty};
use crate::lexer;
use crate::parser::parse;
use crate::resolver::resolve;
use crate::typeck::typecheck;

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Lex → parse → resolve → typecheck a fixture file.
/// Returns `(hir::Module, DiagnosticSink)`.
fn typecheck_fixture(relative_path: &str) -> (hir::Module, DiagnosticSink) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().expect("workspace root");
    let full_path = workspace_root.join(relative_path);

    let source = std::fs::read_to_string(&full_path)
        .unwrap_or_else(|e| panic!("could not read fixture {relative_path}: {e}"));

    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(&source, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    let module = typecheck(program, &mut sink);
    (module, sink)
}

fn has_errors(sink: &DiagnosticSink) -> bool {
    sink.has_errors()
}

fn error_codes(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics()
        .iter()
        .filter_map(|d| d.code.clone())
        .collect()
}

// ─── structs/basic.nv ────────────────────────────────────────────────────────

#[test]
fn typeck_structs_basic_no_errors() {
    let (module, sink) = typecheck_fixture("tests/fixtures/structs/basic.nv");
    assert!(
        !has_errors(&sink),
        "unexpected errors: {:?}",
        sink.diagnostics()
    );
    // Should have at least the structs we declared.
    assert!(!module.structs.is_empty(), "expected structs in HIR module");
}

#[test]
fn typeck_structs_basic_user_fields() {
    let (module, sink) = typecheck_fixture("tests/fixtures/structs/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let user = module.structs.iter().find(|s| s.name == "User");
    assert!(user.is_some(), "expected struct `User` in module");
    let user = user.unwrap();

    let id_field = user.fields.iter().find(|(n, _)| n == "id");
    assert!(id_field.is_some(), "expected field `id`");
    assert_eq!(id_field.unwrap().1, Ty::Int);

    let name_field = user.fields.iter().find(|(n, _)| n == "name");
    assert!(name_field.is_some(), "expected field `name`");
    assert_eq!(name_field.unwrap().1, Ty::String);
}

#[test]
fn typeck_structs_basic_point_fields() {
    let (module, sink) = typecheck_fixture("tests/fixtures/structs/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let point = module.structs.iter().find(|s| s.name == "Point").expect("Point struct");
    assert_eq!(point.fields.iter().find(|(n, _)| n == "x").unwrap().1, Ty::Float);
    assert_eq!(point.fields.iter().find(|(n, _)| n == "y").unwrap().1, Ty::Float);
}

// ─── functions/basic.nv ──────────────────────────────────────────────────────

#[test]
fn typeck_functions_basic_no_errors() {
    let (module, sink) = typecheck_fixture("tests/fixtures/functions/basic.nv");
    assert!(
        !has_errors(&sink),
        "unexpected errors: {:?}",
        sink.diagnostics()
    );
    assert!(!module.functions.is_empty(), "expected functions in HIR module");
}

#[test]
fn typeck_functions_calculate_return_type() {
    let (module, sink) = typecheck_fixture("tests/fixtures/functions/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let calc = module.functions.iter().find(|f| f.name == "calculate").expect("calculate fn");
    assert_eq!(calc.return_ty, Ty::Int);
    assert_eq!(calc.params, vec![("x".to_string(), Ty::Int)]);
}

#[test]
fn typeck_functions_add_return_type() {
    let (module, sink) = typecheck_fixture("tests/fixtures/functions/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let add = module.functions.iter().find(|f| f.name == "add").expect("add fn");
    assert_eq!(add.return_ty, Ty::Int);
    assert_eq!(add.params.len(), 2);
}

#[test]
fn typeck_functions_greet_string_return() {
    let (module, sink) = typecheck_fixture("tests/fixtures/functions/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let greet = module.functions.iter().find(|f| f.name == "greet").expect("greet fn");
    assert_eq!(greet.return_ty, Ty::String);
}

// ─── enums_match/basic.nv ────────────────────────────────────────────────────

#[test]
fn typeck_enums_match_basic_no_errors() {
    let (module, sink) = typecheck_fixture("tests/fixtures/enums_match/basic.nv");
    assert!(
        !has_errors(&sink),
        "unexpected errors: {:?}",
        sink.diagnostics()
    );
    assert!(!module.enums.is_empty(), "expected enums in HIR module");
}

#[test]
fn typeck_enums_match_direction_variants() {
    let (module, sink) = typecheck_fixture("tests/fixtures/enums_match/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let dir = module.enums.iter().find(|e| e.name == "Direction").expect("Direction enum");
    let variant_names: Vec<&str> = dir.variants.iter().map(|(n, _)| n.as_str()).collect();
    assert!(variant_names.contains(&"North"));
    assert!(variant_names.contains(&"South"));
    assert!(variant_names.contains(&"East"));
    assert!(variant_names.contains(&"West"));
}

#[test]
fn typeck_enums_match_shape_variants_have_fields() {
    let (module, sink) = typecheck_fixture("tests/fixtures/enums_match/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let shape = module.enums.iter().find(|e| e.name == "Shape").expect("Shape enum");
    let circle = shape.variants.iter().find(|(n, _)| n == "Circle").expect("Circle variant");
    assert_eq!(circle.1, vec![Ty::Float]);
}

#[test]
fn typeck_enums_area_function_return_type() {
    let (module, sink) = typecheck_fixture("tests/fixtures/enums_match/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let area = module.functions.iter().find(|f| f.name == "area").expect("area fn");
    assert_eq!(area.return_ty, Ty::Float);
}

// ─── result_option/basic.nv ──────────────────────────────────────────────────

#[test]
fn typeck_result_option_basic_no_errors() {
    let (module, sink) = typecheck_fixture("tests/fixtures/result_option/basic.nv");
    assert!(
        !has_errors(&sink),
        "unexpected errors: {:?}",
        sink.diagnostics()
    );
    assert!(!module.functions.is_empty(), "expected functions");
}

#[test]
fn typeck_find_user_returns_option_string() {
    let (module, sink) = typecheck_fixture("tests/fixtures/result_option/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let f = module.functions.iter().find(|f| f.name == "find_user").expect("find_user");
    assert_eq!(f.return_ty, Ty::Option(Box::new(Ty::String)));
}

#[test]
fn typeck_parse_int_returns_result() {
    let (module, sink) = typecheck_fixture("tests/fixtures/result_option/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let f = module.functions.iter().find(|f| f.name == "parse_int").expect("parse_int");
    assert_eq!(f.return_ty, Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)));
}

#[test]
fn typeck_double_parsed_uses_try() {
    let (module, sink) = typecheck_fixture("tests/fixtures/result_option/basic.nv");
    assert!(!has_errors(&sink), "unexpected errors: {:?}", sink.diagnostics());

    let f =
        module.functions.iter().find(|f| f.name == "double_parsed").expect("double_parsed");
    assert_eq!(f.return_ty, Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)));
}

// ─── E0200 — type mismatch ────────────────────────────────────────────────────

#[test]
fn typeck_e0200_emitted_on_mismatch() {
    let src = r#"
fn wrong(x: Int) -> String:
    x
"#;
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(src, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    // The single-expression body is `x` (Int) but return type is String.
    // For single-expression bodies we emit E0200.
    typecheck(program, &mut sink);
    // Sink should now contain an E0200.
    let codes = error_codes(&sink);
    assert!(
        codes.contains(&"E0200".to_string()),
        "expected E0200, got: {:?}",
        codes
    );
}

// ─── E0201 — unknown identifier ───────────────────────────────────────────────

#[test]
fn typeck_e0201_emitted_on_unknown_ident() {
    let src = r#"
fn use_unknown() -> Int:
    let x = does_not_exist
    x
"#;
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(src, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    typecheck(program, &mut sink);
    let codes = error_codes(&sink);
    assert!(
        codes.contains(&"E0201".to_string()),
        "expected E0201, got: {:?}",
        codes
    );
}

// ─── E0203 — ? on non-Result/Option ──────────────────────────────────────────

#[test]
fn typeck_e0203_try_on_int() {
    let src = r#"
fn bad_try(x: Int) -> Int:
    x?
"#;
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(src, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    typecheck(program, &mut sink);
    let codes = error_codes(&sink);
    assert!(
        codes.contains(&"E0203".to_string()),
        "expected E0203, got: {:?}",
        codes
    );
}
