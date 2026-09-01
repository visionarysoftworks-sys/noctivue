//! Name resolution — binds identifiers to their definitions and classifies
//! bare declarations by body shape.
//!
//! Responsibilities (COMPILER_ARCHITECTURE.md §3):
//! - Scope construction and identifier binding.
//! - Import resolution.
//! - **Declaration-kind disambiguation** via [`classify`]:
//!   [`crate::ast::BareDecl`] nodes are classified as struct, function, or
//!   component based on body shape alone — never based on naming convention
//!   (COMPILER_ARCHITECTURE.md §4, DECISIONS.md Issue 1/2).
//!
//! ## Classification cycle detection
//!
//! The classification of some `BareDecl`s depends on what other names in the
//! same scope resolve to (the "transitive resolution order" problem,
//! COMPILER_ARCHITECTURE.md §4, EXECUTION_GUIDE.md Phase 0). If a genuine
//! cycle is detected — `A`'s classification depends on `B`'s, which depends
//! on `A`'s — the resolver MUST emit a `"declaration classification cycle"`
//! diagnostic and MUST NOT loop infinitely or silently guess.

pub mod classify;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::ast::{BareDecl, Item, Program};
use crate::diagnostics::DiagnosticSink;

use classify::{classify, ClassificationStatus, Classification};

/// Run name resolution and declaration classification over an unresolved [`Program`].
///
/// Returns a new `Program` where all resolvable `BareDecl` items have been
/// replaced by their classified counterparts (`Item::Struct`, `Item::Function`,
/// or kept as `Item::BareDecl` for components). Errors are emitted into `sink`.
pub fn resolve(program: Program, sink: &mut DiagnosticSink) -> Program {
    // ── Step 1: collect all BareDecl items into a lookup map ─────────────────
    let all_decls: HashMap<String, BareDecl> = program
        .items
        .iter()
        .filter_map(|item| {
            if let Item::BareDecl(decl) = item {
                Some((decl.name.clone(), decl.clone()))
            } else {
                None
            }
        })
        .collect();

    // ── Step 2: classify each BareDecl (with shared memoisation + cycle det.) ─
    let mut status: HashMap<String, ClassificationStatus> = HashMap::new();

    // Pre-run classification for all bare decls so the status map is fully
    // populated before we rewrite the item list.
    for decl in all_decls.values() {
        classify(decl, &all_decls, &mut status, sink);
    }

    // ── Step 3: rewrite program.items, replacing BareDecls with results ───────
    let new_items = program
        .items
        .into_iter()
        .map(|item| match item {
            Item::BareDecl(ref decl) => {
                match status.get(&decl.name) {
                    Some(ClassificationStatus::Known(Classification::Struct(s))) => {
                        Item::Struct(s.clone())
                    }
                    Some(ClassificationStatus::Known(Classification::Function(f))) => {
                        Item::Function(f.clone())
                    }
                    // Component or Cycle: keep as BareDecl
                    _ => item,
                }
            }
            // Non-BareDecl items pass through unchanged.
            other => other,
        })
        .collect();

    Program {
        imports: program.imports,
        items: new_items,
    }
}
