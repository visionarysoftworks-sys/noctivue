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
//! - Trait resolution: collect trait declarations and verify impl blocks.

pub mod classify;
pub mod derive;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::ast::{BareDecl, Item, Program, TraitDecl, ImplBlock, TypeExpr};
use crate::diagnostics::{Diagnostic, DiagnosticSink};

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

    // ── Step 1b: collect all TraitDecl and ImplBlock items ───────────────────
    let mut traits: HashMap<String, &TraitDecl> = HashMap::new();
    let mut impls: Vec<&ImplBlock> = Vec::new();
    for item in &program.items {
        match item {
            Item::Trait(t) => { traits.insert(t.name.clone(), t); }
            Item::Impl(i) => { impls.push(i); }
            _ => {}
        }
    }

    // Verify impl blocks against their traits
    for impl_block in &impls {
        if let Some(trait_ty) = &impl_block.for_trait {
            if let TypeExpr::Named(trait_name, _, _) = trait_ty {
                if let Some(trait_decl) = traits.get(trait_name) {
                    // Verify all trait methods are implemented
                    for trait_method in &trait_decl.members {
                        let found = impl_block.methods.iter().any(|m| m.name == trait_method.name);
                        if !found {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "missing method `{}` in impl of trait `{}` for type",
                                    trait_method.name, trait_name
                                ))
                                .with_span(impl_block.span.clone(), "here")
                                .with_code("E0300"),
                            );
                        }
                    }
                    // Verify no extra methods (optional - could be inherent methods)
                    // For now we allow extra methods
                } else {
                    sink.emit(
                        Diagnostic::error(format!("trait `{}` not found", trait_name))
                            .with_span(trait_ty.clone().span(), "here")
                            .with_code("E0301"),
                    );
                }
            }
        }
    }

    // ── Step 1c: collect ModDecl items for potential nested resolution ──────────
    // For M0, we just pass through modules without nested resolution.
    // A full module system would recursively resolve nested items.

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

    // ── Step 4: expand `derive` declarations (ADR-018) ───────────────────
    // Runs on the classified program so targets resolve to structs;
    // consumes `Item::Derive` and appends ordinary items.
    let classified = Program {
        imports: program.imports,
        items: new_items,
    };
    derive::expand_derives(classified, sink)
}
