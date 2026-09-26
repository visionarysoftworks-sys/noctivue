//! File-backed module loading for the command-line front end.
//!
//! A `Program` is still the unit consumed by the resolver and type checker,
//! but the loader parses every source file independently first.  This keeps
//! import discovery, cycle reporting, and file ownership separate from the
//! legacy single-file front end while allowing the existing HIR and
//! interpreter to execute a linked module graph.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{self, Expr, FunctionBody, ImportDecl, Item, Param, Program, Stmt, TypeExpr};
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
    /// Extra package roots to search after `vendor/` (the global content
    /// store, in the toolchain's case). Carried on the graph so the
    /// resolver's search order is decided where the graph is built.
    package_roots: Vec<PathBuf>,
    diagnostics: Vec<(PathBuf, Diagnostic)>,
}

impl ModuleGraph {
    /// Load roots and all local imports.  Dependencies are stored before their
    /// importers, which also gives the linker a deterministic order.
    pub fn load(roots: &[PathBuf]) -> Result<Self, String> {
        Self::load_with(roots, &[])
    }

    /// [`ModuleGraph::load`] with extra package roots, searched AFTER the
    /// committed `vendor/` tree (see [`resolve_path`] for the order and
    /// why).  The toolchain passes the global content store here, whose
    /// `points/<name>-<version>.point` files name the extracted trees; a
    /// plain directory of `<name>-<version>/` trees works too, so a
    /// caller does not have to know which kind of root it is handing over.
    pub fn load_with(roots: &[PathBuf], package_roots: &[PathBuf]) -> Result<Self, String> {
        let mut graph = ModuleGraph {
            files: Vec::new(),
            roots: roots.iter().filter_map(|p| canonical(p).ok()).collect(),
            diagnostics: Vec::new(),
            package_roots: package_roots.to_vec(),
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
        graph.check_flat_uses_of_private_items();
        Ok(graph)
    }

    /// Names exported by one loaded file (for lint's unused-import
    /// check and the private-use scan below).
    pub fn exported_names(&self, path: &Path) -> Vec<String> {        self.files
            .iter()
            .find(|f| f.path == path)
            .map(|f| {
                f.program
                    .items
                    .iter()
                    .filter_map(|item| exported_name(item))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Per-file diagnostics recorded while loading (parse errors and
    /// E0100–E0109/W0101 import errors for `path`), in that file's own
    /// coordinates — no joined-source rebasing. Used by the LSP, which
    /// reports per open file rather than per linked unit.
    pub fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, d)| d.clone())
            .collect()
    }

    /// Resolve one import declaration against the loaded graph,
    /// mirroring [`ModuleGraph::load`]'s on-disk rules. Returns the
    /// target file plus the trailing item name when the path names
    /// `module::item` rather than a module. Used by lint so a
    /// flattened import counts as used when its items are referenced.
    pub fn resolve_import_decl(
        &self,
        importer: &ModuleFile,
        import: &ImportDecl,
    ) -> Option<(PathBuf, Option<String>)> {
        self.resolve_import(importer, import)
    }

    /// Hint pass (warning W0101): a use of an item that a
    /// module-only import target defines *privately* — in flat form
    /// (`name(...)`) or qualified (`alias.name(...)`). Without this,
    /// the import parses cleanly and the use fails later with a bare
    /// `unknown identifier` (flat) or one that blames the alias
    /// (qualified), hiding the one-word fix (`export`). Warning-level
    /// by design: it only fires for names no file in the graph
    /// exports and no visible binding defines, so it can never break a
    /// passing program.
    fn check_flat_uses_of_private_items(&mut self) {
        let graph_exports: HashSet<String> = self
            .files
            .iter()
            .flat_map(|f| f.program.items.iter().filter_map(|i| exported_name(i)))
            .collect();
        let mut hints: Vec<(PathBuf, Diagnostic)> = Vec::new();
        for importer in &self.files {
            let defined: HashSet<String> = importer
                .program
                .items
                .iter()
                .filter_map(|i| item_name_of(i))
                .collect();
            let bound = bound_names(&importer.program);
            let mut used: HashMap<String, Span> = HashMap::new();
            collect_used_names(&importer.program, &mut used);
            let qualified = collect_qualified_uses(&importer.program);
            for import in &importer.program.imports {
                let Some((target, item)) = self.resolve_import(importer, import) else {
                    continue;
                };
                if item.is_some() {
                    continue;
                }
                let Some(dep) = self.files.iter().find(|f| f.path == target) else {
                    continue;
                };
                let private: HashSet<String> = dep
                    .program
                    .items
                    .iter()
                    .filter_map(|i| item_name_of(i))
                    .filter(|n| !dep.program.items.iter().any(|e| exported_name(e).as_deref() == Some(n)))
                    .collect();
                for (name, span) in &used {
                    if defined.contains(name)
                        || bound.contains(name)
                        || graph_exports.contains(name)
                    {
                        continue;
                    }
                    if private.contains(name) {
                        hints.push((
                            importer.path.clone(),
                            Diagnostic::warning(format!(
                                "`{name}` is private in imported module `{}`; add `export` to use it here",
                                import.path.join("::")
                            ))
                            .with_span(span.clone(), "used here")
                            .with_code("W0101"),
                        ));
                    }
                }
                // Qualified form of the same trap: `alias.private_fn()`
                // is not rewritten (only exports are), so without a hint
                // the type checker blames the ALIAS (`unknown identifier
                // alias`) instead of naming the private item. Record the
                // member, not just the object.
                let key = import
                    .alias
                    .clone()
                    .or_else(|| import.path.last().cloned())
                    .unwrap_or_default();
                if !key.is_empty() {
                    for (qualifier, field, span) in &qualified {
                        if qualifier == &key && private.contains(field) {
                            hints.push((
                                importer.path.clone(),
                                Diagnostic::warning(format!(
                                    "`{field}` is private in imported module `{}`; add `export` to use it as `{key}.{field}`",
                                    import.path.join("::")
                                ))
                                .with_span(span.clone(), "used here")
                                .with_code("W0101"),
                            ));
                        }
                    }
                }
            }
        }
        // One hint per (file, name): repeated uses share the cause.
        let mut seen = HashSet::new();
        for (path, diag) in hints {
            if seen.insert((path.clone(), diag.message.clone())) {
                self.diagnostics.push((path, diag));
            }
        }
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
        let aliases = self.module_alias_exports();
        self.rewrite_imported_refs(&mut program, &aliases);
        program
    }

    fn add_item_aliases(&self, program: &mut Program) {
        let Some(root) = self
            .files
            .iter()
            .rev()
            .find(|f| self.roots.contains(&f.path))
        else {
            return;
        };
        let mut additions = Vec::new();
        for import in &root.program.imports {
            let Some(alias) = &import.alias else { continue };
            let Some((module, item_name)) = self.resolve_import(root, import) else {
                continue;
            };
            let Some(item_name) = item_name else { continue };
            let Some(source_file) = self.files.iter().find(|f| f.path == module) else {
                continue;
            };
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

    /// Map every name that introduces qualified access to the item
    /// names it exposes: explicit aliases (`import m as a` → `a`) AND
    /// unaliased module names (`import m` → last segment of `m`).
    /// Values are the target module's *exported* item names, which is
    /// what makes the member rewrite precise: `document.path` is left
    /// alone when `path` is a local struct field rather than an
    /// export, so locals that share a module's name keep working
    /// (the `document`/`workspace` case in examples/nightshade).
    /// Item imports (`import m::item [as x]`) need no entry: the item
    /// is used flat (or under its alias via `add_item_aliases`).
    fn module_alias_exports(&self) -> HashMap<String, HashSet<String>> {
        let mut aliases = HashMap::new();
        let Some(root) = self
            .files
            .iter()
            .rev()
            .find(|f| self.roots.contains(&f.path))
        else {
            return aliases;
        };
        for import in &root.program.imports {
            if let Some((module, item)) = self.resolve_import(root, import) {
                if item.is_some() {
                    continue;
                }
                let key = import
                    .alias
                    .clone()
                    .or_else(|| import.path.last().cloned())
                    .unwrap_or_default();
                if key.is_empty() {
                    continue;
                }
                if let Some(source) = self.files.iter().find(|f| f.path == module) {
                    let exports: HashSet<String> = source
                        .program
                        .items
                        .iter()
                        .filter_map(|item| exported_name(item))
                        .collect();
                    aliases
                        .entry(key)
                        .or_insert_with(HashSet::new)
                        .extend(exports);
                }
            }
        }
        aliases
    }

    /// Rewrite qualified references to linked imports inside root-owned
    /// items only. Dependency files are already linked by flattening,
    /// so touching them here could only corrupt same-named locals.
    fn rewrite_imported_refs(
        &self,
        program: &mut Program,
        aliases: &HashMap<String, HashSet<String>>,
    ) {
        if aliases.is_empty() {
            return;
        }
        for item in &mut program.items {
            if self.owned_by_root(item_span(item).start) {
                rewrite_item(item, aliases);
            }
        }
    }

    fn owned_by_root(&self, offset: usize) -> bool {
        self.owner(offset)
            .and_then(|i| self.files.get(i))
            .map(|f| self.roots.contains(&f.path))
            .unwrap_or(true)
    }

    fn resolve_import(
        &self,
        importer: &ModuleFile,
        import: &ImportDecl,
    ) -> Option<(PathBuf, Option<String>)> {
        let path = import.path.join("::");
        if let Ok(exact) = resolve_path(&importer.path, &path, &self.package_roots) {
            if self.files.iter().any(|f| f.path == exact) {
                return Some((exact, None));
            }
        }
        let item = import.path.last()?.clone();
        let module_path = import.path[..import.path.len().saturating_sub(1)].join("::");
        let module = resolve_path(&importer.path, &module_path, &self.package_roots).ok()?;
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
            let Some((target, item)) = self.resolve_import_path(path, import) else {
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
                    Diagnostic::error(format!(
                        "module import cycle involving `{}`",
                        target.display()
                    ))
                    .with_span(import.span.clone(), "cycle enters here")
                    .with_code("E0100"),
                ));
                continue;
            }
            self.visit(&target, states)?;
            if let Some(item_name) = item {
                if let Some(target_file) = self.files.iter().find(|file| file.path == target) {
                    let found = target_file.program.items.iter().find(|candidate| {
                        exported_name(candidate).as_deref() == Some(item_name.as_str())
                    });
                    if found.is_none() {
                        let exists_private = target_file.program.items.iter().any(|candidate| {
                            item_name_of(candidate).as_deref() == Some(item_name.as_str())
                        });
                        let (message, code) = if exists_private {
                            (
                                format!("item `{item_name}` is private in imported module"),
                                "E0102",
                            )
                        } else {
                            (
                                format!("item `{item_name}` is not defined in imported module"),
                                "E0103",
                            )
                        };
                        self.diagnostics.push((
                            path.to_path_buf(),
                            Diagnostic::error(message)
                                .with_span(import.span.clone(), "imported here")
                                .with_code(code),
                        ));
                    } else if import.alias.is_some() {
                        // `import m::Trait as T` (and impl/mod/derive):
                        // the linker can only duplicate plain items
                        // under a new name, so say so instead of
                        // silently adding nothing.
                        if let Some(exported) = found.filter(|f| !aliasable_item(f)) {
                            self.diagnostics.push((
                                path.to_path_buf(),
                                Diagnostic::error(format!(
                                    "item `{item_name}` cannot be imported under an alias: {}s are used through their module",
                                    item_kind_name(exported)
                                ))
                                .with_span(import.span.clone(), "imported here")
                                .with_code("E0109"),
                            ));
                        }
                    }
                }
            }
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

    fn resolve_import_path(
        &self,
        importer: &Path,
        import: &ImportDecl,
    ) -> Option<(PathBuf, Option<String>)> {
        let path = import.path.join("::");
        if let Ok(exact) = resolve_path(importer, &path, &self.package_roots) {
            if exact.is_file() {
                return Some((exact, None));
            }
        }
        let item = import.path.last()?.clone();
        let module = import.path[..import.path.len().saturating_sub(1)].join("::");
        let module = resolve_path(importer, &module, &self.package_roots).ok()?;
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

fn resolve_path(
    importer: &Path,
    import: &str,
    package_roots: &[PathBuf],
) -> std::io::Result<PathBuf> {
    let parts: Vec<&str> = import.split("::").filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "empty module path",
        ));
    }
    let relative = parts.join(std::path::MAIN_SEPARATOR_STR);
    let mut candidates = Vec::new();
    if let Some(parent) = importer.parent() {
        candidates.push(parent.join(&relative));
    }
    let mut ancestor = importer.parent();
    while let Some(dir) = ancestor {
        candidates.push(dir.join("lib").join(&relative));
        // A workspace package can be imported by package name. Prefer a
        // checked-out sibling package, then a COMMITTED `vendor/` tree
        // (the offline escape hatch — docs/TOOLCHAIN.md §3), then the
        // global content store, while retaining the current package's
        // `lib` convention. Vendor precedes the store on purpose: a
        // vendored tree is reviewed and committed, an extracted one is
        // whatever the last fetch left behind.
        //
        // The legacy in-tree `.noct/packages/` is NOT searched: fetched
        // content is global now, and an in-tree tree nobody verified is
        // exactly what must stop compiling. In-tree `.noct/` holds only
        // build outputs.
        if parts.len() > 1 {
            let package = parts[0];
            let module = parts[1..].join(std::path::MAIN_SEPARATOR_STR);
            candidates.push(dir.join(package).join("lib").join(&module));
            candidates.push(dir.join("packages").join(package).join("lib").join(&module));
            candidates.extend(package_tree_candidates(&dir.join("vendor"), package, &module));
            for root in package_roots {
                candidates.extend(package_tree_candidates(root, package, &module));
                candidates.extend(pointer_tree_candidates(root, package, &module));
            }
        }
        ancestor = dir.parent();
    }
    for mut candidate in candidates.clone() {
        if candidate.extension().is_none() {
            candidate.set_extension("nv");
        }
        if candidate.is_file() {
            return candidate.canonicalize();
        }
    }
    // Phase 2 — directory modules: `import a::b` also resolves to
    // `a/b/<last>.nv` or `a/b/mod.nv` when no `a/b.nv` file exists.
    // Exact files always win (phase 1 above runs first); the
    // repeat-last form mirrors the `lib/<name>/...` package layout
    // so multi-file libraries do not need a flat file per module.
    if let Some(last) = parts.last() {
        for base in candidates {
            for leaf in [last.to_string(), "mod".to_string()] {
                let mut candidate = base.join(&leaf);
                candidate.set_extension("nv");
                if candidate.is_file() {
                    return candidate.canonicalize();
                }
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "module not found",
    ))
}

/// Package trees in a `vendor/`-shaped root: a directory named after
/// the package, or after `<package>-<version>`, holding `lib/<module>`.
fn package_tree_candidates(root: &Path, package: &str, module: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let prefix = format!("{package}-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == package || name.starts_with(&prefix) {
            out.push(entry.path().join("lib").join(module));
        }
    }
    out
}

/// Package trees named by a content store's pointers.
///
/// The global store is content-addressed (`trees/<sha256>/`), so the
/// resolver cannot guess a tree from a package name — the store hands
/// over a `points/<name>-<version>.point` file whose first line is the
/// tree path relative to the store root. Keeping the convention to one
/// line of text is what lets a hash-keyed store and a name-keyed
/// resolver coexist without either one learning the other's layout.
///
/// A pointer is data from disk, so it is jailed exactly like a tarball
/// path: absolute paths and `..` are refused rather than resolved.
fn pointer_tree_candidates(root: &Path, package: &str, module: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let points = root.join("points");
    let Ok(entries) = std::fs::read_dir(&points) else {
        return out;
    };
    let prefix = format!("{package}-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(&prefix) || !name.ends_with(".point") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(relative) = text.lines().next().map(str::trim).filter(|l| !l.is_empty()) else {
            continue;
        };
        if relative.starts_with('/')
            || relative.contains(':')
            || relative.split(['/', '\\']).any(|part| part == "..")
        {
            continue;
        }
        out.push(root.join(relative).join("lib").join(module));
    }
    out
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::BareDecl(x) => x.span.clone(),
        Item::Function(x) => x.span.clone(),
        Item::Task(x) => x.span.clone(),
        Item::Derive(x) => x.span.clone(),
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
        Item::Export(inner) => item_name_of(inner),
        _ => None,
    }
}

