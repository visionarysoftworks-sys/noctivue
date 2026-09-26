//! `noct clean` — reclaim derived dependency data.
//!
//! [Phase 1a, extended by the global content store] The reclamation
//! half of the dependency content store story (TOOLCHAIN.md §3,
//! IMPLEMENTATION_PLAN.md Phase 6 exit criterion: "a manual `noct
//! clean` reclaims the store"). Fetched dependency content lives in the
//! global, content-addressed store (`store.rs`), so that is the primary
//! target: `archives/`, `trees/`, `index/` and `points/` under
//! `NOCT_STORE` (or the per-OS data home).
//!
//! The two in-tree directories `.noct/cache/` + `.noct/packages/` are
//! the LEGACY layout, kept as targets so a project that has them from
//! before the move can clean up after itself. They are never written
//! again and never read.
//!
//! Two things this command says out loud, every run:
//!
//! - **`vendor/` is never touched.** It is the committed offline escape
//!   hatch (TOOLCHAIN.md §3) — a `clean` that can eat a checked-in
//!   vendor tree is a footgun, so the preserved set is printed rather
//!   than left implicit. The manifest, the lock, every source file and
//!   `.noct/` itself are preserved for the same reason.
//! - **The store is global.** Reclaiming it affects EVERY project on
//!   this machine, not just the one you are standing in. That is the
//!   documented behaviour (no reference counting yet — ADR-026 defers
//!   auto-GC), so every store line says so out loud.
//!
//! - **A missing target is not an error.** "Nothing to clean" is a
//!   normal, successful outcome (running `clean` twice must be safe).
//!
//! Deletion strategy — **rename-then-delete**:
//!
//! 1. The target is `rename`d to a uniquely named trash directory in
//!    the same parent. This is the atomic step: the public name
//!    disappears in one operation, and a target containing a file
//!    another process holds open fails HERE, with nothing deleted.
//! 2. Read-only flags are cleared first (the store's trees land
//!    read-only, and Windows refuses to delete a read-only file).
//! 3. The trash directory is removed as a whole tree.
//! 4. If that fails (a partial delete is still possible on Windows once
//!    deletion has begun), the trash is renamed BACK to its public name
//!    so a working store is left in place, and the error names both
//!    paths if the rollback also fails. A loud failure naming the
//!    orphan is the outcome; a silent half-clean never is.
//!
//! Usage:
//!   noct clean [--dry-run]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::store::{self, Store};

/// The invariant this command exists to protect, printed on every run.
/// Stated positively so a user who reads only the last line still knows
/// what survived.
const PRESERVED: &str = "preserved: vendor/ (the committed offline escape hatch), nestpkg.nvpm, \
nestpkg.lock, every source file, and .noct/ itself — noct clean only ever removes the global \
content store and the legacy in-tree .noct/cache + .noct/packages";

/// One reclamation target. `global` says whose bytes these are, because
/// "clean" silently eating another project's dependencies would be the
/// surprising outcome and the operator deserves to be told first.
struct Target {
    path: PathBuf,
    global: bool,
}

pub fn run(args: &[String]) -> i32 {
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--dry-run" {
            dry_run = true;
        } else if a == "--help" || a == "-h" {
            print_usage();
            return 0;
        } else {
            eprintln!("noct clean: unknown flag `{a}` (usage: noct clean [--dry-run])");
            return 1;
        }
        i += 1;
    }

    let store = Store::open();
    let mut targets: Vec<Target> = store
        .derived_dirs()
        .into_iter()
        .map(|path| Target { path, global: true })
        .collect();
    targets.extend(
        crate::registry::derived_dirs(Path::new("."))
            .into_iter()
            .map(|path| Target { path, global: false }),
    );

    if dry_run {
        return dry_run_report(&targets);
    }

    // Collect the whole picture (including stats) before touching
    // anything, so a stat failure aborts the run with nothing removed.
    let mut planned: Vec<(&PathBuf, (u64, u64), bool)> = Vec::new();
    for target in &targets {
        if !target.path.exists() {
            continue;
        }
        match tree_stats(&target.path) {
            Ok(stats) => planned.push((&target.path, stats, target.global)),
            Err(e) => {
                eprintln!("noct clean: {e} — nothing was removed");
                println!("{PRESERVED}");
                return 1;
            }
        }
    }

    if planned.is_empty() {
        println!(
            "clean: nothing to clean (no store content, no legacy .noct/cache or .noct/packages here)"
        );
        println!("{PRESERVED}");
        return 0;
    }

    let mut files = 0u64;
    let mut bytes = 0u64;
    for (target, (f, b), global) in &planned {
        if let Err(e) = discard(target) {
            // Loud and specific: what failed, why, and what is left.
            eprintln!("noct clean: {e}");
            eprintln!(
                "noct clean: aborted with {} target(s) already removed — re-run to finish",
                planned.iter().filter(|(t, _, _)| !t.exists()).count()
            );
            println!("{PRESERVED}");
            return 1;
        }
        files += f;
        bytes += b;
        println!(
            "removed {} ({f} file{}, {}{})",
            display(target),
            plural(*f),
            human_bytes(*b),
            scope(*global)
        );
    }

    println!(
        "clean: reclaimed {files} file{}, {}",
        plural(files),
        human_bytes(bytes)
    );
    if let Some(line) = store::trace_line("clean") {
        eprintln!("{line}");
    }
    println!("{PRESERVED}");
    0
}

