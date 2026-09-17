//! Derive mechanism tests — Phase 5/M4 (ADR-018).
//!
//! `derive Serialize for T:` / `derive Deserialize for T:` expand in
//! the resolver to `to_json_<T>` / `from_json_<T>` plus a marker
//! impl. Coverage: full kind round trips, nesting, options, lists,
//! error naming (`Type.field`), every loud-failure code, and the
//! scope requirements (`JsonDoc`, marker traits).

use compiler::diagnostics::DiagnosticSink;

const DERIVE_LIBS: &[&str] = &[
    "stdlib/core/convert.nv",
    "stdlib/core/compare.nv",
    "stdlib/option/option.nv",
    "stdlib/result/result.nv",
    "stdlib/strings/string.nv",
    "stdlib/strings/format.nv",
    "stdlib/testing/assertions.nv",
    "stdlib/collections/list.nv",
    "stdlib/encoding/json/serialize.nv",
    "stdlib/encoding/json/document.nv",
];

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

fn join(sources: &[String]) -> String {
    let mut out = String::new();
    for s in sources {
        out.push_str(s);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn error_texts(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let code = d.code.as_deref().unwrap_or("?");
            format!("[{code}] {}", d.message)
        })
        .collect()
}

fn error_codes(sink: &DiagnosticSink) -> Vec<String> {
    sink.diagnostics()
        .iter()
        .filter(|d| d.is_error())
        .filter_map(|d| d.code.clone())
        .collect()
}

fn pipeline(source: &str) -> (compiler::hir::Module, DiagnosticSink) {
    let mut sink = DiagnosticSink::new();
    let tokens = compiler::lexer::lex(source, &mut sink);
    let program = compiler::parser::parse(&tokens, &mut sink);
    let program = compiler::resolver::resolve(program, &mut sink);
    let module = compiler::typeck::typecheck(program, &mut sink);
    (module, sink)
}

fn check(source: &str, what: &str) -> compiler::hir::Module {
    let (module, sink) = pipeline(source);
    let errors = error_texts(&sink);
    assert!(
        errors.is_empty(),
        "{what} produced error diagnostics:\n  {}",
        errors.join("\n  ")
    );
    module
}

fn run_ok(module: &compiler::hir::Module, what: &str) {
    let mut sink = DiagnosticSink::new();
    let mut interp = interp::Interpreter::new();
    let exit = interp.run(module, &mut sink);
    let errors = error_texts(&sink);
    assert!(
        exit == 0,
        "[{what}] interpreter exit code {exit} (expected 0):\n  {}",
        errors.join("\n  ")
    );
}

fn libs() -> Vec<String> {
    DERIVE_LIBS.iter().map(|p| read(p)).collect()
}

#[test]
fn derive_surface_zero_errors() {
    let sources: Vec<String> = DERIVE_LIBS.iter().map(|p| read(p)).collect();
    let _ = check(&join(&sources), "derive_surface");
}

#[test]
fn derive_round_trip_all_kinds() {
    // `.nv` strings cannot spell `"` (no `\"` escape), so exact
    // wire text is never asserted as a literal: encode, decode,
    // re-encode, compare the two runtime strings, then check every
    // decoded value against plain literals.
    let mut sources = libs();
    sources.push(
        r#"User:
    name: String
    age: Int
    admin: Bool
    score: Float
    grade: Char
    nick: Option<String>
    tags: [String]

derive Serialize for User:
derive Deserialize for User:

fn round_trip(u: User) -> User:
    var out = User { name: "", age: 0, admin: false, score: 0.0, grade: ' ', nick: None, tags: [] }
    match doc_parse(to_json_User(u)):
        Ok(d):
            match from_json_User(d):
                Ok(back):
                    out = back
                Err(e):
                    assert(false, "decode failed")
        Err(e):
            assert(false, "encode produced invalid JSON")
    out

fn main():
    let u = User { name: "Ada", age: 36, admin: true, score: 9.5, grade: 'A', nick: None, tags: ["x", "y"] }
    let back = round_trip(u)
    assert_string_eq(to_json_User(back), to_json_User(u))
    assert_string_eq(back.name, "Ada")
    assert_int_eq(back.age, 36)
    assert_true(back.admin, "admin")
    assert_true(back.score == 9.5, "score")
    assert_true(back.grade == 'A', "grade")
    match back.nick:
        Some(_):
            assert(false, "nick should be None")
        None:
            assert(true, "")
    match list_len_string(back.tags):
        2:
            assert(true, "")
        _:
            assert(false, "tags length")
    let u2 = User { name: "Bo", age: 41, admin: false, score: 1.5, grade: 'B', nick: Some("bobby"), tags: [] }
    let back2 = round_trip(u2)
    assert_string_eq(to_json_User(back2), to_json_User(u2))
    match back2.nick:
        Some(n):
            assert_string_eq(n, "bobby")
        None:
            assert(false, "nick should be Some")
"#
        .to_string(),
    );
    let module = check(&join(&sources), "derive_round_trip");
    run_ok(&module, "derive_round_trip");
}

