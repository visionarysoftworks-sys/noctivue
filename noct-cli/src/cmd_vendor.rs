//! `noct vendor` — copy the full resolved dependency closure into a local vendor directory.
//!
//! [Phase 4] Offline-friendly: every byte comes from local state — the
//! global content store's `archives/<sha256>.pkg` (unpacked here with
//! `registry::unpack`) or its already-verified `trees/<sha256>/` tree,
//! or the referenced tree for `path` sources. No index lookup, no
//! network. The store is shared with every other project, so vendoring
//! is how a project becomes air-gapped (TOOLCHAIN.md §3: `vendor/` is
//! the committed offline escape hatch, and it precedes the store in the
//! resolver's search order).
//!
//! Layout: `<dir>/<name>-<version>/…` (one directory per locked
//! package; lock order is name-sorted). One line per package is printed:
//! `vendored <name> <version> -> <dir>/<name>-<version>`.
//!
//! A package with no local bytes (store entry reclaimed by `noct clean`,
//! never fetched, or a `git` source, whose network fetch is deferred)
//! is a loud error naming the package and the store — never a silent
//! skip.
//!
//! Usage:
//!   noct vendor [--dir <path>]

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{LockedPackage, Source};
use crate::registry::unpack;
use crate::store::{Store, StoreState};

pub fn run(args: &[String]) -> i32 {
    let mut dir = PathBuf::from("vendor");
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--dir" {
            i += 1;
            if i >= args.len() {
                eprintln!("noct vendor: --dir requires a value (usage: noct vendor [--dir <path>])");
                return 1;
            }
            dir = PathBuf::from(&args[i]);
        } else if let Some(p) = a.strip_prefix("--dir=") {
            if p.is_empty() {
                eprintln!("noct vendor: --dir requires a value (usage: noct vendor [--dir <path>])");
                return 1;
            }
            dir = PathBuf::from(p);
        } else if a == "--help" || a == "-h" {
            print_usage();
            return 0;
        } else {
            eprintln!("noct vendor: unknown flag `{a}` (usage: noct vendor [--dir <path>])");
            return 1;
        }
        i += 1;
    }

    let lock_text = match fs::read_to_string(Path::new("nestpkg.lock")) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("noct vendor: no nestpkg.lock in current directory (run `noct add` first)");
            return 1;
        }
    };
    let lock = match crate::manifest::parse_lockfile(&lock_text) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("noct vendor: invalid lockfile: {e}");
            return 1;
        }
    };

    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("noct vendor: cannot create {}: {e}", dir.display());
        return 1;
    }

    if lock.packages.is_empty() {
        println!("nothing to vendor (no locked packages)");
        return 0;
    }

    let store = Store::open();

    for pkg in &lock.packages {
        let dest = dir.join(format!("{}-{}", pkg.name, pkg.version));
        let (result, notice) = match &pkg.source {
            Source::Path(p) => (vendor_path(&pkg.name, Path::new(p), &dest), None),
            Source::Registry => match vendor_registry(pkg, &store, &dest) {
                Ok(notice) => (Ok(()), notice),
                Err(e) => (Err(e), None),
            },
            Source::Git { url, .. } => (
                Err(format!(
                    "no local bytes for `{}@{}` (git source {url} was never fetched — network fetch is deferred)",
                    pkg.name, pkg.version
                )),
                None,
            ),
        };
        if let Err(e) = result {
            eprintln!("noct vendor: {e}");
            return 1;
        }
        if let Some(line) = notice {
            println!("{line}");
        }
        println!(
            "vendored {} {} -> {}",
            pkg.name,
            pkg.version,
            dest.display()
        );
    }
    0
}

fn print_usage() {
    println!("usage: noct vendor [--dir <path>]");
    println!();
    println!("Copy the full resolved dependency closure into a local vendor");
    println!("directory (default `vendor/`). Works offline from the global");
    println!("content store and the lockfile; fails loudly when a package has");
    println!("no local bytes available.");
}

/// Vendor one registry package. Returns a notice to print when the
/// store was not in its verified state, so a repair is visible rather
/// than silent.
///
/// The verified tree is the unit of compilation and is copied as-is
/// (it was just verified against the store's own record, so nothing
/// needs re-hashing). When the store cannot vouch for its tree, the
/// ARCHIVE is re-unpacked instead — and the archive is re-hashed
/// against the lock on the way out, so a tampered archive fails closed
/// here and never lands in a committed `vendor/` tree. Half a store is
/// still vendorable; an unverified one is not vendorable at all.
fn vendor_registry(
    pkg: &LockedPackage,
    store: &Store,
    dest: &Path,
) -> Result<Option<String>, String> {
    let name = &pkg.name;
    let version = pkg.version;
    match store.state(pkg) {
        StoreState::Ready => {
            let tree = store
                .tree_path_for(pkg)
                .ok_or_else(|| format!("`{name}@{version}` has no usable content hash"))?;
            copy_tree(&tree, dest)?;
            Ok(None)
        }
        StoreState::NeedsFetch(why) => {
            let bytes = store
                .read_archive(pkg)
                .map_err(|e| format!("no local bytes for `{name}@{version}` ({why}; {e})"))?;
            let entries = unpack(&bytes)
                .map_err(|e| format!("cannot unpack stored `{name}@{version}`: {e}"))?;
            write_entries(&entries, dest)?;
            Ok(Some(format!(
                "{} {version}: the store's tree is not usable ({why}); vendored from the verified archive instead",
                name
            )))
        }
    }
}

/// Vendor one path package: copy the referenced sibling tree.
fn vendor_path(name: &str, source: &Path, dest: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "cannot vendor `{name}`: path source `{}` not found",
            source.display()
        ));
    }
    copy_tree(source, dest)
}

/// Write unpacked `(relative-path, bytes)` entries under `dest`,
/// clearing it first so re-runs are byte-identical.
fn write_entries(entries: &[(String, Vec<u8>)], dest: &Path) -> Result<(), String> {
    if dest.exists() {
        fs::remove_dir_all(dest)
            .map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
    }
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    for (rel, bytes) in entries {
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create dir: {e}"))?;
        }
        fs::write(&target, bytes).map_err(|e| format!("cannot write {rel}: {e}"))?;
    }
    Ok(())
}

/// Recursively copy a directory tree (`src` must be a directory),
/// clearing `dest` first so re-runs are byte-identical.
fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!("source directory {} not found", src.display()));
    }
    if dest.exists() {
        fs::remove_dir_all(dest)
            .map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
    }
    let mut stack = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((s, d)) = stack.pop() {
        fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        let entries =
            fs::read_dir(&s).map_err(|e| format!("cannot read {}: {e}", s.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("cannot read dir entry: {e}"))?;
            let (child_src, child_dst) = (entry.path(), d.join(entry.file_name()));
            if child_src.is_dir() {
                stack.push((child_src, child_dst));
            } else {
                fs::copy(&child_src, &child_dst)
                    .map_err(|e| format!("cannot copy {}: {e}", child_src.display()))?;
            }
        }
    }
    Ok(())
}
