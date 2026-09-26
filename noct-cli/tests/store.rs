//! CLI tests for the GLOBAL content-addressed dependency store
//! (TOOLCHAIN.md §3, ADR-026, IMPLEMENTATION_PLAN.md Phase 6 exit
//! criterion).
//!
//! Everything here is offline and hermetic: a programmatic signed file
//! index plus scratch projects, with `NOCT_KEYS` and `NOCT_STORE` both
//! pinned inside the scratch tree, so the developer's real key store and
//! real global store are never read or written.
//!
//! What these cases are here to prove, in the order the design states
//! them:
//!
//! 1. fetched content is GLOBAL and shared — two projects, one fetch,
//!    both build offline;
//! 2. the archive is hashed ONCE, at fetch, and no build re-hashes it
//!    (proved with the counters, not a comment);
//! 3. a hand-edited extracted tree does NOT compile — the blocker this
//!    store exists to close;
//! 4. `--frozen` and `--offline` both refuse to fetch, auto-fetch fills
//!    a genuine miss, and a stale lock still errors without fetching;
//! 5. a legacy in-tree `.noct/cache` + `.noct/packages` project still
//!    builds, and its unverified trees are never trusted;
//! 6. `NOCT_STORE` really redirects the store, and `noct clean` reclaims
//!    it while preserving vendor/, the manifest and the lock.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

/// Generous, because several cases run a real `noct build` (codegen plus
/// the cargo link against `runtime-native`, which is cached in a shared
/// target dir after the first). The same reasoning and value as
/// `native_build.rs`'s `BUILD_TIMEOUT_SECS`; the lockfile/store cases
/// that never invoke cargo finish in well under a second.
const CASE_TIMEOUT_SECS: u64 = 300;
const SEED: [u8; 32] = [0x42; 32];
const KEY_ID: &str = "key:4242";

/// The published `leaf` module: the extracted tree a build must
/// reproduce, byte for byte.
const GOOD_LEAF: &str = "export fn which() -> String:\n    \"store\"\n";
/// The same module, hand-edited: the tamper every integrity case uses.
const PWNED_LEAF: &str = "export fn which() -> String:\n    \"pwned\"\n";
/// What a pre-store `.noct/packages/` tree might have contained.
const LEGACY_LEAF: &str = "export fn which() -> String:\n    \"legacy\"\n";

// ── Harness ─────────────────────────────────────────────────────────────────

