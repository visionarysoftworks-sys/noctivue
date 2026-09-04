//! Generate TextMate grammar (.tmLanguage.json) from compiler's token definitions.
//! Run with: `cargo run --bin gen_syntax_highlighting`

use std::fs;
use serde::Serialize;

fn main() {
    // Keywords from lexer/mod.rs: keyword_or_ident()
    let core_keywords = [
        "as", "async", "await", "break", "const", "continue",
        "else", "export", "for", "if", "import",
        "in", "let", "loop", "match", "mod", "return",
        "use", "var", "while",
    ];

    let type_keywords = [
        "enum", "fn", "impl", "struct", "trait", "type", "unsafe",
    ];

    let memory_keywords = [
        "owned", "borrow", "managed", "weak", "unowned", "task",
    ];

    let reserved_keywords = [
        "actor", "defer", "extern", "macro", "native", "operator",
        "protocol", "reflect", "spawn", "static", "where", "yield",
    ];

    let literals = ["true", "false"];

    // Operators from lexer/token.rs.
    // NOTE: multi-character operators MUST come before their single-character
    // prefixes — TextMate tries patterns in order at each position, so `->`
    // would otherwise tokenize as `-` + `>` and `==` as `=` + `=`.
    let operators = [
        "==", "!=", "<=", ">=", "&&", "||",
        "+=", "-=", "*=", "/=", "%=",
        "..=", "..",
        "??", "->", "::",
        "+", "-", "*", "/", "%",
        "<", ">", "!",
        "=",
        "?", ".",
        ":", ",", ";",
        "(", ")",
        "[", "]",
        "{", "}",
    ];

    let mut patterns: Vec<Pattern> = Vec::new();

    // 1. Comments
    patterns.push(Pattern::simple("comment.line.double-slash.noctivue", r"//.*$"));
    patterns.push(Pattern::complex(
        "comment.block.noctivue",
        r"/\*",
        r"\*/",
        vec![Pattern::simple("comment.block.noctivue", r"[^*]*\*+([^/*][^*]*\*+)*")],
    ));
    patterns.push(Pattern::simple("comment.documentation.noctivue", r"///.*$"));

    // 2. String literals
    patterns.push(Pattern::complex(
        "string.quoted.double.noctivue",
        r#"""#,
        r#"""#,
        vec![
            Pattern::simple("constant.character.escape.noctivue", r#"\["nrt]"#),
            // Interpolation `{expr}`: inner identifiers get variable scope
            // rather than inheriting the punctuation scope.
            Pattern::complex(
                "punctuation.definition.interpolation.begin.noctivue",
                r"\{",
                r"\}",
                vec![
                    Pattern::simple("entity.name.type.noctivue", r"\b[A-Z][a-zA-Z0-9_]*\b"),
                    Pattern::simple("variable.other.noctivue", r"\b[a-z_][a-zA-Z0-9_]*\b"),
                    Pattern::simple("constant.numeric.integer.noctivue", r"\b\d[\d_]*\b"),
                ],
            ),
        ],
    ));

    // 3. Character literals
    patterns.push(Pattern::simple("string.quoted.single.noctivue", r"'([^'\\]|\\.)*'"));

    // 4. Numbers
    patterns.push(Pattern::simple("constant.numeric.float.noctivue", r"\b\d+\.\d+([eE][+-]?\d+)?\b"));
    patterns.push(Pattern::simple("constant.numeric.integer.noctivue", r"\b(0x[0-9a-fA-F_]+|0b[01_]+|0o[0-7_]+|\d[\d_]*)\b"));

    // 5. Keywords
    for kw in &core_keywords {
        patterns.push(Pattern::simple("keyword.control.noctivue", format!(r"\b{}\b", regex::escape(kw))));
    }
    for kw in &type_keywords {
        patterns.push(Pattern::simple("keyword.other.noctivue", format!(r"\b{}\b", regex::escape(kw))));
    }
    for kw in &memory_keywords {
        patterns.push(Pattern::simple("keyword.other.memory.noctivue", format!(r"\b{}\b", regex::escape(kw))));
    }
    for kw in &reserved_keywords {
        patterns.push(Pattern::simple("keyword.other.reserved.noctivue", format!(r"\b{}\b", regex::escape(kw))));
    }
    for kw in &literals {
        patterns.push(Pattern::simple("constant.language.noctivue", format!(r"\b{}\b", regex::escape(kw))));
    }

    // 6. Operators
    for op in &operators {
        let escaped = regex::escape(op);
        let scope = if ["+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "&&", "||", "!"].contains(op) {
            "keyword.operator.arithmetic.noctivue"
        } else if ["=", "+=", "-=", "*=", "/=", "%="].contains(op) {
            "keyword.operator.assignment.noctivue"
        } else if ["..", "..="].contains(op) {
            "keyword.operator.range.noctivue"
        } else if ["?", "??"].contains(op) {
            "keyword.operator.optional.noctivue"
        } else if *op == "->" {
            "keyword.operator.arrow.noctivue"
        } else {
            "punctuation.separator.noctivue"
        };
        // NOTE: `regex::escape` output is already a complete, correct regex
        // (`(` -> `\(`). Do NOT prepend another backslash — that produces
        // `\\(` which is a hard Oniguruma compile error ("unmatched
        // parenthesis") and silently breaks the whole TextMate grammar load.
        patterns.push(Pattern::simple(scope, escaped));
    }

    // 7. Identifiers (types start with uppercase, functions/variables lowercase).
    // NOTE: the function-call pattern comes first so `name(` tokenizes as a
    // function, not a plain variable — TextMate uses the first match in order.
    patterns.push(Pattern::simple("entity.name.function.noctivue", r"\b[a-z_][a-zA-Z0-9_]*\b(?=\()"));
    patterns.push(Pattern::simple("entity.name.type.noctivue", r"\b[A-Z][a-zA-Z0-9_]*\b"));
    patterns.push(Pattern::simple("variable.other.noctivue", r"\b[a-z_][a-zA-Z0-9_]*\b"));

    // Build the grammar
    let grammar = Grammar {
        scope_name: "source.noctivue".to_string(),
        name: "Noctivue".to_string(),
        file_types: vec!["nv".to_string()],
        patterns,
    };

    let json = serde_json::to_string_pretty(&grammar).expect("serialize grammar");
    // Write into the workspace's VS Code extension (editors/vscode), resolved
    // from this crate's manifest dir so it works on any machine/checkout.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output_path =
        manifest_dir.join("../editors/vscode/syntaxes/noctivue.tmLanguage.json");
    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).expect("create syntaxes dir");
    }
    fs::write(&output_path, json).expect("write grammar");
    println!("Generated: {}", output_path.display());
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Grammar {
    scope_name: String,
    name: String,
    file_types: Vec<String>,
    patterns: Vec<Pattern>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Pattern {
    Simple(SimplePattern),
    Complex(ComplexPattern),
}

impl Pattern {
    fn simple(name: impl Into<String>, match_: impl Into<String>) -> Self {
        Pattern::Simple(SimplePattern { name: name.into(), r#match: match_.into() })
    }
    fn complex(name: impl Into<String>, begin: impl Into<String>, end: impl Into<String>, patterns: Vec<Pattern>) -> Self {
        Pattern::Complex(ComplexPattern { name: name.into(), begin: begin.into(), end: end.into(), patterns })
    }
}

#[derive(Serialize)]
struct SimplePattern {
    name: String,
    #[serde(rename = "match")]
    r#match: String,
}

#[derive(Serialize)]
struct ComplexPattern {
    name: String,
    begin: String,
    end: String,
    patterns: Vec<Pattern>,
}