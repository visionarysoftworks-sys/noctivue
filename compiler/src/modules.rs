//! File-backed module loading for the command-line front end.
//!
//! A `Program` is still the unit consumed by the resolver and type checker,
//! but the loader parses every source file independently first.  This keeps
//! import discovery, cycle reporting, and file ownership separate from the
//! legacy single-file front end while allowing the existing HIR and
//! interpreter to execute a linked module graph.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{self, Expr, FunctionBody, ImportDecl, Item, Program, Stmt};
use crate::diagnostics::{Diagnostic, DiagnosticSink, Span};
use crate::{lexer, parser};

#[derive(Debug, Clone)]
pub struct ModuleFile {
    pub path: PathBuf,
    pub source: String,
    pub program: Program,
    pub base: usize,
}

#[derive(Debug, Clone)]
pub struct ModuleGraph {
    pub files: Vec<ModuleFile>,
    roots: HashSet<PathBuf>,
    diagnostics: Vec<(PathBuf, Diagnostic)>,
}

impl ModuleGraph {
    /// Load roots and all local imports.  Dependencies are stored before their
    /// importers, which also gives the linker a deterministic order.
    pub fn load(roots: &[PathBuf]) -> Result<Self, String> {
        let mut graph = ModuleGraph {
            files: Vec::new(),
            roots: roots.iter().filter_map(|p| canonical(p).ok()).collect(),
            diagnostics: Vec::new(),
        };
        let mut states = HashMap::<PathBuf, Visit>::new();
        for root in roots {
            let root = canonical(root)
                .map_err(|e| format!("error: cannot read `{}`: {e}", root.display()))?;
            graph.visit(&root, &mut states)?;
        }
        let mut base = 0;
        for file in &mut graph.files {
            file.base = base;
            base += file.source.len();
            if !file.source.ends_with('\n') {
                base += 1;
            }
        }
        Ok(graph)
    }

    pub fn joined_source(&self) -> (String, Vec<(String, usize)>) {
        let mut source = String::new();
        let mut files = Vec::new();
        for file in &self.files {
            files.push((file.path.to_string_lossy().into_owned(), source.len()));
            source.push_str(&file.source);
            if !source.ends_with('\n') {
                source.push('\n');
            }
        }
        (source, files)
    }

    /// Transfer diagnostics found while parsing individual files into the
    /// shared sink, rebasing their spans to the joined source.
    pub fn emit_diagnostics(&self, sink: &mut DiagnosticSink) {
        for (path, diag) in &self.diagnostics {
            let Some(file) = self.files.iter().find(|f| &f.path == path) else {
                sink.emit(diag.clone());
                continue;
            };
            let mut diag = diag.clone();
            for label in &mut diag.labels {
                label.span.start += file.base;
                label.span.end += file.base;
            }
            sink.emit(diag);
        }
    }

    /// Link the parsed joined program.  Root files retain private items;
    /// imported files contribute only `export` items.
    pub fn link(&self, mut program: Program) -> Program {
        let mut items = Vec::new();
        for item in program.items {
            let owner = self.owner(item_span(&item).start);
            let is_root = owner
                .and_then(|i| self.files.get(i))
                .map(|f| self.roots.contains(&f.path))
                .unwrap_or(true);
            if is_root {
                items.push(unwrap_export(item));
            } else if let Item::Export(inner) = item {
                items.push(unwrap_export(*inner));
            }
        }
        program.items = items;
        self.add_item_aliases(&mut program);
        rewrite_module_aliases(&mut program, self.root_import_aliases());
        program
    }

    fn add_item_aliases(&self, program: &mut Program) {
        let Some(root) = self.files.iter().rev().find(|f| self.roots.contains(&f.path)) else {
            return;
        };
        let mut additions = Vec::new();
        for import in &root.program.imports {
            let Some(alias) = &import.alias else { continue };
            let Some((module, item_name)) = self.resolve_import(root, import) else { continue };
            let Some(item_name) = item_name else { continue };
            let Some(source_file) = self.files.iter().find(|f| f.path == module) else { continue };
            for item in &source_file.program.items {
                if exported_name(item).as_deref() == Some(item_name.as_str()) {
                    if let Some(item) = rename_item(unwrap_export(item.clone()), alias) {
                        additions.push(item);
                    }
                }
            }
        }
        program.items.extend(additions);
    }