#[test]
fn derive_nested_options_lists() {
    let mut sources = libs();
    sources.push(
        r#"Address:
    city: String
    zip: Int

Order:
    id: Int
    ship_to: Address
    gift: Option<Address>
    counts: [Int]
    notes: Option<[String]>

derive Serialize for Address:
derive Deserialize for Address:
derive Serialize for Order:
derive Deserialize for Order:

fn check_order(o: Order) -> Unit:
    match doc_parse(to_json_Order(o)):
        Ok(d):
            match from_json_Order(d):
                Ok(back):
                    assert_string_eq(to_json_Order(back), to_json_Order(o))
                Err(e):
                    assert(false, "order decode failed")
        Err(e):
            assert(false, "order encode invalid")

fn main():
    let home = Address { city: "Berlin", zip: 10115 }
    let full = Order { id: 7, ship_to: home, gift: Some(home), counts: [1, 2, 3], notes: Some(["fragile"]) }
    check_order(full)
    let bare = Order { id: 8, ship_to: home, gift: None, counts: [], notes: None }
    check_order(bare)
    match doc_parse(to_json_Order(full)):
        Ok(d):
            match from_json_Order(d):
                Ok(back):
                    assert_string_eq(back.ship_to.city, "Berlin")
                    assert_int_eq(back.ship_to.zip, 10115)
                    match back.gift:
                        Some(g):
                            assert_string_eq(g.city, "Berlin")
                        None:
                            assert(false, "gift should be Some")
                    match back.notes:
                        Some(n):
                            assert_int_eq(list_len_string(n), 1)
                        None:
                            assert(false, "notes should be Some")
                Err(e):
                    assert(false, "decode failed")
        Err(e):
            assert(false, "encode invalid")
"#
        .to_string(),
    );
    let module = check(&join(&sources), "derive_nested");
    run_ok(&module, "derive_nested");
}

#[test]
fn derive_single_field_shape() {
    let mut sources = libs();
    sources.push(
        r#"Empty:
    x: Int

derive Serialize for Empty:
derive Deserialize for Empty:

fn main():
    match doc_parse(to_json_Empty(Empty { x: 1 })):
        Ok(d):
            match from_json_Empty(d):
                Ok(back):
                    assert_int_eq(back.x, 1)
                Err(e):
                    assert(false, "decode failed")
        Err(e):
            assert(false, "encode invalid")
"#
        .to_string(),
    );
    // Single-field struct: the minimal shape (empty structs have no
    // fields to synthesize around; the shape is covered by construction).
    let module = check(&join(&sources), "derive_empty");
    run_ok(&module, "derive_empty");
}

#[test]
fn derive_missing_field_names_type_and_field() {
    let mut sources = libs();
    sources.push(
        r#"User:
    name: String
    age: Int

derive Serialize for User:
derive Deserialize for User:

fn main():
    match doc_parse(json_object_string_pair_builtin("name", "Cy", "x", "y")):
        Ok(d):
            match from_json_User(d):
                Ok(_):
                    assert(false, "missing age passed")
                Err(e):
                    assert_true(string_contains(e, "User.age"), "names type and field")
        Err(e):
            assert(false, "fixture invalid")
"#
        .to_string(),
    );
    let module = check(&join(&sources), "derive_missing");
    run_ok(&module, "derive_missing");
}

