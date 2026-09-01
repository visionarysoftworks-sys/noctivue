# SYNTAX.md — Syntax Reference & Formal Grammar

**Status:** Experimental core grammar (Proposed, pending prototype
validation per DECISIONS.md Issue 1/2).

## 1. Design Summary

Noctivue's primary visual identity is **colon-first structure**:

```nv
User:
    id: Int
    name: String

calculate(x: Int) -> Int:
    x * 2

Dashboard:
    column:
        text("Analytics")
        button("Refresh"): refresh()
```

A colon introduces structure. Indentation (or braces) expresses
hierarchy. Keywords express behavior, not declaration kind — see
COMPILER_ARCHITECTURE.md §4 for how the parser and resolver jointly
determine *what kind* of declaration a colon-block is without guessing
from naming convention.

## 2. Indentation Mode

```nv
if ready:
    start()
    connect()

print("Done")
```

`print("Done")` is outside the `if` because it returns to the enclosing
indentation level. A dedent to a level that does not match any
enclosing block is a compile error (LANGUAGE_SPEC.md §2).

## 3. Three Density Levels

These represent the same underlying AST wherever the construct allows
all three (ADR-008):

**Minimal** (expression statement, no block needed):
```nv
text("Hello")
```

**Compact** (single-line block body after `:`):
```nv
button("Save"): save()
```

**Expanded** (multi-line block body):
```nv
button:
    text: "Save"
    icon: "save"
    disabled: saving

    on_click:
        save()
```

## 4. Concise vs. Explicit Declaration Style

Fully equivalent pairs (ADR-007/ADR-008):

```nv
User:                  struct User:
    id: Int                id: Int

calculate(x: Int) -> Int:      fn calculate(x: Int) -> Int:
    x * 2                          x * 2
```

Behavioral/control-flow keywords (`if`, `for`, `while`, `match`,
`return`, `break`, `continue`, `async`, `await`, `import`, `export`)
remain mandatory in both styles — they are never optional, because they
carry control-flow semantics rather than declaration-kind information.

## 5. Compact Multi-Statement Forms

```nv
row: icon("check", size: 18); text("Payment successful", size: 14); spacer(8)

main: start(); connect(); serve()
```

Semicolons are optional statement separators, used only for horizontal
compaction. They carry no independent semantic meaning (DECISIONS.md
Issue 5) and the formatter treats their use as a formatting decision.

## 6. Inline / Braced Forms

```nv
button(text: "Save"): save()          // inline form

button: {                              // braced form
    text: "Save"
    on_click:
        save()
}

main: { start(); connect() }           // single-line braces
```

Braces and indentation express the same block semantics
(LANGUAGE_SPEC.md §2). Braces are never mandatory (design brief §7).

## 7. Long Lines & Formatting