    fn root_import_aliases(&self) -> HashMap<String, String> {
        let mut aliases = HashMap::new();
        let Some(root) = self.files.iter().rev().find(|f| self.roots.contains(&f.path)) else {
            return aliases;
        };
        for import in &root.program.imports {
            if let Some((_, item)) = self.resolve_import(root, import) {
                if item.is_none() {
                    let alias = import
                        .alias
                        .clone()
                        .or_else(|| import.path.last().cloned())
                        .unwrap_or_default();
                    aliases.insert(alias, import.path.last().cloned().unwrap_or_default());
                }
            }
        }
        aliases
    }

    fn resolve_import(&self, importer: &ModuleFile, import: &ImportDecl) -> Option<(PathBuf, Option<String>)> {
        let path = import.path.join("::");
        let exact = resolve_path(&importer.path, &path).ok()?;
        if self.files.iter().any(|f| f.path == exact) {
            return Some((exact, None));
        }
        let item = import.path.last()?.clone();
        let module_path = import.path[..import.path.len().saturating_sub(1)].join("::");
        let module = resolve_path(&importer.path, &module_path).ok()?;
        if self.files.iter().any(|f| f.path == module) {
            Some((module, Some(item)))
        } else {
            None
        }
    }

    fn visit(&mut self, path: &Path, states: &mut HashMap<PathBuf, Visit>) -> Result<(), String> {
        match states.get(path) {
            Some(Visit::Done) => return Ok(()),
            Some(Visit::Active) => return Ok(()),
            None => {}
        }
        states.insert(path.to_path_buf(), Visit::Active);
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        let mut local_sink = DiagnosticSink::new();
        let tokens = lexer::lex(&source, &mut local_sink);
        let program = parser::parse(&tokens, &mut local_sink);
        for diag in local_sink.take() {
            self.diagnostics.push((path.to_path_buf(), diag));
        }
        for import in &program.imports {
            let Some((target, _)) = self.resolve_import_path(path, import) else {
                self.diagnostics.push((
                    path.to_path_buf(),
                    Diagnostic::error(format!(
                        "cannot resolve imported module `{}`",
                        import.path.join("::")
                    ))
                    .with_span(import.span.clone(), "imported here")
                    .with_code("E0101"),
                ));
                continue;
            };
            if states.get(&target) == Some(&Visit::Active) {
                self.diagnostics.push((
                    path.to_path_buf(),
                    Diagnostic::error(format!("module import cycle involving `{}`", target.display()))
                        .with_span(import.span.clone(), "cycle enters here")
                        .with_code("E0100"),
                ));
                continue;
            }
            self.visit(&target, states)?;
        }
        self.files.push(ModuleFile {
            path: path.to_path_buf(),
            source,
            program,
            base: 0,
        });
        states.insert(path.to_path_buf(), Visit::Done);
        Ok(())
    }

    fn resolve_import_path(&self, importer: &Path, import: &ImportDecl) -> Option<(PathBuf, Option<String>)> {
        let path = import.path.join("::");
        if let Ok(exact) = resolve_path(importer, &path) {
            if exact.is_file() {
                return Some((exact, None));
            }
        }
        let item = import.path.last()?.clone();
        let module = import.path[..import.path.len().saturating_sub(1)].join("::");
        let module = resolve_path(importer, &module).ok()?;
        if module.is_file() {
            Some((module, Some(item)))
        } else {
            None
        }
    }

    fn owner(&self, offset: usize) -> Option<usize> {
        self.files
            .iter()
            .enumerate()
            .rev()
            .find(|(_, f)| f.base <= offset)
            .map(|(i, _)| i)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Active,
    Done,
}

fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize()
}

