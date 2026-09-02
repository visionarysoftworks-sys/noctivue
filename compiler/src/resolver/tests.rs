//! Resolver unit tests — BareDecl classification + cycle detection.
//!
//! Tests cover all fixture files referenced in the Phase 0/1 spec:
//! - Struct classification (body = exclusively field decls)
//! - Function classification (return type present, or body has non-field stmts)
//! - Empty body → struct (vacuous rule)
//! - Cycle detection: self (A→A), direct (A→B→A), indirect (A→B→C→A)
//! - Component vs function transitive resolution
//! - structs/basic.nv, functions/basic.nv, enums_match/basic.nv,
//!   result_option/basic.nv

use std::path::Path;

use crate::ast::{Item, Program};
use crate::diagnostics::DiagnosticSink;
use crate::lexer;
use crate::parser::parse;
use crate::resolver::resolve;

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Lex, parse, and resolve the fixture at `relative_path` (relative to the
/// workspace root, i.e. `E:\Projects\Noctivue\0.0.1`).
fn resolve_fixture(relative_path: &str) -> (Program, DiagnosticSink) {
    // Determine absolute path: the workspace root is two levels above
    // `compiler/` (i.e. `compiler/src/resolver/tests.rs` → `../../../..`).
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR")); // …/compiler
    let workspace_root = manifest_dir.parent().expect("workspace root");
    let full_path = workspace_root.join(relative_path);

    let source = std::fs::read_to_string(&full_path)
        .unwrap_or_else(|e| panic!("could not read fixture {relative_path}: {e}"));

    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(&source, &mut sink);
    let program = parse(&tokens, &mut sink);
    let program = resolve(program, &mut sink);
    (program, sink)
}

// ─── helpers for item inspection ─────────────────────────────────────────────

fn is_struct(item: &Item) -> bool {
    matches!(item, Item::Struct(_))
}

fn is_function(item: &Item) -> bool {
    matches!(item, Item::Function(_))
}

fn is_bare_decl(item: &Item) -> bool {
    matches!(item, Item::BareDecl(_))
}

fn is_enum(item: &Item) -> bool {
    matches!(item, Item::Enum(_))
}

fn item_name(item: &Item) -> &str {
    match item {
        Item::Struct(s) => &s.name,
        Item::Function(f) => &f.name,
        Item::BareDecl(b) => &b.name,
        Item::Enum(e) => &e.name,
        Item::Trait(t) => &t.name,
        Item::Impl(_) => "<impl>",
        Item::Const(c) => &c.name,
        Item::Export(_) => "<export>",
        Item::Mod(m) => &m.name,
    }
}

fn find_item<'a>(items: &'a [Item], name: &str) -> &'a Item {
    items
        .iter()
        .find(|i| item_name(i) == name)
        .unwrap_or_else(|| panic!("no item named `{name}` in program"))
}

fn has_e0010(sink: &DiagnosticSink) -> bool {
    sink.diagnostics()
        .iter()
        .any(|d| d.code.as_deref() == Some("E0010"))
}

// ─── 1. struct_minimal.nv → Item::Struct ─────────────────────────────────────