There is no language-level line-length limit. Formatting (when to break
a call's arguments across lines) is entirely the formatter's
responsibility, and `noct fmt` **MUST** be able to transform between
compact and expanded representations of the same construct without
changing semantics (ADR-008):

```nv
card(title: "Monthly Revenue", value: "$128,420", change: "+12.4%", icon: "trending-up")
```

may format as:

```nv
card(
    title: "Monthly Revenue",
    value: "$128,420",
    change: "+12.4%",
    icon: "trending-up"
)
```

## 8. Keyword Table

See LANGUAGE_SPEC.md §7 for the full reserved-word table.

## 9. Operator Precedence (highest to lowest)

| Precedence | Operators | Associativity |
|---|---|---|
| 1 (highest) | `.` `::` `()` `[]` (call, index, member/path access) | left |
| 2 | unary `-` `!` | right |
| 3 | `*` `/` `%` | left |
| 4 | `+` `-` | left |
| 5 | `..` `..=` (range) | none (non-associative) |
| 6 | `<` `<=` `>` `>=` | left |
| 7 | `==` `!=` | left |
| 8 | `&&` | left |
| 9 | `\|\|` | left |
| 10 | `??` (optional-coalesce) | left |
| 11 | `?` (error propagation, postfix) | n/a (postfix) |
| 12 (lowest) | `=` `+=` `-=` `*=` `/=` `%=` | right |

## 10. Formal Grammar (EBNF, initial draft)

This grammar is intentionally incomplete in areas marked `PROPOSED`/
`OPEN` elsewhere in this documentation set (notably generics bounds
syntax and managed-mode declaration syntax); it is sufficient to parse
the example program in `examples/dashboard.nv`.

```ebnf
(* ===== Top level ===== *)
program        = { import_decl } , { top_decl } ;

import_decl    = "import" , path , [ "as" , identifier ] , newline ;
export_decl    = "export" , top_decl ;
path           = identifier , { "::" , identifier } ;

top_decl       = export_decl
                | function_decl
                | struct_decl
                | enum_decl
                | trait_decl
                | impl_decl
                | component_decl
                | const_decl ;

(* ===== Blocks: three equivalent forms ===== *)
block          = indented_block | braced_block | inline_block ;
indented_block = ":" , newline , INDENT , { statement } , DEDENT ;
braced_block   = ":" , "{" , { statement | ";" } , "}" ;
inline_block   = ":" , statement_list_inline ;
statement_list_inline
               = statement , { ";" , statement } ;

(* ===== Declarations ===== *)
(* Concise and explicit forms lower to the same AST node; the explicit
   keyword, when present, is a parse-time hint that removes any need
   for resolver-side inference (COMPILER_ARCHITECTURE.md §4). *)

struct_decl    = [ "struct" ] , type_identifier , generic_params? , block_fields ;
block_fields   = ":" , newline , INDENT , { field_decl } , DEDENT
               | ":" , "{" , { field_decl } , "}" ;
field_decl     = identifier , ":" , type_expr , newline ;

enum_decl      = "enum" , type_identifier , generic_params? ,
                 ":" , newline , INDENT , { enum_variant } , DEDENT ;
enum_variant   = type_identifier , [ "(" , type_list , ")" ] , newline ;

trait_decl     = "trait" , type_identifier , generic_params? ,
                 ":" , newline , INDENT , { trait_member } , DEDENT ;
trait_member   = function_sig , newline ;

impl_decl      = "impl" , type_expr , [ "for" , type_expr ] ,
                 ":" , newline , INDENT , { function_decl } , DEDENT ;

function_decl  = [ "fn" ] , identifier , generic_params? ,
                 "(" , [ param_list ] , ")" ,
                 [ "->" , type_expr ] , function_body ;
function_sig   = [ "fn" ] , identifier , generic_params? ,
                 "(" , [ param_list ] , ")" , [ "->" , type_expr ] ;
function_body  = block | ":" , expression ;   (* single-expr body: `x * 2` *)
param_list     = param , { "," , param } ;
param          = identifier , ":" , type_expr , [ "=" , expression ] ;

const_decl     = "const" , identifier , ":" , type_expr , "=" , expression , newline ;

(* Component declarations (UI) are NOT a separate grammar production:
   a bare `TypeIdentifier:` with a body composed of nested calls
   resolves to either a struct or a component during name resolution,
   per DECISIONS.md Issue 2. Grammatically, component_decl == struct_decl
   with a builder-call body; this line documents intent, not a new rule. *)
component_decl = struct_decl ;

(* ===== Statements ===== *)
statement      = let_stmt | var_stmt | assign_stmt | expr_stmt
               | if_stmt | for_stmt | while_stmt | loop_stmt
               | match_stmt | return_stmt | break_stmt | continue_stmt
               | function_decl | struct_decl | state_decl ;

let_stmt       = "let" , identifier , [ ":" , type_expr ] , "=" , expression ;
var_stmt       = "var" , identifier , [ ":" , type_expr ] , "=" , expression ;
state_decl     = "state" , identifier , "=" , expression ;   (* UI framework sugar, see UI_SPEC.md *)
assign_stmt    = lvalue , assign_op , expression ;
assign_op      = "=" | "+=" | "-=" | "*=" | "/=" | "%=" ;
expr_stmt      = expression ;

return_stmt    = "return" , [ expression ] ;
break_stmt     = "break" , [ expression ] ;
continue_stmt  = "continue" ;

(* ===== Control flow ===== *)
if_stmt        = "if" , expression , block , { "else" , "if" , expression , block } ,
                 [ "else" , block ] ;
while_stmt     = "while" , expression , block ;
loop_stmt      = "loop" , block ;
for_stmt       = "for" , identifier , "in" , expression , block ;
match_stmt     = "match" , expression , ":" , newline , INDENT , { match_arm } , DEDENT ;
match_arm      = pattern , [ "if" , expression ] , block_or_expr ;
block_or_expr  = ":" , expression | block ;
pattern        = literal | identifier | type_identifier , [ "(" , pattern_list , ")" ] | "_" ;
pattern_list   = pattern , { "," , pattern } ;

(* ===== Expressions ===== *)
expression     = assignment_expr ;   (* precedence per §9 *)
call_expr      = ( identifier | member_expr ) , "(" , [ arg_list ] , ")" , [ block ] ;
arg_list       = arg , { "," , arg } ;
arg            = [ identifier , ":" ] , expression ;   (* named args, e.g. title: "..." *)
member_expr    = expression , "." , identifier ;
index_expr     = expression , "[" , expression , "]" ;
try_expr       = expression , "?" ;
range_expr     = expression , ( ".." | "..=" ) , expression ;
string_interp  = '"' , { char | "{" , expression , "}" } , '"' ;

(* ===== Types ===== *)
type_expr      = type_identifier , [ generic_args ]
               | "Option" , "<" , type_expr , ">"
               | "Result" , "<" , type_expr , "," , type_expr , ">"
               | "(" , type_list , ")"                       (* tuple *)
               | "[" , type_expr , "]"                        (* collection, see TYPE_SYSTEM.md *)
               | function_type ;
function_type  = "(" , [ type_list ] , ")" , "->" , type_expr ;
generic_params = "<" , generic_param_list , ">" ;
generic_param_list = identifier , [ ":" , trait_bound_list ] , { "," , identifier , [ ":" , trait_bound_list ] } ;
trait_bound_list = type_identifier , { "+" , type_identifier } ;
generic_args   = "<" , type_list , ">" ;
type_list      = type_expr , { "," , type_expr } ;

(* ===== Lexical terminals ===== *)
identifier      = XID_Start , { XID_Continue } ;
type_identifier = identifier ;  (* PascalCase is a style convention, not a grammar rule *)
literal         = int_literal | float_literal | bool_literal | char_literal | string_literal ;
```

### 10.1 Notes on `INDENT`/`DEDENT`

`INDENT` and `DEDENT` are synthesized by the lexer (an "offside rule"
pass) before the token stream reaches the parser, the same general
approach used by Python and F#. Braced blocks (`{ }`) suppress this pass
for their contents, so indentation inside a brace-delimited block is
purely cosmetic.

## 11. What This Grammar Does *Not* Yet Resolve

Per DECISIONS.md Issues 1–2, this grammar is **syntactically**
unambiguous (a parser can be generated from it as-is), but two
**semantic** questions are intentionally left to name resolution rather
than encoded here:

1. Whether a bare `TypeIdentifier:` block is a `struct` or a UI
   component (both parse as `component_decl == struct_decl`).
2. Whether a lowercase `identifier(...):` is a plain function or a
   builder-style call that takes a trailing closure (both parse as
   `call_expr` with an optional trailing `block`).

These are **Open** and tracked in DECISIONS.md, not silently assumed
here.