/// One scratch tree: `base/{work,work2,keys,index,store}`.
struct Scratch {
    base: PathBuf,
    dir: PathBuf,
    other: PathBuf,
    keys: PathBuf,
    index: PathBuf,
    store: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("noctivue-store-{tag}-{}-{id}", std::process::id()));
        let dir = base.join("work");
        let other = base.join("work2");
        let keys = base.join("keys");
        let index = base.join("index");
        let store = base.join("store");
        for dir in [&dir, &other, &keys] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Scratch {
            base,
            dir,
            other,
            keys,
            index,
            store,
        }
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }

    /// `noct` in `project`, with the key store AND the content store
    /// pinned. `store: Option<&Path>` overrides the store root, which is
    /// how the `$NOCT_STORE` cases prove the override is honoured.
    fn run_in(&self, project: &Path, store: Option<&Path>, args: &[&str]) -> Output {
        let mut child = Command::new(noct_bin());
        child
            .args(args)
            .current_dir(project)
            .env("NOCT_KEYS", &self.keys)
            .env("NOCT_STORE", store.unwrap_or(&self.store))
            .env_remove("NOCT_STORE_REVERIFY")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = child.spawn().expect("failed to spawn noct binary");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(CASE_TIMEOUT_SECS)) {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => panic!("failed waiting on noct binary: {e}"),
            Err(_) => panic!(
                "TIMEOUT (>{CASE_TIMEOUT_SECS}s): `noct {}` hung",
                args.join(" ")
            ),
        }
    }

    /// A run with `NOCT_STORE_TRACE=1`, so the store's hashing counters
    /// are observable from outside the process.
    fn run_traced(&self, project: &Path, args: &[&str]) -> Output {
        let child = Command::new(noct_bin())
            .args(args)
            .current_dir(project)
            .env("NOCT_KEYS", &self.keys)
            .env("NOCT_STORE", &self.store)
            .env("NOCT_STORE_TRACE", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn noct binary");
        child.wait_with_output().expect("wait")
    }

    /// The `archives=N trees=M …` line the store prints under
    /// `NOCT_STORE_TRACE`, as `(archives, trees)`.
    fn counters(&self, out: &Output) -> (u64, u64) {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let line = stderr
            .lines()
            .find(|l| l.starts_with("store-verify["))
            .unwrap_or_else(|| panic!("no store-verify trace line in:\n{stderr}"));
        let field = |key: &str| -> u64 {
            line.split_whitespace()
                .find_map(|t| t.strip_prefix(&format!("{key}=")))
                .unwrap_or_else(|| panic!("no `{key}:` in trace line `{line}`"))
                .parse()
                .expect("counter is a number")
        };
        (field("archives"), field("trees"))
    }

    fn index_arg(&self) -> String {
        self.index.to_string_lossy().into_owned()
    }

    /// A signed file index holding the fixture closure, written into
    /// `self.index`.
    fn build_index(&self) {
        let sk = SigningKey::from_bytes(&SEED);
        std::fs::create_dir_all(self.index.join("_keys")).unwrap();
        std::fs::write(
            self.index.join("_keys").join(format!("{KEY_ID}.pub")),
            format!("{}\n", hex(&sk.verifying_key().to_bytes())),
        )
        .unwrap();
        let vdir = self.index.join("leaf").join("1.4.0");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(
            vdir.join("manifest.nvpm"),
            "package:\n    name: leaf\n    version: 1.4.0\n",
        )
        .unwrap();
        let bytes = tarball(&[("lib/main.nv", GOOD_LEAF)]);
        let hash = sha256_hex(&bytes);
        let payload = format!("noctivue-publish-v1:leaf@1.4.0:{hash}");
        std::fs::write(vdir.join("pkg.bin"), &bytes).unwrap();
        std::fs::write(vdir.join("pkg.hash"), format!("{hash}\n")).unwrap();
        std::fs::write(
            vdir.join("pkg.sig"),
            format!("{}\n", hex(&sk.sign(payload.as_bytes()).to_bytes())),
        )
        .unwrap();
        std::fs::write(vdir.join("pkg.key"), format!("{KEY_ID}\n")).unwrap();
    }

    /// A project whose `main.nv` imports the package by name (so a
    /// successful run proves the resolver found the tree through the
    /// store) and whose manifest declares it (so a lock copied from
    /// another project is current here).
    fn write_project(&self, project: &Path, package: &str) {
        std::fs::create_dir_all(project).unwrap();
        std::fs::write(
            project.join("nestpkg.nvpm"),
            format!(
                "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    {package}: 1.4.0\n"
            ),
        )
        .unwrap();
        std::fs::write(
            project.join("main.nv"),
            format!("import {package}::main as dep\n\nmain():\n    println(dep.which())\n"),
        )
        .unwrap();
    }

    /// `add <package> --index`, the one path into the store.
    fn add(&self, project: &Path, package: &str) -> Output {
        let idx = self.index_arg();
        let out = self.run_in(project, None, &["add", package, "--index", &idx]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "setup add failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut out = String::from("sha256:");
    for b in Sha256::digest(bytes) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (path, content) in files {
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(content.len() as u64).to_le_bytes());
        out.extend_from_slice(content.as_bytes());
    }
    out
}

/// The store's tree for one locked package, via its by-name pointer —
/// the same path the module resolver walks.
fn stored_tree(store: &Path, name: &str, version: &str) -> PathBuf {
    let point = store.join("points").join(format!("{name}-{version}.point"));
    let relative = std::fs::read_to_string(&point)
        .unwrap_or_else(|e| panic!("read {}: {e}", point.display()));
    store.join(relative.trim())
}

/// Clear the read-only flag the store places, recursively.
///
/// This is the honest part of the read-only story, exercised on purpose:
/// read-only is a DETERRENT, not a security boundary (see `store.rs`), so
/// a determined local editor CAN write into a store tree. The hash check
/// is what has to notice — and the tests below prove it does.
fn make_writable(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    }
    let mut perms = meta.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

/// Every file under `root` as `(relative path, bytes)`, sorted — the
/// byte-identity witness for "this command changed nothing".
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

// ── 1. One store, shared across projects ────────────────────────────────────

#[test]
fn two_projects_share_one_store_and_both_build_offline() {
    let s = Scratch::new("shared");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.write_project(&s.other, "leaf");

    // One fetch, in the first project.
    s.add(&s.dir, "leaf");
    let store_entries = |store: &Path, sub: &str| -> usize {
        std::fs::read_dir(store.join(sub)).map(|d| d.count()).unwrap_or(0)
    };
    assert_eq!(store_entries(&s.store, "archives"), 1);
    assert_eq!(store_entries(&s.store, "trees"), 1);

    // The second project arrives with the same lock (a lockfile is
    // checked in, so this is the ordinary case) and NO index: the
    // content is already in the store, keyed by the same `content:`
    // hash, so `get` verifies it and downloads nothing at all.
    std::fs::copy(s.dir.join("nestpkg.lock"), s.other.join("nestpkg.lock")).unwrap();
    let out = s.run_in(&s.other, None, &["get"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("leaf 1.4.0 verified"),
        "the second project must verify against the shared store, not refetch:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Still one copy, and no index was ever needed for the second one.
    assert_eq!(store_entries(&s.store, "archives"), 1);
    assert_eq!(store_entries(&s.store, "trees"), 1);

    // Both projects build with the index and the network nowhere in
    // sight: the store is the only source.
    for project in [&s.dir, &s.other] {
        let out = s.run_in(project, None, &["run", "main.nv"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "offline run failed. stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("store"), "got:\n{stdout}");
    }
    // Nothing landed in either project tree: in-tree holds build outputs
    // only now.
    for project in [&s.dir, &s.other] {
        assert!(!project.join(".noct/packages").exists());
        assert!(!project.join(".noct/cache").exists());
    }
    s.cleanup();
}

// ── 2. Archives hash once, at fetch ─────────────────────────────────────────

#[test]
fn a_second_build_hashes_nothing_at_all() {
    // The performance claim of the whole change, asserted rather than
    // asserted-in-a-comment: the FIRST build of a cold store hashes the
    // archive once (at fetch) and the extracted tree once (to record its
    // hash); every build after that hashes NEITHER.
    let s = Scratch::new("warm");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");

    let out = s.run_traced(&s.dir, &["run", "main.nv"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "cold build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold = s.counters(&out);

    let out = s.run_traced(&s.dir, &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(0));
    let warm = s.counters(&out);

    assert_eq!(
        warm.0, 0,
        "a warm build must not re-hash a single archive byte (cold was {cold:?})"
    );
    assert_eq!(
        warm.1, 0,
        "a warm build must not re-hash the extracted tree (cold was {cold:?})"
    );
    // And the warm run really did check the store (a gate that skipped
    // verification would also report zero).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("fingerprints="),
        "the warm gate must still walk the tree:\n{stderr}"
    );
    s.cleanup();
}

#[test]
fn the_store_records_the_tree_hash_at_fetch() {
    // "Hash once at fetch" must be *recorded*, not implied: the store's
    // own index file carries the tree hash and the fingerprint, and the
    // build compares against it.
    let s = Scratch::new("record");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");

    let tree = stored_tree(&s.store, "leaf", "1.4.0");
    let hex = tree.file_name().unwrap().to_string_lossy().into_owned();
    let record = std::fs::read_to_string(s.store.join("index").join(format!("{hex}.record")))
        .expect("the store must record what it verified");
    assert!(record.contains("content: sha256:"), "{record}");
    assert!(record.contains("tree: sha256:"), "{record}");
    assert!(record.contains("fingerprint: sha256:"), "{record}");
    // The lock's content hash is the archive half; the tree hash is the
    // store's own, and the two are different values.
    let lock = std::fs::read_to_string(s.dir.join("nestpkg.lock")).unwrap();
    let locked = lock
        .lines()
        .find_map(|l| l.trim().strip_prefix("content: "))
        .expect("lock records a content hash")
        .to_string();
    let tree_hash = record
        .lines()
        .find_map(|l| l.trim().strip_prefix("tree: "))
        .unwrap()
        .to_string();
    assert_eq!(locked, format!("sha256:{hex}"));
    assert_ne!(locked, tree_hash, "the tree hash is not the archive hash");
    s.cleanup();
}

// ── 3. The blocker: a hand-edited tree must not compile ─────────────────────

#[test]
fn a_hand_edited_extracted_tree_must_not_compile() {
    // THE regression test. Before the store, an edited file inside the
    // extracted dependency tree compiled and shipped: nothing hashed the
    // tree, only the archive. Now the build re-verifies the tree against
    // the store's record, and the edit fails the build loudly.
    let s = Scratch::new("tamper");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");

    // Sanity: untampered, it runs.
    let out = s.run_in(&s.dir, None, &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));

    // The tamper: one file, one line, the read-only flag cleared first
    // (read-only is a deterrent, not a boundary — so the boundary has to
    // be the hash).
    let lib = stored_tree(&s.store, "leaf", "1.4.0").join("lib/main.nv");
    make_writable(&lib);
    std::fs::write(&lib, PWNED_LEAF).unwrap();

    for command in [vec!["run", "main.nv"], vec!["build", "main.nv"]] {
        let out = s.run_in(&s.dir, None, &command);
        assert_eq!(
            out.status.code(),
            Some(1),
            "`noct {}` compiled a tampered dependency tree",
            command.join(" ")
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("extracted tree hash mismatch"),
            "must name the integrity failure, got:\n{stderr}"
        );
        assert!(
            stderr.contains("leaf@1.4.0"),
            "must name the package, got:\n{stderr}"
        );
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("pwned"),
            "the tampered bytes must never run"
        );
    }

    // `noct get` repairs it loudly, from the verified archive, rather
    // than patching bytes in place.
    let out = s.run_in(&s.dir, None, &["get", "--index", &s.index_arg()]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("local copy rejected"), "got:\n{stdout}");
    assert_eq!(
        std::fs::read_to_string(stored_tree(&s.store, "leaf", "1.4.0").join("lib/main.nv")).unwrap(),
        GOOD_LEAF
    );
    s.cleanup();
}

// ── 4. Auto-fetch, `--offline`, `--frozen`, stale lock ──────────────────────

#[test]
fn auto_fetch_fills_a_missing_dependency_and_the_build_then_succeeds() {
    let s = Scratch::new("autofetch");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");
    // A clean machine: the lock survives, the store does not.
    std::fs::remove_dir_all(&s.store).unwrap();

    // No --index: a miss is a loud error naming the closure command.
    let out = s.run_in(&s.dir, None, &["build", "main.nv"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("noct get"), "must point at `get`:\n{stderr}");
    assert!(
        !stderr.contains("noct add"),
        "a missing store entry is not a lock problem:\n{stderr}"
    );

    // With --index: fetched on the spot, and the fetch is VISIBLE.
    let out = s.run_in(&s.dir, None, &["build", "main.nv", "--index", &s.index_arg()]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "auto-fetch then build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("auto-fetching `leaf@1.4.0` into the store"),
        "the auto-fetch must be announced, with what and why:\n{stderr}"
    );
    // Same path, same verification: the tree is present and usable.
    assert!(stored_tree(&s.store, "leaf", "1.4.0").join("lib/main.nv").is_file());
    let out = s.run_in(&s.dir, None, &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("store"));
    s.cleanup();
}

#[test]
fn offline_and_frozen_both_refuse_to_fetch() {
    let s = Scratch::new("optout");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");

    // `--offline` is a hard opt-out even WITH an index: the flag is what
    // stopped it, and the message says so.
    std::fs::remove_dir_all(&s.store).unwrap();
    let out = s.run_in(
        &s.dir,
        None,
        &["build", "main.nv", "--offline", "--index", &s.index_arg()],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--offline"), "got:\n{stderr}");
    assert!(!stderr.contains("auto-fetching"), "got:\n{stderr}");
    assert!(!s.store.exists(), "--offline must not have fetched anything");

    // `--frozen` is metadata-only: it does not consult the store at all,
    // so it cannot fetch, and a store-less project fails on the missing
    // SOURCES (a compile error) rather than on an integrity verdict.
    let out = s.run_in(
        &s.dir,
        None,
        &["build", "main.nv", "--frozen", "--index", &s.index_arg()],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("auto-fetching"), "--frozen fetched:\n{stderr}");
    assert!(
        !stderr.contains("not current"),
        "--frozen must not fail the store gate at all:\n{stderr}"
    );
    assert!(!s.store.exists(), "--frozen must not have fetched anything");

    // With a prepared store, `--frozen` builds fine and leaves the store
    // byte-identical: it is the hermetic-CI flag (prepare the store
    // deliberately, then never touch it).
    s.add(&s.dir, "leaf");
    let before = snapshot(&s.store);
    let out = s.run_in(&s.dir, None, &["build", "main.nv", "--frozen"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "--frozen build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(snapshot(&s.store), before, "--frozen must not write");

    // The documented cost of metadata-only: `--frozen` skips tree
    // verification, so a tampered tree is NOT caught there. This is
    // stated rather than hidden — the flag's contract is "the store is
    // what I prepared", and any other build catches the edit (see
    // `a_hand_edited_extracted_tree_must_not_compile`).
    let lib = stored_tree(&s.store, "leaf", "1.4.0").join("lib/main.nv");
    make_writable(&lib);
    std::fs::write(&lib, PWNED_LEAF).unwrap();
    let out = s.run_in(&s.dir, None, &["build", "main.nv", "--frozen"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("hash mismatch"),
        "--frozen is metadata-only by contract; it must not run verification:\n{stderr}"
    );
    // ...while a normal build does catch it, with no flags at all.
    let out = s.run_in(&s.dir, None, &["build", "main.nv"]);
    assert_eq!(out.status.code(), Some(1), "a plain build must catch the edit");
    assert!(String::from_utf8_lossy(&out.stderr).contains("extracted tree hash mismatch"));
    s.cleanup();
}

#[test]
fn a_stale_lock_still_errors_without_fetching() {
    let s = Scratch::new("stale");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");
    std::fs::remove_dir_all(&s.store).unwrap();

    // The manifest gains a dependency the lock does not know about. That
    // is a lock problem, and auto-fetch must not paper over it: fetching
    // fills the store from the lock, it never re-resolves.
    std::fs::write(
        s.dir.join("nestpkg.nvpm"),
        "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    leaf: 1.4.0\n    other: 1.0.0\n",
    )
    .unwrap();

    let out = s.run_in(&s.dir, None, &["build", "main.nv", "--index", &s.index_arg()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("noct add"), "a stale lock points at add:\n{stderr}");
    assert!(
        !stderr.contains("auto-fetching"),
        "a stale lock must never trigger a fetch:\n{stderr}"
    );
    assert!(!s.store.exists(), "nothing may be fetched for a stale lock");
    s.cleanup();
}

// ── 5. Legacy in-tree layout ───────────────────────────────────────────────

#[test]
fn a_legacy_in_tree_project_refetches_and_never_trusts_the_old_tree() {
    // Migration, chosen and tested: the legacy in-tree pairs are IGNORED
    // and refetched into the store through the normal verified path.
    // Adopting them "with verification" was the alternative; it is a
    // second route into the store that skips the index (and with it the
    // signature re-check), and the toolchain's own rule is that there is
    // exactly one fetch path. The cost is one `noct get` per machine.
    let s = Scratch::new("legacy");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");
    // Fabricate the pre-store layout, with a DIFFERENT (hand-edited)
    // tree in it — the exact hazard this change closes.
    std::fs::create_dir_all(s.dir.join(".noct/cache")).unwrap();
    std::fs::create_dir_all(s.dir.join(".noct/packages/leaf-1.4.0/lib")).unwrap();
    std::fs::write(s.dir.join(".noct/cache/leaf-1.4.0.pkg"), b"legacy-bytes").unwrap();
    std::fs::write(s.dir.join(".noct/packages/leaf-1.4.0/lib/main.nv"), LEGACY_LEAF).unwrap();
    // And remove the store, so the build has to find its bytes somewhere.
    std::fs::remove_dir_all(&s.store).unwrap();

    // 1. The legacy tree alone cannot satisfy the build: the run fails
    //    rather than compiling those bytes.
    let out = s.run_in(&s.dir, None, &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("noct get"), "must say how to fix it:\n{stderr}");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("legacy"),
        "the legacy tree must not run"
    );

    // 2. One fetch into the store, and the build compiles the VERIFIED
    //    tree — not the legacy one, which is still sitting there.
    let out = s.run_in(&s.dir, None, &["run", "main.nv", "--index", &s.index_arg()]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the store refetch must satisfy the build:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("store"), "the verified tree is what runs:\n{stdout}");
    assert!(!stdout.contains("legacy"), "the legacy tree must not run:\n{stdout}");

    // 3. `noct clean` reclaims the legacy pair as well.
    let out = s.run_in(&s.dir, None, &["clean"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(!s.dir.join(".noct/packages").exists());
    assert!(!s.dir.join(".noct/cache").exists());
    assert!(s.dir.join(".noct").is_dir(), ".noct/ itself survives");
    s.cleanup();
}

// ── 6. `NOCT_STORE` and `noct clean` ────────────────────────────────────────

#[test]
fn noct_store_redirects_the_store() {
    let s = Scratch::new("override");
    s.build_index();
    s.write_project(&s.dir, "leaf");

    let elsewhere = s.base.join("elsewhere");
    let idx = s.index_arg();
    let out = s.run_in(
        &s.dir,
        Some(&elsewhere),
        &["add", "leaf", "--index", &idx],
    );
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        elsewhere.join("archives").is_dir(),
        "$NOCT_STORE must receive the bytes"
    );
    assert!(
        !s.store.exists(),
        "the default root must stay untouched when overridden"
    );
    // And the build follows the override.
    let out = s.run_in(&s.dir, Some(&elsewhere), &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    // Without it, the same project is a clean machine again.
    let out = s.run_in(&s.dir, None, &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(1));
    s.cleanup();
}

#[test]
fn clean_reclaims_the_store_and_preserves_the_escape_hatch() {
    let s = Scratch::new("clean");
    s.build_index();
    s.write_project(&s.dir, "leaf");
    s.add(&s.dir, "leaf");
    // The committed offline escape hatch, and build outputs in-tree.
    let out = s.run_in(&s.dir, None, &["vendor"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::create_dir_all(s.dir.join(".noct/build")).unwrap();
    std::fs::write(s.dir.join(".noct/build/app.bin"), b"\x00\x01").unwrap();
    let manifest = std::fs::read(s.dir.join("nestpkg.nvpm")).unwrap();
    let lock = std::fs::read(s.dir.join("nestpkg.lock")).unwrap();

    let out = s.run_in(&s.dir, None, &["clean"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    for sub in ["archives", "trees", "index", "points"] {
        assert!(
            stdout.contains(&format!(
                "removed {}",
                s.store.join(sub).to_string_lossy().replace('\\', "/")
            )),
            "must reclaim the store's {sub}:\n{stdout}"
        );
    }
    assert!(!s.store.join("trees").exists());
    assert!(!s.store.join("archives").exists());
    // Preserved: the committed escape hatch, the inputs, the sources, and
    // the build outputs that are the only thing `.noct/` is for now.
    assert!(s.dir.join("vendor/leaf-1.4.0/lib/main.nv").is_file());
    assert_eq!(std::fs::read(s.dir.join("nestpkg.nvpm")).unwrap(), manifest);
    assert_eq!(std::fs::read(s.dir.join("nestpkg.lock")).unwrap(), lock);
    assert!(s.dir.join("main.nv").is_file());
    assert_eq!(std::fs::read(s.dir.join(".noct/build/app.bin")).unwrap(), b"\x00\x01");
    assert!(s.dir.join(".noct").is_dir());

    // And the store is genuinely gone: the project needs a fetch again,
    // while `vendor/` still builds offline with no index at all.
    let out = s.run_in(&s.dir, None, &["run", "main.nv"]);
    assert_eq!(out.status.code(), Some(1), "the store was not really reclaimed");
    s.cleanup();
}