fn check_codes(source: &str, what: &str, want: &[&str]) {
    let (_, sink) = pipeline(source);
    let codes = error_codes(&sink);
    for w in want {
        assert!(
            codes.iter().any(|c| c == w),
            "[{what}] expected error {w}, got: {codes:?}\n{texts}",
            texts = error_texts(&sink).join("\n  ")
        );
    }
}

#[test]
fn derive_rejections() {
    // Scope-gated content errors get the marker traits preamble so
    // they reach field checking (scope failures are covered by
    // derive_scope_requirements_are_loud).
    let ser = read("stdlib/encoding/json/serialize.nv") + "\n";
    // `deriave` is just an identifier here: loud, whatever the code.
    check_codes(
        "S:\n    x: Int\nderiave Clone for S:\n",
        "typo",
        &["E0102"],
    );
    check_codes(
        "S:\n    x: Int\nderive Clone for S:\n",
        "bad_trait",
        &["E0320"],
    );
    // E0321: enum targets are a loud not-yet.
    check_codes(
        &(ser.clone() + "enum Color:\n    Red\n    Blue\nderive Serialize for Color:\n"),
        "enum_target",
        &["E0321"],
    );
    // E0322: unknown target (and the not-a-struct hint shape).
    check_codes(
        &(ser.clone() + "derive Serialize for Missing:\n"),
        "unknown",
        &["E0322"],
    );
    // E0323: generic structs.
    check_codes(
        &(ser.clone() + "struct Box<T>:\n    x: T\nderive Serialize for Box:\n"),
        "generic",
        &["E0323"],
    );
    // E0324: duplicates.
    check_codes(
        &(ser.clone() + "S:\n    x: Int\nderive Serialize for S:\nderive Serialize for S:\n"),
        "duplicate",
        &["E0324"],
    );
    // E0325: unsupported field types.
    check_codes(
        &(ser.clone() + "S:\n    r: Result<Int, String>\nderive Serialize for S:\n"),
        "result_field",
        &["E0325"],
    );
    check_codes(
        &(ser.clone() + "S:\n    f: (Int) -> String\nderive Serialize for S:\n"),
        "fn_field",
        &["E0325"],
    );
    // E0326: nested type without the matching derive.
    check_codes(
        &(ser.clone()
            + "Inner:\n    x: Int\nOuter:\n    inner: Inner\nderive Serialize for Outer:\n"),
        "missing_nested",
        &["E0326"],
    );
    // E0327: chained failure (Inner fails E0325, Outer follows).
    check_codes(
        &(ser.clone()
            + "Inner:\n    r: Result<Int, String>\nOuter:\n    inner: Inner\nderive Serialize for Inner:\nderive Serialize for Outer:\n"),
        "chained",
        &["E0325", "E0327"],
    );
    // Non-empty derive body + statement-level derive.
    check_codes(
        "S:\n    x: Int\nderive Serialize for S:\n    junk\n",
        "nonempty_body",
        &["E0100"],
    );
    check_codes(
        "fn main():\n    derive Serialize for S:\n",
        "stmt_level",
        &["E0100"],
    );
}

#[test]
fn derive_scope_requirements_are_loud() {
    // No serialize.nv: the marker trait is missing (E0328 names the file).
    let (_, sink) = pipeline("S:\n    x: Int\nderive Serialize for S:\nfn main():\n    1\n");
    let codes = error_codes(&sink);
    assert!(
        codes.iter().any(|c| c == "E0328"),
        "expected E0328 (missing Serialize trait), got: {codes:?}"
    );
    // serialize.nv but no document.nv: JsonDoc missing for Deserialize.
    let doc_less = read("stdlib/encoding/json/serialize.nv")
        + "\nS:\n    x: Int\nderive Deserialize for S:\nfn main():\n    1\n";
    let (_, sink) = pipeline(&doc_less);
    let codes = error_codes(&sink);
    assert!(
        codes.iter().any(|c| c == "E0328"),
        "expected E0328 (missing JsonDoc), got: {codes:?}"
    );
}