fn resolve_path(importer: &Path, import: &str) -> std::io::Result<PathBuf> {
    let relative = import.replace("::", std::path::MAIN_SEPARATOR_STR);
    let mut candidates = Vec::new();
    if let Some(parent) = importer.parent() {
        candidates.push(parent.join(&relative));
    }
    let mut ancestor = importer.parent();
    while let Some(dir) = ancestor {
        candidates.push(dir.join("lib").join(&relative));
        ancestor = dir.parent();
    }
    for mut candidate in candidates {
        if candidate.extension().is_none() {
            candidate.set_extension("nv");
        }
        if candidate.is_file() {
            return candidate.canonicalize();
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "module not found"))
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::BareDecl(x) => x.span.clone(),
        Item::Function(x) => x.span.clone(),
        Item::Struct(x) => x.span.clone(),
        Item::Enum(x) => x.span.clone(),
        Item::Trait(x) => x.span.clone(),
        Item::Impl(x) => x.span.clone(),
        Item::Const(x) => x.span.clone(),
        Item::Mod(x) => x.span.clone(),
        Item::Export(x) => item_span(x),
    }
}

fn unwrap_export(item: Item) -> Item {
    match item {
        Item::Export(inner) => unwrap_export(*inner),
        other => other,
    }
}

fn exported_name(item: &Item) -> Option<String> {
    match item {
        Item::Export(inner) => exported_name(inner),
        Item::BareDecl(x) => Some(x.name.clone()),
        Item::Function(x) => Some(x.name.clone()),
        Item::Struct(x) => Some(x.name.clone()),
        Item::Enum(x) => Some(x.name.clone()),
        Item::Trait(x) => Some(x.name.clone()),
        Item::Const(x) => Some(x.name.clone()),
        _ => None,
    }
}

fn rename_item(item: Item, name: &str) -> Option<Item> {
    Some(match item {
        Item::Function(mut x) => { x.name = name.to_string(); Item::Function(x) }
        Item::Struct(mut x) => { x.name = name.to_string(); Item::Struct(x) }
        Item::Enum(mut x) => { x.name = name.to_string(); Item::Enum(x) }
        Item::Const(mut x) => { x.name = name.to_string(); Item::Const(x) }
        Item::BareDecl(mut x) => { x.name = name.to_string(); Item::BareDecl(x) }
        _ => return None,
    })
}

fn rewrite_module_aliases(program: &mut Program, aliases: HashMap<String, String>) {
    if aliases.is_empty() { return; }
    for item in &mut program.items {
        rewrite_item(item, &aliases);
    }
}

fn rewrite_item(item: &mut Item, aliases: &HashMap<String, String>) {
    match item {
        Item::Function(f) => {
            if let FunctionBody::Block(b) = &mut f.body { rewrite_block(b, aliases); }
            if let FunctionBody::Expr(e) = &mut f.body { rewrite_expr(e, aliases); }
        }
        Item::BareDecl(d) => rewrite_block(&mut d.body, aliases),
        Item::Const(c) => rewrite_expr(&mut c.value, aliases),
        Item::Export(i) => rewrite_item(i, aliases),
        _ => {}
    }
}

fn rewrite_block(block: &mut ast::Block, aliases: &HashMap<String, String>) {
    for stmt in &mut block.stmts { rewrite_stmt(stmt, aliases); }
}