#[test]
fn resolver_struct_minimal_classified_as_struct() {
    let (prog, sink) = resolve_fixture("tests/fixtures/ambiguous_decls/struct_minimal.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());
    assert_eq!(prog.items.len(), 1);
    assert!(is_struct(&prog.items[0]), "expected Item::Struct, got {:?}", prog.items[0]);
    assert_eq!(item_name(&prog.items[0]), "Item");
}

// ─── 2. function_minimal.nv → Item::Function ─────────────────────────────────

#[test]
fn resolver_function_minimal_classified_as_function() {
    let (prog, sink) = resolve_fixture("tests/fixtures/ambiguous_decls/function_minimal.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());
    assert_eq!(prog.items.len(), 1);
    assert!(is_function(&prog.items[0]), "expected Item::Function, got {:?}", prog.items[0]);
    assert_eq!(item_name(&prog.items[0]), "identity");
}

// ─── 3. function_no_return_type.nv → all four classified as Item::Function ───

#[test]
fn resolver_function_no_return_type_all_classified_as_function() {
    let (prog, sink) =
        resolve_fixture("tests/fixtures/ambiguous_decls/function_no_return_type.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());

    let expected = ["make_greeting", "early_return", "accumulate", "double"];
    assert_eq!(
        prog.items.len(),
        expected.len(),
        "expected {} items, got {}",
        expected.len(),
        prog.items.len()
    );

    for name in &expected {
        let item = find_item(&prog.items, name);
        assert!(
            is_function(item),
            "`{name}` should be classified as Function, got {item:?}"
        );
    }
}

// ─── 4. empty_body.nv → Item::Struct (vacuous rule) ──────────────────────────

#[test]
fn resolver_empty_body_classified_as_struct() {
    let (prog, sink) = resolve_fixture("tests/fixtures/ambiguous_decls/empty_body.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());
    assert_eq!(prog.items.len(), 1);
    assert!(is_struct(&prog.items[0]), "expected Item::Struct, got {:?}", prog.items[0]);
    assert_eq!(item_name(&prog.items[0]), "Empty");
}

// ─── 5. cycle_self.nv → E0010 emitted ────────────────────────────────────────

#[test]
fn resolver_cycle_self_emits_e0010() {
    let (_, sink) = resolve_fixture("tests/fixtures/ambiguous_decls/cycle_self.nv");
    assert!(
        sink.has_errors(),
        "expected diagnostic errors for self-cycle"
    );
    assert!(
        has_e0010(&sink),
        "expected E0010 for self-cycle, diagnostics: {:?}",
        sink.diagnostics()
    );
}

// ─── 6. cycle_direct.nv → E0010 emitted ──────────────────────────────────────

#[test]
fn resolver_cycle_direct_emits_e0010() {
    let (_, sink) = resolve_fixture("tests/fixtures/ambiguous_decls/cycle_direct.nv");
    assert!(
        sink.has_errors(),
        "expected diagnostic errors for direct cycle"
    );
    assert!(
        has_e0010(&sink),
        "expected E0010 for direct cycle, diagnostics: {:?}",
        sink.diagnostics()
    );
}

// ─── 7. cycle_indirect.nv → E0010 emitted ────────────────────────────────────

#[test]
fn resolver_cycle_indirect_emits_e0010() {
    let (_, sink) = resolve_fixture("tests/fixtures/ambiguous_decls/cycle_indirect.nv");
    assert!(
        sink.has_errors(),
        "expected diagnostic errors for indirect cycle"
    );
    assert!(
        has_e0010(&sink),
        "expected E0010 for indirect cycle, diagnostics: {:?}",
        sink.diagnostics()
    );
}

// ─── 8. component_vs_function.nv ─────────────────────────────────────────────
//   Splash  → component (BareDecl kept)
//   loading_screen → Function
//   Screen  → Struct

#[test]
fn resolver_component_vs_function() {
    let (prog, sink) =
        resolve_fixture("tests/fixtures/ambiguous_decls/component_vs_function.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());

    // loading_screen: return type present → function
    let loading = find_item(&prog.items, "loading_screen");
    assert!(
        is_function(loading),
        "`loading_screen` should be Function, got {loading:?}"
    );

    // Screen: all fields → struct
    let screen = find_item(&prog.items, "Screen");
    assert!(is_struct(screen), "`Screen` should be Struct, got {screen:?}");

    // Splash: body is a call to a function → component (BareDecl kept)
    let splash = find_item(&prog.items, "Splash");
    assert!(
        is_bare_decl(splash),
        "`Splash` should be kept as BareDecl (component), got {splash:?}"
    );
}

// ─── 9. structs/basic.nv → all top-level items are Struct ────────────────────

#[test]
fn resolver_structs_basic_all_struct() {
    let (prog, sink) = resolve_fixture("tests/fixtures/structs/basic.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());

    for item in &prog.items {
        // explicit `struct` items are already Struct at parse time;
        // bare colon-block items should also be classified as Struct.
        assert!(
            is_struct(item),
            "expected all items to be Struct, got `{}` as {item:?}",
            item_name(item)
        );
    }
    assert!(
        !prog.items.is_empty(),
        "expected at least one item in structs/basic.nv"
    );
}

// ─── 10. functions/basic.nv → all top-level items are Function ───────────────

#[test]
fn resolver_functions_basic_all_function() {
    let (prog, sink) = resolve_fixture("tests/fixtures/functions/basic.nv");
    assert!(!sink.has_errors(), "unexpected errors: {:?}", sink.diagnostics());

    for item in &prog.items {
        assert!(
            is_function(item),
            "expected all items to be Function, got `{}` as {item:?}",
            item_name(item)
        );
    }
    assert!(
        !prog.items.is_empty(),
        "expected at least one item in functions/basic.nv"
    );
}

// ─── 11. enums_match/basic.nv → parses and resolves without errors ───────────

#[test]
fn resolver_enums_match_basic_no_errors() {
    let (prog, sink) = resolve_fixture("tests/fixtures/enums_match/basic.nv");
    assert!(
        !sink.has_errors(),
        "unexpected errors in enums_match/basic.nv: {:?}",
        sink.diagnostics()
    );
    // The file has enums and explicit `fn` declarations — all should resolve
    // cleanly. Enums are always explicit (not BareDecl), so they pass through.
    assert!(
        !prog.items.is_empty(),
        "expected items in enums_match/basic.nv"
    );
    for item in &prog.items {
        assert!(
            is_function(item) || is_enum(item),
            "expected Function or Enum items, got `{}` as {item:?}",
            item_name(item)
        );
    }
}

// ─── 12. result_option/basic.nv → parses and resolves without errors ─────────

#[test]
fn resolver_result_option_basic_no_errors() {
    let (prog, sink) = resolve_fixture("tests/fixtures/result_option/basic.nv");
    assert!(
        !sink.has_errors(),
        "unexpected errors in result_option/basic.nv: {:?}",
        sink.diagnostics()
    );
    assert!(
        !prog.items.is_empty(),
        "expected items in result_option/basic.nv"
    );
    // All top-level declarations in this fixture are functions.
    for item in &prog.items {
        assert!(
            is_function(item),
            "expected all items to be Function, got `{}` as {item:?}",
            item_name(item)
        );
    }
}