fn item_name_of(item: &Item) -> Option<String> {
    match item {
        Item::Export(inner) => item_name_of(inner),
        Item::BareDecl(x) => Some(x.name.clone()),
        Item::Function(x) => Some(x.name.clone()),
        Item::Task(x) => Some(x.name.clone()),
        Item::Derive(x) => Some(x.target.clone()),
        Item::Struct(x) => Some(x.name.clone()),
        Item::Enum(x) => Some(x.name.clone()),
        Item::Trait(x) => Some(x.name.clone()),
        Item::Const(x) => Some(x.name.clone()),
        Item::Mod(x) => Some(x.name.clone()),
        Item::Impl(_) => None,
    }
}

fn rename_item(item: Item, name: &str) -> Option<Item> {
    Some(match item {
        Item::Function(mut x) => {
            x.name = name.to_string();
            Item::Function(x)
        }
        Item::Task(mut x) => {
            x.name = name.to_string();
            Item::Task(x)
        }
        Item::Struct(mut x) => {
            x.name = name.to_string();
            Item::Struct(x)
        }
        Item::Enum(mut x) => {
            x.name = name.to_string();
            Item::Enum(x)
        }
        Item::Const(mut x) => {
            x.name = name.to_string();
            Item::Const(x)
        }
        Item::BareDecl(mut x) => {
            x.name = name.to_string();
            Item::BareDecl(x)
        }
        _ => return None,
    })
}