fn rewrite_stmt(stmt: &mut Stmt, aliases: &HashMap<String, String>) {
    match stmt {
        Stmt::Let(x) => rewrite_expr(&mut x.value, aliases),
        Stmt::Var(x) => rewrite_expr(&mut x.value, aliases),
        Stmt::State(x) => rewrite_expr(&mut x.value, aliases),
        Stmt::Assign(x) => { rewrite_expr(&mut x.target, aliases); rewrite_expr(&mut x.value, aliases); }
        Stmt::Expr(e) => rewrite_expr(e, aliases),
        Stmt::Return(x) => if let Some(e) = &mut x.value { rewrite_expr(e, aliases); },
        Stmt::Break(x) => if let Some(e) = &mut x.value { rewrite_expr(e, aliases); },
        Stmt::If(x) => {
            rewrite_expr(&mut x.condition, aliases); rewrite_block(&mut x.then_block, aliases);
            for (e, b) in &mut x.else_if_clauses { rewrite_expr(e, aliases); rewrite_block(b, aliases); }
            if let Some(b) = &mut x.else_block { rewrite_block(b, aliases); }
        }
        Stmt::While(x) => { rewrite_expr(&mut x.condition, aliases); rewrite_block(&mut x.body, aliases); }
        Stmt::Loop(x) => rewrite_block(&mut x.body, aliases),
        Stmt::For(x) => { rewrite_expr(&mut x.iterable, aliases); rewrite_block(&mut x.body, aliases); }
        Stmt::Match(x) => {
            rewrite_expr(&mut x.scrutinee, aliases);
            for arm in &mut x.arms {
                if let Some(e) = &mut arm.guard { rewrite_expr(e, aliases); }
                match &mut arm.body {
                    ast::MatchBody::Block(b) => rewrite_block(b, aliases),
                    ast::MatchBody::Expr(e) => rewrite_expr(e, aliases),
                }
            }
        }
        Stmt::Function(f) => if let FunctionBody::Block(b) = &mut f.body { rewrite_block(b, aliases); },
        _ => {}
    }
}

fn rewrite_expr(expr: &mut Expr, aliases: &HashMap<String, String>) {
    if let Expr::Member(m) = expr {
        rewrite_expr(&mut m.object, aliases);
        if let Expr::Ident(alias, span) = m.object.as_ref() {
            if aliases.contains_key(alias) {
                let field = m.field.clone();
                *expr = Expr::Ident(field, span.clone());
                return;
            }
        }

    }
    match expr {
        Expr::Call(c) => { rewrite_expr(&mut c.callee, aliases); for a in &mut c.args { rewrite_expr(&mut a.value, aliases); } }
        Expr::Member(m) => rewrite_expr(&mut m.object, aliases),
        Expr::Index(i) => { rewrite_expr(&mut i.object, aliases); rewrite_expr(&mut i.index, aliases); }
        Expr::BinOp(x) => { rewrite_expr(&mut x.left, aliases); rewrite_expr(&mut x.right, aliases); }
        Expr::UnaryOp(x) => rewrite_expr(&mut x.operand, aliases),
        Expr::Try(x) => rewrite_expr(&mut x.expr, aliases),
        Expr::Range(x) => { rewrite_expr(&mut x.start, aliases); rewrite_expr(&mut x.end, aliases); }
        Expr::StringInterp(x) => for p in &mut x.parts { if let ast::InterpPart::Expr(e) = p { rewrite_expr(e, aliases); } },
        Expr::StructLit(x) => for f in &mut x.fields {
            match f { ast::StructField::Named(_, e) => rewrite_expr(e, aliases), ast::StructField::Spread(e) => rewrite_expr(e, aliases) }
        },
        Expr::ListLit(x) => for e in &mut x.elements { rewrite_expr(e, aliases); },
        Expr::Closure(x) => rewrite_expr(&mut x.body, aliases),
        Expr::Spread(x) => rewrite_expr(&mut x.expr, aliases),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()
            .join("tests").join("fixtures").join("modules").join(name)
    }

    #[test]
    fn links_exported_module_and_rewrites_alias() {
        let graph = ModuleGraph::load(&[fixture("main.nv")]).unwrap();
        let (source, _) = graph.joined_source();
        let mut sink = DiagnosticSink::new();
        let tokens = lexer::lex(&source, &mut sink);
        let program = parser::parse(&tokens, &mut sink);
        let program = graph.link(program);
        let program = crate::resolver::resolve(program, &mut sink);
        let _module = crate::typeck::typecheck(program, &mut sink);
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    }

    #[test]
    fn reports_import_cycle_at_import_site() {
        let graph = ModuleGraph::load(&[fixture("cycle_a.nv")]).unwrap();
        assert!(graph.diagnostics.iter().any(|(_, d)| d.code.as_deref() == Some("E0100")
            && !d.labels.is_empty()));
    }
}