/// The parenthetical that says whose bytes these were.
fn scope(global: bool) -> &'static str {
    if global {
        " — the global store, shared by every project on this machine"
    } else {
        " — legacy in-tree copy (pre-store layout)"
    }
}

/// `--dry-run`: list exactly what would go, change nothing.
fn dry_run_report(targets: &[Target]) -> i32 {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut present = 0usize;
    for target in targets {
        if !target.path.exists() {
            println!(
                "dry-run: {} absent (nothing to remove)",
                display(&target.path)
            );
            continue;
        }
        match tree_stats(&target.path) {
            Ok((f, b)) => {
                present += 1;
                files += f;
                bytes += b;
                println!(
                    "dry-run: would remove {} ({f} file{}, {}{})",
                    display(&target.path),
                    plural(f),
                    human_bytes(b),
                    scope(target.global)
                );
            }
            Err(e) => {
                eprintln!("noct clean: cannot stat {e} — nothing was changed");
                println!("{PRESERVED}");
                return 1;
            }
        }
    }
    if present == 0 {
        println!("dry-run: nothing to clean (no store content, no legacy .noct/cache or .noct/packages here)");
    } else {
        println!(
            "dry-run: {files} file{}, {} would be reclaimed; nothing was changed",
            plural(files),
            human_bytes(bytes)
        );
    }
    println!("{PRESERVED}");
    0
}

/// Remove one derived tree: rename it aside, clear the read-only
/// flags the store placed, then delete the trash. See the module docs
/// for why this is not a bare `remove_dir_all`.
fn discard(target: &Path) -> Result<(), String> {
    let trash = unique_trash_path(target);
    if let Err(e) = fs::rename(target, &trash) {
        return Err(format!(
            "cannot reclaim {}: cannot move it aside ({e}) — nothing under it was deleted (a file held open by another process, or an unwritable parent, blocks this)",
            target.display()
        ));
    }
    // Store trees are placed read-only, and Windows refuses to delete a
    // read-only file: the flags come off before the delete, and a
    // failure here restores the tree rather than stranding it.
    if let Err(e) = store::clear_readonly_tree(&trash) {
        let _ = fs::rename(&trash, target);
        return Err(format!(
            "cannot reclaim {}: read-only flags could not be cleared ({e}) — restored, nothing under it was deleted",
            target.display()
        ));
    }
    let result = if trash.is_dir() {
        fs::remove_dir_all(&trash)
    } else {
        fs::remove_file(&trash)
    };
    if let Err(e) = result {
        // Roll back so the caller keeps a usable store rather than a
        // renamed-away one. If the rollback also fails, say exactly
        // where the bytes went.
        return match fs::rename(&trash, target) {
            Ok(()) => Err(format!(
                "cannot reclaim {}: moved aside to {} but deleting it failed ({e}) — restored to {} , nothing under it was deleted",
                target.display(),
                trash.display(),
                target.display()
            )),
            Err(rollback) => Err(format!(
                "cannot reclaim {}: moved aside to {} and deleting it failed ({e}); restoring it also failed ({rollback}) — the bytes are intact but stranded at {}",
                target.display(),
                trash.display(),
                trash.display()
            )),
        };
    }
    Ok(())
}

/// A unique sibling path for the temporary rename. Same parent ⇒ same
/// volume, so the rename cannot degrade into a copy.
fn unique_trash_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "data".to_string());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let mut candidate = parent.join(format!(".noct-clean-{name}-{pid}-{stamp}"));
    let mut n = 0u32;
    while candidate.exists() {
        n += 1;
        candidate = parent.join(format!(".noct-clean-{name}-{pid}-{stamp}-{n}"));
    }
    candidate
}

/// `(file count, total bytes)` for a whole tree, without following the
/// "partial success" trap: an unreadable entry is an `Err`, so a
/// removal is never planned on top of a tree we could not read.
fn tree_stats(root: &Path) -> Result<(u64, u64), String> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read an entry of {}: {e}", dir.display()))?;
            let path = entry.path();
            let meta = entry
                .metadata()
                .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if meta.is_dir() {
                stack.push(path);
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    Ok((files, bytes))
}

/// Paths as `.noct/cache`, not `.noct\cache` — the message is read by
/// humans comparing against docs, so normalize the separators and drop
/// the leading `./` that `Path::new(".").join(..)` leaves behind. (A
/// drive prefix keeps its own colon: `C:/Users/…`, never `C:/\/Users/…`.)
fn display(path: &Path) -> String {
    let rendered = path.to_string_lossy().replace('\\', "/");
    rendered
        .strip_prefix("./")
        .map(str::to_string)
        .unwrap_or(rendered)
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn print_usage() {
    println!("usage: noct clean [--dry-run]");
    println!();
    println!("Reclaim derived dependency data: the global content store");
    println!("(archives + extracted trees + records + pointers) and the");
    println!("legacy in-tree .noct/cache/ + .noct/packages/ pair. `noct get`");
    println!("recreates the store from nestpkg.lock, so cleaning costs only a");
    println!("re-fetch.");
    println!();
    println!("The store is shared by every project on this machine, so");
    println!("reclaiming it is machine-wide, not project-local; every store");
    println!("line says so. vendor/ (the committed offline escape hatch),");
    println!("nestpkg.nvpm, nestpkg.lock, every source file and the .noct/");
    println!("directory itself are all preserved, and the preserved set is");
    println!("printed on every run. A missing target is not an error.");
    println!();
    println!("options:");
    println!("    --dry-run   list exactly what would be removed (with file counts");
    println!("                and sizes) and change nothing");
    println!("    -h, --help  print this message");
}
