//! `noct get` — fetch the whole locked dependency closure.
//!
//! [Phase 1a — the `pub get` analogue] `noct add` is the only fetch
//! path and it is per-dependency, hand-driven: with a lockfile and a
//! clean machine you would otherwise have to `noct add` every locked
//! package by hand, and `build`/`run`/`test` only answer "not in
//! cache (run `noct add` to fetch)". This command closes that gap.
//!
//! Where the bytes land is the global content store (`store.rs`,
//! TOOLCHAIN.md §3), shared by every project on the machine and keyed
//! by the lock's `content:` hash — so this command is also the
//! reclamation side of `noct clean`: after a `clean`, one `get` brings
//! the whole closure back.
//!
//! Discipline, deliberately unchanged from `add`:
//!
//! - **One fetch path.** Every registry package goes through
//!   `registry::fetch_locked` — hash check → TOFU key check →
//!   signature check → the global store (archive + read-only extracted
//!   tree + the store's own tree-hash record). There is no second,
//!   laxer route into the store, and a failed verification never writes
//!   anything (the write happens after every check passes).
//! - **Never re-download what is already good.** A package whose stored
//!   tree still verifies against the store's record is reported
//!   `verified` and left alone (`store::Store::state`). A local copy
//!   that FAILS verification is a loud refetch, never a silent
//!   overwrite of the evidence.
//! - **`path` sources are skipped, not fetched** — the tree is the
//!   user's own directory, already on disk. A `path` source whose
//!   directory is missing is a loud error (the closure is not
//!   satisfied), still never a fetch.
//! - **`git` sources fail loud.** Network fetch is deferred; there are
//!   no git bytes anywhere to verify.
//! - **A lock entry with no `content:` hash is an error, never a
//!   silent fetch.** Bytes the lock cannot name are bytes nobody can
//!   verify later.
//! - **The lock and manifest are never written.** Resolution is `add`'s
//!   job. `get` reads `nestpkg.lock` and fetches exactly what it says,
//!   byte-for-byte — this command can never change what you asked for,
//!   only make the machine match it.
//!
//! `--index` is only required when something actually has to be
//! fetched: a fully-verified closure re-runs with no index and no
//! network at all.
//!
//! Usage:
//!   noct get [--index <dir>]
//!
//! Output: one line per locked package — `<name> <version> fetched |
//! verified | skipped (path …)` — then a summary count. Exit 0 only if
//! every locked registry package ended up present AND verified (re-read
//! from disk, not assumed from what the fetches reported); 1 otherwise.

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{parse_lockfile, LockedPackage, Lockfile, Source};
use crate::registry::{fetch_locked, FileIndex};
use crate::store::{self, Store, StoreState};

/// What ended up happening to one locked package. Only the counts
/// reach the summary; the per-package line is printed inline.
#[derive(Default)]
struct Tally {
    fetched: usize,
    verified: usize,
    skipped: usize,
}