/// Whether an item can be duplicated under an import alias by
/// [`rename_item`]. Trait/impl/module/derive items carry identity
/// beyond their name, so aliasing them is rejected (E0109) rather
/// than silently dropped.
fn aliasable_item(item: &Item) -> bool {
    matches!(
        unwrap_export_ref(item),
        Item::Function(_)
            | Item::Task(_)
            | Item::Struct(_)
            | Item::Enum(_)
            | Item::Const(_)
            | Item::BareDecl(_)
    )
}

fn unwrap_export_ref<'a>(item: &'a Item) -> &'a Item {
    match item {
        Item::Export(inner) => unwrap_export_ref(inner),
        other => other,
    }
}

fn item_kind_name(item: &Item) -> &'static str {
    match unwrap_export_ref(item) {
        Item::BareDecl(_) => "declaration",
        Item::Function(_) => "function",
        Item::Task(_) => "task",
        Item::Derive(_) => "derive",
        Item::Struct(_) => "struct",
        Item::Enum(_) => "enum",
        Item::Trait(_) => "trait",
        Item::Impl(_) => "impl block",
        Item::Const(_) => "const",
        Item::Mod(_) => "module",
        Item::Export(_) => "item",
    }
}

/// Last segment of a possibly qualified name (`a::B` → `B`).
fn last_segment(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// Value-identifier uses in a program, mapped to their first use
/// span. Covers plain idents, member roots (`m.f` → `m`), struct
/// literal names, and named types. Used by the private-use hint and
/// (via names only) lint's unused-import check.
fn collect_used_names(program: &Program, used: &mut HashMap<String, Span>) {
    for item in &program.items {
        collect_used_item(item, used);
    }
}

/// `alias.member` / `module.member` pairs in a program, with the span
/// of the whole access. Unlike [`collect_used_names`] this keeps the
/// MEMBER name (not just the object), which is what the qualified
/// private-use hint needs: `p.private_fn()` must be reported as a
/// private `private_fn`, not as an unknown `p`.
fn collect_qualified_uses(program: &Program) -> Vec<(String, String, Span)> {
    let mut out = Vec::new();
    for item in &program.items {
        collect_qualified_item(item, &mut out);
    }
    out
}

fn collect_qualified_item(item: &Item, out: &mut Vec<(String, String, Span)>) {
    match item {
        Item::Function(f) => collect_qualified_body(&f.body, out),
        Item::BareDecl(d) => collect_qualified_block(&d.body, out),
        Item::Task(t) => collect_qualified_block(&t.body, out),
        Item::Const(c) => collect_qualified_expr(&c.value, out),
        Item::Mod(m) => {
            for s in &m.items {
                collect_qualified_stmt(s, out);
            }
        }
        Item::Export(inner) => collect_qualified_item(inner, out),
        _ => {}
    }
}

fn collect_qualified_body(body: &FunctionBody, out: &mut Vec<(String, String, Span)>) {
    match body {
        FunctionBody::Block(b) => collect_qualified_block(b, out),
        FunctionBody::Expr(e) => collect_qualified_expr(e, out),
    }
}

fn collect_qualified_block(block: &ast::Block, out: &mut Vec<(String, String, Span)>) {
    for stmt in &block.stmts {
        collect_qualified_stmt(stmt, out);
    }
}

fn collect_qualified_stmt(stmt: &Stmt, out: &mut Vec<(String, String, Span)>) {
    match stmt {
        Stmt::Let(x) => collect_qualified_expr(&x.value, out),
        Stmt::Var(x) => collect_qualified_expr(&x.value, out),
        Stmt::State(x) => collect_qualified_expr(&x.value, out),
        Stmt::Decl(x) => collect_qualified_expr(&x.value, out),
        Stmt::Assign(x) => {
            collect_qualified_expr(&x.target, out);
            collect_qualified_expr(&x.value, out);
        }
        Stmt::Expr(e) => collect_qualified_expr(e, out),
        Stmt::Return(x) => {
            if let Some(e) = &x.value {
                collect_qualified_expr(e, out);
            }
        }
        Stmt::Break(x) => {
            if let Some(e) = &x.value {
                collect_qualified_expr(e, out);
            }
        }
        Stmt::If(x) => {
            collect_qualified_expr(&x.condition, out);
            collect_qualified_block(&x.then_block, out);
            for (e, b) in &x.else_if_clauses {
                collect_qualified_expr(e, out);
                collect_qualified_block(b, out);
            }
            if let Some(b) = &x.else_block {
                collect_qualified_block(b, out);
            }
        }
        Stmt::While(x) => {
            collect_qualified_expr(&x.condition, out);
            collect_qualified_block(&x.body, out);
        }
        Stmt::Loop(x) => collect_qualified_block(&x.body, out),
        Stmt::For(x) => {
            collect_qualified_expr(&x.iterable, out);
            collect_qualified_block(&x.body, out);
        }
        Stmt::Match(x) => {
            collect_qualified_expr(&x.scrutinee, out);
            for arm in &x.arms {
                if let Some(e) = &arm.guard {
                    collect_qualified_expr(e, out);
                }
                match &arm.body {
                    ast::MatchBody::Block(b) => collect_qualified_block(b, out),
                    ast::MatchBody::Expr(e) => collect_qualified_expr(e, out),
                }
            }
        }
        Stmt::Function(f) => collect_qualified_body(&f.body, out),
        Stmt::Task(t) => collect_qualified_block(&t.body, out),
        _ => {}
    }
}

fn collect_qualified_expr(expr: &Expr, out: &mut Vec<(String, String, Span)>) {
    if let Expr::Member(m) = expr {
        if let Expr::Ident(name, _) = m.object.as_ref() {
            out.push((name.clone(), m.field.clone(), m.span.clone()));
        }
    }
    match expr {
        Expr::Call(c) => {
            collect_qualified_expr(&c.callee, out);
            for a in &c.args {
                collect_qualified_expr(&a.value, out);
            }
            if let Some(b) = &c.trailing_block {
                collect_qualified_block(b, out);
            }
        }
        Expr::Member(m) => collect_qualified_expr(&m.object, out),
        Expr::Index(i) => {
            collect_qualified_expr(&i.object, out);
            collect_qualified_expr(&i.index, out);
        }
        Expr::BinOp(x) => {
            collect_qualified_expr(&x.left, out);
            collect_qualified_expr(&x.right, out);
        }
        Expr::UnaryOp(x) => collect_qualified_expr(&x.operand, out),
        Expr::Try(x) => collect_qualified_expr(&x.expr, out),
        Expr::Range(x) => {
            collect_qualified_expr(&x.start, out);
            collect_qualified_expr(&x.end, out);
        }
        Expr::StringInterp(x) => {
            for p in &x.parts {
                if let ast::InterpPart::Expr(e) = p {
                    collect_qualified_expr(e, out);
                }
            }
        }
        Expr::StructLit(x) => {
            for f in &x.fields {
                match f {
                    ast::StructField::Named(_, e) => collect_qualified_expr(e, out),
                    ast::StructField::Spread(e) => collect_qualified_expr(e, out),
                }
            }
        }
        Expr::ListLit(x) => {
            for e in &x.elements {
                collect_qualified_expr(e, out);
            }
        }
        Expr::Closure(x) => collect_qualified_expr(&x.body, out),
        Expr::Spread(x) => collect_qualified_expr(&x.expr, out),
        _ => {}
    }
}

fn collect_used_item(item: &Item, used: &mut HashMap<String, Span>) {
    match item {
        Item::Function(f) => {
            for p in &f.params {
                collect_used_type(&p.ty, used);
                if let Some(v) = &p.default {
                    collect_used_expr(v, used);
                }
            }
            if let Some(t) = &f.return_ty {
                collect_used_type(t, used);
            }
            match &f.body {
                FunctionBody::Block(b) => collect_used_block(b, used),
                FunctionBody::Expr(e) => collect_used_expr(e, used),
            }
        }
        Item::BareDecl(d) => {
            if let Some(params) = &d.params {
                for p in params {
                    collect_used_type(&p.ty, used);
                    if let Some(v) = &p.default {
                        collect_used_expr(v, used);
                    }
                }
            }
            if let Some(t) = &d.return_ty {
                collect_used_type(t, used);
            }
            collect_used_block(&d.body, used);
        }
        Item::Task(t) => {
            for p in &t.params {
                collect_used_type(&p.ty, used);
                if let Some(v) = &p.default {
                    collect_used_expr(v, used);
                }
            }
            collect_used_block(&t.body, used);
        }
        Item::Const(c) => {
            collect_used_type(&c.ty, used);
            collect_used_expr(&c.value, used);
        }
        Item::Struct(s) => {
            for f in &s.fields {
                collect_used_type(&f.ty, used);
            }
        }
        Item::Enum(e) => {
            for v in &e.variants {
                for t in &v.fields {
                    collect_used_type(t, used);
                }
            }
        }
        Item::Trait(t) => {
            for m in &t.members {
                for p in &m.params {
                    collect_used_type(&p.ty, used);
                }
                if let Some(r) = &m.return_ty {
                    collect_used_type(r, used);
                }
            }
        }
        Item::Impl(i) => {
            collect_used_type(&i.ty, used);
            if let Some(t) = &i.for_trait {
                collect_used_type(t, used);
            }
            for m in &i.methods {
                collect_used_item(&Item::Function(m.clone()), used);
            }
        }
        Item::Mod(m) => {
            for s in &m.items {
                collect_used_stmt(s, used);
            }
        }
        Item::Export(inner) => collect_used_item(inner, used),
        Item::Derive(_) => {}
    }
}

fn collect_used_block(block: &ast::Block, used: &mut HashMap<String, Span>) {
    for stmt in &block.stmts {
        collect_used_stmt(stmt, used);
    }
}

fn collect_used_stmt(stmt: &Stmt, used: &mut HashMap<String, Span>) {
    match stmt {
        Stmt::Let(x) => {
            if let Some(t) = &x.ty {
                collect_used_type(t, used);
            }
            collect_used_expr(&x.value, used);
        }
        Stmt::Var(x) => {
            if let Some(t) = &x.ty {
                collect_used_type(t, used);
            }
            collect_used_expr(&x.value, used);
        }
        Stmt::State(x) => {
            collect_used_expr(&x.value, used);
        }
        Stmt::Decl(x) => {
            collect_used_expr(&x.value, used);
        }
        Stmt::Assign(x) => {
            collect_used_expr(&x.target, used);
            collect_used_expr(&x.value, used);
        }
        Stmt::Expr(e) => collect_used_expr(e, used),
        Stmt::Return(x) => {
            if let Some(e) = &x.value {
                collect_used_expr(e, used);
            }
        }
        Stmt::Break(x) => {
            if let Some(e) = &x.value {
                collect_used_expr(e, used);
            }
        }
        Stmt::If(x) => {
            collect_used_expr(&x.condition, used);
            collect_used_block(&x.then_block, used);
            for (e, b) in &x.else_if_clauses {
                collect_used_expr(e, used);
                collect_used_block(b, used);
            }
            if let Some(b) = &x.else_block {
                collect_used_block(b, used);
            }
        }
        Stmt::While(x) => {
            collect_used_expr(&x.condition, used);
            collect_used_block(&x.body, used);
        }
        Stmt::Loop(x) => collect_used_block(&x.body, used),
        Stmt::For(x) => {
            collect_used_expr(&x.iterable, used);
            collect_used_block(&x.body, used);
        }
        Stmt::Match(x) => {
            collect_used_expr(&x.scrutinee, used);
            for arm in &x.arms {
                if let Some(e) = &arm.guard {
                    collect_used_expr(e, used);
                }
                match &arm.body {
                    ast::MatchBody::Block(b) => collect_used_block(b, used),
                    ast::MatchBody::Expr(e) => collect_used_expr(e, used),
                }
            }
        }
        Stmt::Function(f) => collect_used_item(&Item::Function(f.clone()), used),
        Stmt::Task(t) => collect_used_item(&Item::Task(t.clone()), used),
        Stmt::Struct(s) => collect_used_item(&Item::Struct(s.clone()), used),
        Stmt::BareField(f) => collect_used_type(&f.ty, used),
        _ => {}
    }
}

fn collect_used_expr(expr: &Expr, used: &mut HashMap<String, Span>) {
    match expr {
        Expr::Ident(name, span) => {
            used.entry(last_segment(name).to_string())
                .or_insert_with(|| span.clone());
        }
        Expr::Member(m) => collect_used_expr(&m.object, used),
        Expr::Call(c) => {
            collect_used_expr(&c.callee, used);
            for a in &c.args {
                collect_used_expr(&a.value, used);
            }
            if let Some(b) = &c.trailing_block {
                collect_used_block(b, used);
            }
        }
        Expr::Index(i) => {
            collect_used_expr(&i.object, used);
            collect_used_expr(&i.index, used);
        }
        Expr::BinOp(x) => {
            collect_used_expr(&x.left, used);
            collect_used_expr(&x.right, used);
        }
        Expr::UnaryOp(x) => collect_used_expr(&x.operand, used),
        Expr::Try(x) => collect_used_expr(&x.expr, used),
        Expr::Range(x) => {
            collect_used_expr(&x.start, used);
            collect_used_expr(&x.end, used);
        }
        Expr::StringInterp(x) => {
            for p in &x.parts {
                if let ast::InterpPart::Expr(e) = p {
                    collect_used_expr(e, used);
                }
            }
        }
        Expr::StructLit(x) => {
            used.entry(last_segment(&x.name).to_string())
                .or_insert_with(|| x.span.clone());
            for f in &x.fields {
                match f {
                    ast::StructField::Named(_, e) => collect_used_expr(e, used),
                    ast::StructField::Spread(e) => collect_used_expr(e, used),
                }
            }
        }
        Expr::ListLit(x) => {
            for e in &x.elements {
                collect_used_expr(e, used);
            }
        }
        Expr::Closure(x) => collect_used_expr(&x.body, used),
        Expr::Spread(x) => collect_used_expr(&x.expr, used),
        _ => {}
    }
}

fn collect_used_type(ty: &TypeExpr, used: &mut HashMap<String, Span>) {
    match ty {
        TypeExpr::Named(name, args, span) => {
            used.entry(last_segment(name).to_string())
                .or_insert_with(|| span.clone());
            for a in args {
                collect_used_type(a, used);
            }
        }
        TypeExpr::Tuple(elems, _) => {
            for e in elems {
                collect_used_type(e, used);
            }
        }
        TypeExpr::Collection(inner, _) => collect_used_type(inner, used),
        TypeExpr::Function(params, ret, _) => {
            for p in params {
                collect_used_type(p, used);
            }
            collect_used_type(ret, used);
        }
    }
}

/// Names bound inside a program (locals, params, nested items,
/// match bindings): uses of these names never refer to imports.
/// Over-approximate by design — missing a hint is cheaper than a
/// false one.
fn bound_names(program: &Program) -> HashSet<String> {
    let mut bound = HashSet::new();
    for item in &program.items {
        collect_bound_item(item, &mut bound);
    }
    bound
}

fn collect_bound_item(item: &Item, bound: &mut HashSet<String>) {
    match item {
        Item::Function(f) => {
            bound.insert(f.name.clone());
            for p in &f.params {
                bound.insert(p.name.clone());
            }
            match &f.body {
                FunctionBody::Block(b) => collect_bound_block(b, bound),
                FunctionBody::Expr(_) => {}
            }
        }
        Item::BareDecl(d) => {
            bound.insert(d.name.clone());
            if let Some(params) = &d.params {
                for p in params {
                    bound.insert(p.name.clone());
                }
            }
            collect_bound_block(&d.body, bound);
        }
        Item::Task(t) => {
            bound.insert(t.name.clone());
            for p in &t.params {
                bound.insert(p.name.clone());
            }
            collect_bound_block(&t.body, bound);
        }
        Item::Const(c) => {
            bound.insert(c.name.clone());
        }
        Item::Mod(m) => {
            bound.insert(m.name.clone());
            for s in &m.items {
                collect_bound_stmt(s, bound);
            }
        }
        Item::Export(inner) => collect_bound_item(inner, bound),
        _ => {}
    }
}

fn collect_bound_block(block: &ast::Block, bound: &mut HashSet<String>) {
    for stmt in &block.stmts {
        collect_bound_stmt(stmt, bound);
    }
}

fn collect_bound_stmt(stmt: &Stmt, bound: &mut HashSet<String>) {
    match stmt {
        Stmt::Let(x) => {
            bound.insert(x.name.clone());
        }
        Stmt::Var(x) => {
            bound.insert(x.name.clone());
        }
        Stmt::State(x) => {
            bound.insert(x.name.clone());
        }
        Stmt::Decl(x) => {
            bound.insert(x.name.clone());
        }
        Stmt::If(x) => {
            collect_bound_block(&x.then_block, bound);
            for (_, b) in &x.else_if_clauses {
                collect_bound_block(b, bound);
            }
            if let Some(b) = &x.else_block {
                collect_bound_block(b, bound);
            }
        }
        Stmt::While(x) => collect_bound_block(&x.body, bound),
        Stmt::Loop(x) => collect_bound_block(&x.body, bound),
        Stmt::For(x) => {
            bound.insert(x.binding.clone());
            collect_bound_block(&x.body, bound);
        }
        Stmt::Match(x) => {
            for arm in &x.arms {
                collect_bound_pattern(&arm.pattern, bound);
                match &arm.body {
                    ast::MatchBody::Block(b) => collect_bound_block(b, bound),
                    ast::MatchBody::Expr(_) => {}
                }
            }
        }
        Stmt::Function(f) => collect_bound_item(&Item::Function(f.clone()), bound),
        Stmt::Task(t) => collect_bound_item(&Item::Task(t.clone()), bound),
        Stmt::Struct(s) => {
            bound.insert(s.name.clone());
        }
        _ => {}
    }
}

fn collect_bound_pattern(pattern: &ast::Pattern, bound: &mut HashSet<String>) {
    match pattern {
        ast::Pattern::Ident(name, _) => {
            bound.insert(name.clone());
        }
        ast::Pattern::Variant(_, args, _) => {
            for a in args {
                collect_bound_pattern(a, bound);
            }
        }
        _ => {}
    }
}

/// Strip a leading import-alias qualifier (`alias::Rest` → `Rest`)
/// when `alias` names a linked module. Plain names pass through, so
/// this is a no-op for programs without qualified references.
fn strip_alias_prefix(name: &mut String, aliases: &HashMap<String, HashSet<String>>) {
    if let Some((head, _)) = name.split_once("::") {
        if aliases.contains_key(head) {
            *name = name[head.len() + 2..].to_string();
        }
    }
}

fn rewrite_item(item: &mut Item, aliases: &HashMap<String, HashSet<String>>) {
    match item {
        Item::Function(f) => rewrite_function_decl(f, aliases),
        Item::BareDecl(d) => {
            for p in &mut d.params.iter_mut().flatten() {
                rewrite_param(p, aliases);
            }
            if let Some(t) = &mut d.return_ty {
                rewrite_type(t, aliases);
            }
            rewrite_block(&mut d.body, aliases);
        }
        Item::Task(t) => {
            for p in &mut t.params {
                rewrite_param(p, aliases);
            }
            rewrite_block(&mut t.body, aliases);
        }
        Item::Const(c) => {
            rewrite_type(&mut c.ty, aliases);
            rewrite_expr(&mut c.value, aliases);
        }
        Item::Struct(s) => {
            for f in &mut s.fields {
                rewrite_type(&mut f.ty, aliases);
            }
        }
        Item::Enum(e) => {
            for v in &mut e.variants {
                for t in &mut v.fields {
                    rewrite_type(t, aliases);
                }
            }
        }
        Item::Trait(t) => {
            for m in &mut t.members {
                rewrite_sig(m, aliases);
            }
        }
        Item::Impl(i) => {
            rewrite_type(&mut i.ty, aliases);
            if let Some(t) = &mut i.for_trait {
                rewrite_type(t, aliases);
            }
            for m in &mut i.methods {
                rewrite_function_decl(m, aliases);
            }
        }
        Item::Mod(m) => {
            for s in &mut m.items {
                rewrite_stmt(s, aliases);
            }
        }
        Item::Export(i) => rewrite_item(i, aliases),
        Item::Derive(_) => {}
    }
}

fn rewrite_function_decl(
    f: &mut ast::FunctionDecl,
    aliases: &HashMap<String, HashSet<String>>,
) {
    for p in &mut f.params {
        rewrite_param(p, aliases);
    }
    if let Some(t) = &mut f.return_ty {
        rewrite_type(t, aliases);
    }
    match &mut f.body {
        FunctionBody::Block(b) => rewrite_block(b, aliases),
        FunctionBody::Expr(e) => rewrite_expr(e, aliases),
    }
}

fn rewrite_sig(sig: &mut ast::FunctionSig, aliases: &HashMap<String, HashSet<String>>) {
    for p in &mut sig.params {
        rewrite_param(p, aliases);
    }
    if let Some(t) = &mut sig.return_ty {
        rewrite_type(t, aliases);
    }
}

fn rewrite_param(p: &mut Param, aliases: &HashMap<String, HashSet<String>>) {
    rewrite_type(&mut p.ty, aliases);
    if let Some(v) = &mut p.default {
        rewrite_expr(v, aliases);
    }
}

fn rewrite_type(ty: &mut TypeExpr, aliases: &HashMap<String, HashSet<String>>) {
    match ty {
        TypeExpr::Named(name, args, _) => {
            strip_alias_prefix(name, aliases);
            for a in args {
                rewrite_type(a, aliases);
            }
        }
        TypeExpr::Tuple(elems, _) => {
            for e in elems {
                rewrite_type(e, aliases);
            }
        }
        TypeExpr::Collection(inner, _) => rewrite_type(inner, aliases),
        TypeExpr::Function(params, ret, _) => {
            for p in params {
                rewrite_type(p, aliases);
            }
            rewrite_type(ret, aliases);
        }
    }
}

fn rewrite_block(block: &mut ast::Block, aliases: &HashMap<String, HashSet<String>>) {
    for stmt in &mut block.stmts {
        rewrite_stmt(stmt, aliases);
    }
}

fn rewrite_stmt(stmt: &mut Stmt, aliases: &HashMap<String, HashSet<String>>) {
    match stmt {
        Stmt::Let(x) => {
            if let Some(t) = &mut x.ty {
                rewrite_type(t, aliases);
            }
            rewrite_expr(&mut x.value, aliases);
        }
        Stmt::Var(x) => {
            if let Some(t) = &mut x.ty {
                rewrite_type(t, aliases);
            }
            rewrite_expr(&mut x.value, aliases);
        }
        Stmt::State(x) => rewrite_expr(&mut x.value, aliases),
        Stmt::Decl(x) => rewrite_expr(&mut x.value, aliases),
        Stmt::Assign(x) => {
            rewrite_expr(&mut x.target, aliases);
            rewrite_expr(&mut x.value, aliases);
        }
        Stmt::Expr(e) => rewrite_expr(e, aliases),
        Stmt::Return(x) => {
            if let Some(e) = &mut x.value {
                rewrite_expr(e, aliases);
            }
        }
        Stmt::Break(x) => {
            if let Some(e) = &mut x.value {
                rewrite_expr(e, aliases);
            }
        }
        Stmt::If(x) => {
            rewrite_expr(&mut x.condition, aliases);
            rewrite_block(&mut x.then_block, aliases);
            for (e, b) in &mut x.else_if_clauses {
                rewrite_expr(e, aliases);
                rewrite_block(b, aliases);
            }
            if let Some(b) = &mut x.else_block {
                rewrite_block(b, aliases);
            }
        }
        Stmt::While(x) => {
            rewrite_expr(&mut x.condition, aliases);
            rewrite_block(&mut x.body, aliases);
        }
        Stmt::Loop(x) => rewrite_block(&mut x.body, aliases),
        Stmt::For(x) => {
            rewrite_expr(&mut x.iterable, aliases);
            rewrite_block(&mut x.body, aliases);
        }
        Stmt::Match(x) => {
            rewrite_expr(&mut x.scrutinee, aliases);
            for arm in &mut x.arms {
                if let Some(e) = &mut arm.guard {
                    rewrite_expr(e, aliases);
                }
                match &mut arm.body {
                    ast::MatchBody::Block(b) => rewrite_block(b, aliases),
                    ast::MatchBody::Expr(e) => rewrite_expr(e, aliases),
                }
            }
        }
        Stmt::Function(f) => rewrite_function_decl(f, aliases),
        Stmt::Task(t) => {
            for p in &mut t.params {
                rewrite_param(p, aliases);
            }
            rewrite_block(&mut t.body, aliases);
        }
        Stmt::Struct(s) => {
            for f in &mut s.fields {
                rewrite_type(&mut f.ty, aliases);
            }
        }
        Stmt::BareField(f) => rewrite_type(&mut f.ty, aliases),
        _ => {}
    }
}

fn rewrite_expr(expr: &mut Expr, aliases: &HashMap<String, HashSet<String>>) {
    if let Expr::Member(m) = expr {
        rewrite_expr(&mut m.object, aliases);
        // Collapse `alias.item` to `item` only when `item` is an
        // export of the aliased module. Anything else (a local
        // struct field, an unknown name) is left alone so shadowing
        // locals keep working and genuine errors stay visible.
        if let Expr::Ident(name, span) = m.object.as_ref() {
            if aliases
                .get(name)
                .map(|exports| exports.contains(&m.field))
                .unwrap_or(false)
            {
                let field = m.field.clone();
                *expr = Expr::Ident(field, span.clone());
                return;
            }
        }
    }
    match expr {
        Expr::Ident(name, _) => strip_alias_prefix(name, aliases),
        Expr::Call(c) => {
            rewrite_expr(&mut c.callee, aliases);
            for a in &mut c.args {
                rewrite_expr(&mut a.value, aliases);
            }
            if let Some(b) = &mut c.trailing_block {
                rewrite_block(b, aliases);
            }
        }
        Expr::Member(m) => rewrite_expr(&mut m.object, aliases),
        Expr::Index(i) => {
            rewrite_expr(&mut i.object, aliases);
            rewrite_expr(&mut i.index, aliases);
        }
        Expr::BinOp(x) => {
            rewrite_expr(&mut x.left, aliases);
            rewrite_expr(&mut x.right, aliases);
        }
        Expr::UnaryOp(x) => rewrite_expr(&mut x.operand, aliases),
        Expr::Try(x) => rewrite_expr(&mut x.expr, aliases),
        Expr::Range(x) => {
            rewrite_expr(&mut x.start, aliases);
            rewrite_expr(&mut x.end, aliases);
        }
        Expr::StringInterp(x) => {
            for p in &mut x.parts {
                if let ast::InterpPart::Expr(e) = p {
                    rewrite_expr(e, aliases);
                }
            }
        }
        Expr::StructLit(x) => {
            strip_alias_prefix(&mut x.name, aliases);
            for f in &mut x.fields {
                match f {
                    ast::StructField::Named(_, e) => rewrite_expr(e, aliases),
                    ast::StructField::Spread(e) => rewrite_expr(e, aliases),
                }
            }
        }
        Expr::ListLit(x) => {
            for e in &mut x.elements {
                rewrite_expr(e, aliases);
            }
        }
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
            .parent()
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("modules")
            .join(name)
    }

    /// The module fixtures directory itself, used as an extra package
    /// root: it holds `store/` (a content-addressed store shape) and
    /// `vendor/` (the committed escape hatch).
    fn fixture_dir() -> PathBuf {
        fixture("main.nv")
            .parent()
            .unwrap()
            .to_path_buf()
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
        assert!(graph
            .diagnostics
            .iter()
            .any(|(_, d)| d.code.as_deref() == Some("E0100") && !d.labels.is_empty()));
    }

    #[test]
    fn binds_direct_exported_item_alias() {
        let graph = ModuleGraph::load(&[fixture("direct.nv")]).unwrap();
        let (source, _) = graph.joined_source();
        let mut sink = DiagnosticSink::new();
        let tokens = lexer::lex(&source, &mut sink);
        let program = graph.link(parser::parse(&tokens, &mut sink));
        let program = crate::resolver::resolve(program, &mut sink);
        let _module = crate::typeck::typecheck(program, &mut sink);
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    }

    #[test]
    fn rejects_private_direct_import() {
        let graph = ModuleGraph::load(&[fixture("private_import.nv")]).unwrap();
        assert!(graph
            .diagnostics
            .iter()
            .any(|(_, d)| d.code.as_deref() == Some("E0102")));
    }

    fn check_linked(name: &str) {
        let graph = ModuleGraph::load(&[fixture(name)]).unwrap();
        let (source, _) = graph.joined_source();
        let mut sink = DiagnosticSink::new();
        let tokens = lexer::lex(&source, &mut sink);
        let program = parser::parse(&tokens, &mut sink);
        graph.emit_diagnostics(&mut sink);
        let program = graph.link(program);
        let program = crate::resolver::resolve(program, &mut sink);
        let _module = crate::typeck::typecheck(program, &mut sink);
        assert!(!sink.has_errors(), "{name}: {:?}", sink.diagnostics());
    }

    #[test]
    fn rewrites_unaliased_qualified_access() {
        // `import m` then `m.item(...)`: the export-gated rewrite
        // collapses it to the flattened `item(...)`.
        check_linked("qual.nv");
    }

    #[test]
    fn resolves_directory_module_and_supports_both_access_forms() {
        // `import dirpkg` finds `dirpkg/dirpkg.nv`; flat and
        // qualified uses both resolve.
        check_linked("dir_main.nv");
    }

    #[test]
    fn rewrites_alias_in_type_and_struct_literal_positions() {
        // `t::Config` in fn signatures, let annotations, and struct
        // literals strips to the linked `Config`.
        check_linked("typeref.nv");
    }

    #[test]
    fn keeps_dependency_locals_out_of_rewrite_scope() {
        // `support/shadow.nv` binds a local named `api` (the root's
        // import alias) with member access; root-scoped rewriting
        // must leave it alone.
        check_linked("shadow_main.nv");
    }

    #[test]
    fn warns_on_flat_use_of_private_item() {
        // `secret()` matches a private item of the imported module:
        // W0101 names the one-word fix instead of leaving a bare
        // E0201 at the use site.
        let graph = ModuleGraph::load(&[fixture("private_use.nv")]).unwrap();
        assert!(graph
            .diagnostics
            .iter()
            .any(|(_, d)| d.code.as_deref() == Some("W0101")));
    }

    #[test]
    fn resolves_committed_vendor_tree() {
        // `noct vendor` writes `vendor/<name>-<version>/lib/...`; the
        // resolver must find it so a vendored project builds offline
        // (docs/TOOLCHAIN.md §3: vendor/ is the committed escape
        // hatch). The fixture lives under `modules/vendor/`, which the
        // ancestor walk reaches from `modules/vendor_import.nv`.
        check_linked("vendor_import.nv");
    }

    #[test]
    fn vendor_takes_precedence_over_the_content_store() {
        // A committed tree beats a fetched one: same package in both
        // roots must resolve to the VENDOR copy, whatever the store
        // holds. The store fixture's copy of `shadowed` returns an Int,
        // so if the store had won, typecheck would fail on the call
        // site — reaching here with zero errors is the assertion.
        let store = fixture_dir().join("store");
        let graph =
            ModuleGraph::load_with(&[fixture("vendor_shadow.nv")], &[store]).unwrap();
        let (source, _) = graph.joined_source();
        let mut sink = DiagnosticSink::new();
        let tokens = lexer::lex(&source, &mut sink);
        let program = graph.link(parser::parse(&tokens, &mut sink));
        let program = crate::resolver::resolve(program, &mut sink);
        let _module = crate::typeck::typecheck(program, &mut sink);
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    }

    #[test]
    fn resolves_a_content_addressed_store_tree_through_its_pointer() {
        // No vendor copy: the package resolves through the store's
        // `points/<name>-<version>.point` → `trees/<hash>/lib/...`.
        let store = fixture_dir().join("store");
        let graph =
            ModuleGraph::load_with(&[fixture("store_import.nv")], &[store]).unwrap();
        assert!(
            !graph.diagnostics.iter().any(|(p, _)| p.ends_with("store_import.nv")),
            "the store tree must resolve: {:?}",
            graph.diagnostics
        );
        let (source, _) = graph.joined_source();
        let mut sink = DiagnosticSink::new();
        let tokens = lexer::lex(&source, &mut sink);
        let program = graph.link(parser::parse(&tokens, &mut sink));
        let program = crate::resolver::resolve(program, &mut sink);
        let _module = crate::typeck::typecheck(program, &mut sink);
        assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    }

    #[test]
    fn legacy_in_tree_packages_are_no_longer_searched() {
        // `.noct/packages/<name>-<version>/` is the pre-store layout and
        // is deliberately NOT searched: an unverified in-tree tree is
        // what must stop compiling. The fixture exists precisely so this
        // stays true — it must FAIL to resolve with no store root.
        let dir = fixture_dir();
        let graph = ModuleGraph::load(&[fixture("legacy_cache_import.nv")]).unwrap();
        let unresolved = graph
            .diagnostics
            .iter()
            .any(|(_, d)| d.code.as_deref() == Some("E0101"));
        assert!(
            unresolved,
            "the legacy in-tree tree must not resolve; diagnostics: {:?}",
            graph.diagnostics
        );
        let _ = dir;
    }

    #[test]
    fn warns_on_qualified_use_of_private_item() {
        // `p.private_one()` is refused (only exports are rewritten), and
        // without the hint the E0201 blames the ALIAS `p`. The hint
        // must name the private item and the one-word fix.
        let graph = ModuleGraph::load(&[fixture("private_qualified.nv")]).unwrap();
        let hint = graph
            .diagnostics
            .iter()
            .find(|(_, d)| d.code.as_deref() == Some("W0101"))
            .unwrap_or_else(|| panic!("no W0101 hint: {:?}", graph.diagnostics));
        let (_, diag) = hint;
        assert!(
            diag.message.contains("private_one")
                && diag.message.contains("export")
                && diag.message.contains("p.private_one"),
            "hint must name the item, the fix, and the qualified use: {}",
            diag.message
        );
    }

    #[test]
    fn rejects_alias_on_trait_item() {
        // Traits cannot be duplicated under a new name (E0109);
        // previously the alias silently added nothing.
        let graph = ModuleGraph::load(&[fixture("trait_alias.nv")]).unwrap();
        assert!(graph
            .diagnostics
            .iter()
            .any(|(_, d)| d.code.as_deref() == Some("E0109")));
    }
}