pub fn run(args: &[String]) -> i32 {
    let mut index_dir: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        // Flag handling mirrors `cmd_add`/`cmd_outdated`: `--index <dir>`
        // and `--index=<dir>` are both accepted, both loud when empty.
        if a == "--index" {
            i += 1;
            if i >= args.len() {
                eprintln!(
                    "noct get: --index requires a directory (usage: noct get [--index <dir>])"
                );
                return 1;
            }
            index_dir = Some(args[i].clone());
        } else if let Some(p) = a.strip_prefix("--index=") {
            if p.is_empty() {
                eprintln!(
                    "noct get: --index requires a directory (usage: noct get [--index <dir>])"
                );
                return 1;
            }
            index_dir = Some(p.to_string());
        } else if a == "--help" || a == "-h" {
            print_usage();
            return 0;
        } else {
            eprintln!("noct get: unknown flag `{a}` (usage: noct get [--index <dir>])");
            return 1;
        }
        i += 1;
    }

    if let Some(dir) = &index_dir {
        if !Path::new(dir).is_dir() {
            eprintln!("noct get: cannot read index `{dir}` (not a directory)");
            return 1;
        }
    }

    let lock_text = match fs::read_to_string(Path::new("nestpkg.lock")) {
        Ok(t) => t,
        Err(_) => {
            eprintln!(
                "noct get: no nestpkg.lock in current directory (run `noct add` to establish one — `get` never resolves, it only fetches what the lock already says)"
            );
            return 1;
        }
    };
    let lock: Lockfile = match parse_lockfile(&lock_text) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "noct get: invalid lockfile nestpkg.lock: {e} (run `noct add` to re-establish it)"
            );
            return 1;
        }
    };

    if lock.packages.is_empty() {
        println!("get: nothing to fetch (no locked packages)");
        return 0;
    }

    // Split the closure by source. Problems found here are collected
    // (not returned on first hit) so one run names everything wrong
    // with the lock instead of making the user fix it N times.
    let mut registry: Vec<&LockedPackage> = Vec::new();
    let mut paths: Vec<(&LockedPackage, &String)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for pkg in &lock.packages {
        match &pkg.source {
            Source::Registry => {
                // Second line of defence: `parse_lockfile` already
                // rejects a registry entry with no `content:` (it is
                // the documented lock rule), so reaching this is a
                // belt-and-braces check. Bytes the lock cannot name are
                // bytes nobody can verify later — never fetch them.
                if pkg.content.is_none() {
                    problems.push(format!(
                        "lock entry `{}` has no content hash — refusing to fetch bytes the lock cannot verify (run `noct add` to re-resolve it)",
                        pkg.name
                    ));
                } else {
                    registry.push(pkg);
                }
            }
            Source::Path(p) => paths.push((pkg, p)),
            Source::Git { url, .. } => problems.push(format!(
                "cannot fetch `{}@{}`: git source {url} needs a network fetch (deferred — no git bytes are ever cached or verified)",
                pkg.name, pkg.version
            )),
        }
    }

    let keys = crate::registry::key_dir();
    let store = Store::open();
    // Built once, used lazily: a closure that is already fully
    // verified never needs an index, so it works fully offline.
    let index = index_dir.as_ref().map(|d| FileIndex::new(PathBuf::from(d)));
    let mut tally = Tally::default();

    for (pkg, source_path) in &paths {
        let full = Path::new(source_path.as_str());
        if full.is_dir() {
            tally.skipped += 1;
            println!(
                "{} {} skipped (path source: {} — already on disk, nothing to fetch)",
                pkg.name, pkg.version, source_path
            );
        } else {
            problems.push(format!(
                "`{}@{}` path source `{}` is not a directory (it is your tree — `get` never fetches it, and the closure is not satisfied without it)",
                pkg.name, pkg.version, source_path
            ));
        }
    }

    for pkg in &registry {
        match store.state(pkg) {
            StoreState::Ready => {
                tally.verified += 1;
                println!("{} {} verified", pkg.name, pkg.version);
            }
            StoreState::NeedsFetch(reason) => {
                // A local copy that EXISTS and failed verification is a
                // repair, and repairs are announced: the user just
                // learned their store is not what the lock says.
                let on_disk = store
                    .tree_path_for(pkg)
                    .is_some_and(|tree| tree.exists())
                    || store::content_hex(pkg.content.as_deref().unwrap_or_default())
                        .is_some_and(|hex| store.archive_path(hex).exists());
                if on_disk {
                    println!(
                        "{} {} local copy rejected ({}); refetching",
                        pkg.name, pkg.version, reason
                    );
                }
                let Some(idx) = index.as_ref() else {
                    problems.push(format!(
                        "`{}@{}` is not in the local store ({reason}); pass --index <dir> to fetch it (no default registry exists)",
                        pkg.name, pkg.version
                    ));
                    continue;
                };
                // `rotate_key` is deliberately NOT exposed: re-recording
                // a publisher key is an explicit, out-of-band-verified
                // act (`noct add --rotate-key`), never a side effect of
                // a bulk fetch.
                match fetch_locked(pkg, idx, &store, &keys, false) {
                    Ok(()) => {
                        tally.fetched += 1;
                        println!("{} {} fetched", pkg.name, pkg.version);
                    }
                    Err(e) => problems.push(format!("`{}@{}`: {e}", pkg.name, pkg.version)),
                }
            }
        }
    }

    if !problems.is_empty() {
        eprintln!(
            "noct get: {} problem(s) fetching the locked closure:",
            problems.len()
        );
        for p in &problems {
            eprintln!("  - {p}");
        }
        eprintln!(
            "noct get: {} fetched, {} verified, {} skipped before the failure; the lock was not modified",
            tally.fetched, tally.verified, tally.skipped
        );
        return 1;
    }

    // Final gate. Exit 0 is a claim about the DISK, not about what the
    // fetches reported, so re-read every locked registry package's
    // state now — the same read-only check the build gate runs, minus
    // the fetch.
    let gate: Vec<String> = crate::registry::check_store_current(&lock, &store, true);
    if !gate.is_empty() {
        eprintln!("noct get: fetch reported success but the store does not verify:");
        for p in &gate {
            eprintln!("  - {p}");
        }
        return 1;
    }

    println!(
        "get: {} fetched, {} verified, {} skipped ({} locked packages, store {})",
        tally.fetched,
        tally.verified,
        tally.skipped,
        lock.packages.len(),
        store.root().display()
    );
    if let Some(line) = store::trace_line("get") {
        eprintln!("{line}");
    }
    0
}

fn print_usage() {
    println!("usage: noct get [--index <dir>]");
    println!();
    println!("Fetch every package in nestpkg.lock's closure into the global");
    println!("content store (~/.noctivue by default, $NOCT_STORE to override),");
    println!("the `pub get` analogue: with a lockfile and a clean machine you");
    println!("fetch the whole dependency set in one command instead of");
    println!("running `noct add` per dependency. The store is shared by every");
    println!("project on this machine, so a dependency you already have");
    println!("anywhere costs nothing to add here.");
    println!();
    println!("Verification is identical to `noct add` — every registry");
    println!("package is hash-checked, TOFU-key-checked and");
    println!("signature-checked before its bytes land, and the extracted");
    println!("tree is verified and placed read-only. Packages already");
    println!("present and verified are reported `verified`, never");
    println!("refetched; a local copy that fails verification is");
    println!("refetched loudly. `path` sources are skipped (already on");
    println!("disk); `git` sources fail loud (network fetch deferred).");
    println!("nestpkg.lock and nestpkg.nvpm are never modified.");
    println!();
    println!("options:");
    println!("    --index <dir>   file-backed index to fetch missing packages from");
    println!("                    (no default registry exists; only needed when");
    println!("                    something is actually missing)");
    println!("    -h, --help      print this message");
    println!();
    println!("Per-package lines are `<name> <version> fetched|verified|skipped`,");
    println!("followed by a summary count. Exit 0 only if every locked");
    println!("registry package is present and verified.");
}
