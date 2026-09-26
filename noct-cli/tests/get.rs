//! CLI tests for `noct get` (Phase 1a — the `pub get` analogue).
//!
//! Offline only, following the `vendor.rs` harness: every case builds a
//! signed file index plus a scratch project, establishes a real
//! lockfile with `noct add`, and then drives `noct get` against it.
//! `NOCT_KEYS` and `NOCT_STORE` are pinned inside the scratch dir on
//! every invocation, so the real user key store and the real global
//! content store are never read or written.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 15;
const SEED: [u8; 32] = [0x42; 32];
const KEY_ID: &str = "key:4242";

fn run_cli_in_keys(dir: &Path, keys: &Path, args: &[&str]) -> Output {
    let child = Command::new(noct_bin())
        .args(args)
        .current_dir(dir)
        .env("NOCT_KEYS", keys)
        // The global content store is per-MACHINE; every test gets its
        // own, or the suite would write to (and read from) the
        // developer's real store and tests would not be hermetic.
        .env("NOCT_STORE", dir.parent().unwrap_or(dir).join("store"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn noct binary");
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

/// The store this test's invocations use, derived the same way the
/// harness derives it.
fn store_of(dir: &Path) -> PathBuf {
    dir.parent().unwrap_or(dir).join("store")
}

/// The tree a locked package landed in: read the store's by-name
/// pointer rather than re-deriving the hash, so the test walks the same
/// path the module resolver does.
fn stored_tree(dir: &Path, name: &str, version: &str) -> PathBuf {
    let point = store_of(dir)
        .join("points")
        .join(format!("{name}-{version}.point"));
    let relative = std::fs::read_to_string(&point)
        .unwrap_or_else(|e| panic!("read {}: {e}", point.display()));
    store_of(dir).join(relative.trim())
}

/// How many entries a store subdirectory has; a missing directory is
/// zero, not a panic (a store nothing was ever fetched into does not
/// exist yet).
fn store_entry_count(dir: &Path, sub: &str) -> usize {
    std::fs::read_dir(store_of(dir).join(sub))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// `(base, project, keys)`. Everything lives under one temp dir so
/// cleanup is a single `remove_dir_all`.
fn scratch() -> (PathBuf, PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-get-{}-{id}", std::process::id()));
    let dir = base.join("work");
    let keys = base.join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&keys).unwrap();
    (base, dir, keys)
}

fn cleanup(base: &PathBuf) {
    let _ = std::fs::remove_dir_all(base);
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

/// (name, version, &[(dep-name, req, tier)], &[(path, content)]).
type Pkg<'a> = (
    &'a str,
    &'a str,
    &'a [(&'a str, &'a str, &'a str)],
    &'a [(&'a str, &'a str)],
);

/// Build a signed file index; returns its root. Same shape as the
/// `vendor.rs` fixture, so both suites exercise the same layout.
fn build_index(base: &Path, pkgs: &[Pkg]) -> PathBuf {
    let root = base.join("index");
    let sk = SigningKey::from_bytes(&SEED);
    let pubkey = hex(&sk.verifying_key().to_bytes());
    std::fs::create_dir_all(root.join("_keys")).unwrap();
    std::fs::write(
        root.join("_keys").join(format!("{KEY_ID}.pub")),
        format!("{pubkey}\n"),
    )
    .unwrap();
    for (name, version, deps, files) in pkgs {
        let vdir = root.join(name).join(version);
        std::fs::create_dir_all(&vdir).unwrap();
        let mut manifest = format!("package:\n    name: {name}\n    version: {version}\n");
        if !deps.is_empty() {
            manifest.push_str("dependencies:\n");
            for (dname, req, tier) in *deps {
                if *tier == "native" {
                    manifest.push_str(&format!("    {dname}: {req}\n"));
                } else {
                    let opt = if *tier == "foreign-runtime" {
                        "\n        opt_in: true"
                    } else {
                        ""
                    };
                    manifest.push_str(&format!(
                        "    {dname}:\n        version: {req}\n        tier: {tier}{opt}\n"
                    ));
                }
            }
        }
        std::fs::write(vdir.join("manifest.nvpm"), &manifest).unwrap();
        let mut tarball = Vec::new();
        for (path, content) in *files {
            tarball.extend_from_slice(&(path.len() as u32).to_le_bytes());
            tarball.extend_from_slice(path.as_bytes());
            tarball.extend_from_slice(&(content.len() as u64).to_le_bytes());
            tarball.extend_from_slice(content.as_bytes());
        }
        let hash = sha256_hex(&tarball);
        let payload = format!("noctivue-publish-v1:{name}@{version}:{hash}");
        let sig = hex(&sk.sign(payload.as_bytes()).to_bytes());
        std::fs::write(vdir.join("pkg.bin"), &tarball).unwrap();
        std::fs::write(vdir.join("pkg.hash"), format!("{hash}\n")).unwrap();
        std::fs::write(vdir.join("pkg.sig"), format!("{sig}\n")).unwrap();
        std::fs::write(vdir.join("pkg.key"), format!("{KEY_ID}\n")).unwrap();
    }
    root
}

/// The two-package closure every case starts from: `mid` depends on
/// `leaf` with a caret, so the lock pins `leaf 1.4.0` (the highest
/// satisfying of three indexed versions).
fn closure_index(base: &Path) -> PathBuf {
    build_index(
        base,
        &[
            ("leaf", "1.0.0", &[], &[("lib.nv", "a")]),
            ("leaf", "1.4.0", &[], &[("lib/main.nv", "hi")]),
            ("leaf", "2.0.0", &[], &[("lib.nv", "c")]),
            (
                "mid",
                "0.5.0",
                &[("leaf", "1.0.0", "native")],
                &[("lib.nv", "m")],
            ),
        ],
    )
}

fn write_app(dir: &Path) {
    std::fs::write(
        dir.join("nestpkg.nvpm"),
        "package:\n    name: myapp\n    version: 0.1.0\n",
    )
    .unwrap();
}

/// Establish a real two-package lockfile with `add`, then optionally
/// wipe the store to simulate a clean machine.
fn establish(dir: &Path, keys: &Path, index: &Path, wipe_store: bool) {
    write_app(dir);
    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(dir, keys, &["add", "mid", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "setup add failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if wipe_store {
        std::fs::remove_dir_all(store_of(dir)).unwrap();
    }
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).expect("read back file")
}

/// Clear the read-only flag the store placed on an extracted file, so a
/// test can tamper with it. Read-only is a DETERRENT (see `store.rs`),
/// not a security boundary — an attacker with write access to the store
/// can do exactly this, and the test proves the hash check notices.
fn make_writable(file: &Path) {
    let mut perms = std::fs::metadata(file)
        .unwrap_or_else(|e| panic!("stat {}: {e}", file.display()))
        .permissions();
    assert!(perms.readonly(), "{} should have landed read-only", file.display());
    perms.set_readonly(false);
    std::fs::set_permissions(file, perms)
        .unwrap_or_else(|e| panic!("clear read-only on {}: {e}", file.display()));
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn get_fetches_the_whole_closure_from_a_clean_machine() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, true);
    let lock_before = read(&dir, "nestpkg.lock");
    let manifest_before = read(&dir, "nestpkg.nvpm");
    assert!(
        !store_of(&dir).exists(),
        "setup must leave a clean machine (no store)"
    );

    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The whole closure, not just the direct dep: leaf is transitive.
    assert!(stdout.contains("leaf 1.4.0 fetched"), "got:\n{stdout}");
    assert!(stdout.contains("mid 0.5.0 fetched"), "got:\n{stdout}");
    assert!(
        stdout.contains("get: 2 fetched, 0 verified, 0 skipped (2 locked packages"),
        "got:\n{stdout}"
    );
    // The bytes are GLOBAL now: two halves in the store, keyed by the
    // lock's content hash, and nothing at all in the project tree.
    assert_eq!(read(&stored_tree(&dir, "leaf", "1.4.0"), "lib/main.nv"), "hi");
    assert_eq!(read(&stored_tree(&dir, "mid", "0.5.0"), "lib.nv"), "m");
    assert_eq!(store_entry_count(&dir, "archives"), 2);
    assert_eq!(store_entry_count(&dir, "trees"), 2);
    assert!(
        !dir.join(".noct/cache").exists() && !dir.join(".noct/packages").exists(),
        "fetched content must not land in the project tree"
    );

    // `get` never resolves: the lock and the manifest are byte-identical.
    assert_eq!(read(&dir, "nestpkg.lock"), lock_before);
    assert_eq!(read(&dir, "nestpkg.nvpm"), manifest_before);
    cleanup(&base);
}

#[test]
fn get_second_run_verifies_without_refetching() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, false);
    let archive_before = std::fs::read(
        std::fs::read_dir(store_of(&dir).join("archives"))
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path(),
    )
    .unwrap();

    // Even WITH an index, a good store is not refetched.
    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("leaf 1.4.0 verified"), "got:\n{stdout}");
    assert!(stdout.contains("mid 0.5.0 verified"), "got:\n{stdout}");
    assert!(
        stdout.contains("get: 0 fetched, 2 verified, 0 skipped"),
        "got:\n{stdout}"
    );
    let fetched: Vec<&str> = stdout.lines().filter(|l| l.ends_with(" fetched")).collect();
    assert!(fetched.is_empty(), "must not refetch:\n{stdout}");
    // `fetch_locked` prints on a TOFU record; it must never run here.
    assert!(
        !stdout.contains("new publisher key"),
        "fetch path must not run for a verified closure:\n{stdout}"
    );
    let archive_after = std::fs::read(
        std::fs::read_dir(store_of(&dir).join("archives"))
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert_eq!(archive_after, archive_before, "archive bytes must be untouched");

    // No --index at all: a fully verified closure re-runs offline.
    let out = run_cli_in_keys(&dir, &keys, &["get"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("2 verified"));
    cleanup(&base);
}

#[test]
fn get_skips_path_sources_instead_of_fetching_them() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    write_app(&dir);
    std::fs::create_dir_all(dir.join("sibling")).unwrap();
    std::fs::write(
        dir.join("sibling/nestpkg.nvpm"),
        "package:\n    name: sibling\n    version: 0.3.5\n",
    )
    .unwrap();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    let idx = index.to_string_lossy().to_string();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "mid", "--index", &idx])
            .status
            .code(),
        Some(0)
    );
    let lock_before = read(&dir, "nestpkg.lock");

    // No --index: a path source needs none, and it must be SKIPPED,
    // not fetched (there is no path source in any index anyway).
    let out = run_cli_in_keys(&dir, &keys, &["get"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("sibling 0.3.5 skipped (path source: sibling"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("mid 0.5.0 verified"), "got:\n{stdout}");
    assert!(
        stdout.contains("get: 0 fetched, 2 verified, 1 skipped (3 locked packages"),
        "got:\n{stdout}"
    );
    // A path source never gains a derived copy, anywhere.
    assert!(!store_of(&dir).join("points/sibling-0.3.5.point").exists());
    assert_eq!(store_entry_count(&dir, "archives"), 2);
    assert_eq!(read(&dir, "nestpkg.lock"), lock_before);
    cleanup(&base);
}

#[test]
fn get_refetches_a_tampered_store_tree_loudly() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, false);
    // Tamper with the EXTRACTED TREE — the blocker this store exists to
    // fix. The test has to clear the read-only flag the store placed,
    // which is the honest part of the story: read-only is a deterrent,
    // not a boundary, so a determined local editor CAN write here. The
    // hash is what catches it.
    let tree = stored_tree(&dir, "leaf", "1.4.0");
    make_writable(&tree.join("lib/main.nv"));
    std::fs::write(tree.join("lib/main.nv"), "evil").unwrap();

    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("leaf 1.4.0 local copy rejected"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("extracted tree hash mismatch"),
        "must name the reason:\n{stdout}"
    );
    assert!(
        stdout.contains("refetching"),
        "must announce the repair:\n{stdout}"
    );
    assert!(stdout.contains("leaf 1.4.0 fetched"), "got:\n{stdout}");
    assert!(stdout.contains("mid 0.5.0 verified"), "got:\n{stdout}");
    // The tampered bytes are gone: the tree is replaced by the verified
    // one from the archive, never patched in place.
    assert_eq!(read(&stored_tree(&dir, "leaf", "1.4.0"), "lib/main.nv"), "hi");
    cleanup(&base);
}

#[test]
fn get_fails_loudly_when_the_index_bytes_break_the_lock() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, false);
    // Force a refetch of exactly one package, and make the index serve
    // bytes that no longer match the lock: the fetch must fail closed.
    let tree = stored_tree(&dir, "leaf", "1.4.0");
    make_writable(&tree.join("lib/main.nv"));
    std::fs::write(tree.join("lib/main.nv"), "evil").unwrap();
    std::fs::write(index.join("leaf/1.4.0/pkg.bin"), b"evil").unwrap();

    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("hash mismatch"), "got:\n{stderr}");
    assert!(
        stderr.contains("leaf@1.4.0"),
        "must name the package:\n{stderr}"
    );
    // No half-written replacement: the rejected bytes never became a
    // verified tree.
    assert_eq!(read(&stored_tree(&dir, "leaf", "1.4.0"), "lib/main.nv"), "evil");
    cleanup(&base);
}

#[test]
fn get_refuses_a_rotated_publisher_key_on_refetch() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, false);
    // Same TOFU discipline as `add`: a changed publisher key is a hard
    // failure that points at --rotate-key, never a silent re-record.
    let other = hex(&SigningKey::from_bytes(&[0x77; 32])
        .verifying_key()
        .to_bytes());
    std::fs::write(keys.join(KEY_ID), format!("{other}\n")).unwrap();
    // Force a refetch of exactly one package.
    let tree = stored_tree(&dir, "leaf", "1.4.0");
    make_writable(&tree.join("lib/main.nv"));
    std::fs::write(tree.join("lib/main.nv"), "evil").unwrap();

    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--rotate-key"), "got:\n{stderr}");
    assert!(
        stderr.contains("leaf@1.4.0"),
        "must name the package:\n{stderr}"
    );
    cleanup(&base);
}

#[test]
fn get_refuses_a_lock_entry_with_no_content_hash() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, true);
    // Hand-edit the lock: a registry entry with no `content:` line.
    let lock = read(&dir, "nestpkg.lock");
    let stripped: String = lock
        .lines()
        .filter(|l| !l.trim_start().starts_with("content:"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(
        !stripped.contains("content:"),
        "sanity: the line must be gone"
    );
    std::fs::write(dir.join("nestpkg.lock"), &stripped).unwrap();

    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The lockfile parser is the first line of defence ("needs
    // `content:`"); `get`'s own guard is the second. Either way the
    // requirement is the same: loud, names the entry, never a fetch.
    assert!(stderr.contains("content"), "got:\n{stderr}");
    assert!(stderr.contains("leaf"), "must name the entry:\n{stderr}");
    assert!(
        stderr.contains("nestpkg.lock"),
        "must name the file:\n{stderr}"
    );
    // Never a silent fetch: nothing landed for that package.
    assert!(!store_of(&dir).join("points/leaf-1.4.0.point").exists());
    assert_eq!(store_entry_count(&dir, "trees"), 0);
    cleanup(&base);
}

#[test]
fn get_without_a_lockfile_is_loud() {
    let (base, dir, keys) = scratch();
    write_app(&dir);
    let out = run_cli_in_keys(&dir, &keys, &["get"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nestpkg.lock"),
        "must name the file:\n{stderr}"
    );
    assert!(stderr.contains("noct add"), "must point at add:\n{stderr}");
    cleanup(&base);
}

#[test]
fn get_needs_an_index_exactly_when_something_is_missing() {
    let (base, dir, keys) = scratch();
    let index = closure_index(&base);
    establish(&dir, &keys, &index, true);
    let lock_before = read(&dir, "nestpkg.lock");
    // Clean machine, no --index: loud, and it names the package plus
    // the flag that would fix it.
    let out = run_cli_in_keys(&dir, &keys, &["get"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("leaf@1.4.0"),
        "must name the package:\n{stderr}"
    );
    assert!(stderr.contains("--index"), "must name the flag:\n{stderr}");
    // A failed run leaves the lock byte-for-byte alone.
    assert_eq!(read(&dir, "nestpkg.lock"), lock_before);
    // And an unreachable index is loud too, not a partial success.
    let out = run_cli_in_keys(&dir, &keys, &["get", "--index", "nosuchindex"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read index"));
    cleanup(&base);
}

#[test]
fn get_rejects_bad_flags_and_prints_usage() {
    let (base, dir, keys) = scratch();
    write_app(&dir);
    for (args, needle) in [
        (vec!["get", "--index"], "--index requires a directory"),
        (vec!["get", "--index="], "--index requires a directory"),
        (vec!["get", "--bogus"], "unknown flag"),
        (vec!["get", "extra"], "unknown flag"),
    ] {
        let out = run_cli_in_keys(&dir, &keys, &args);
        assert_eq!(out.status.code(), Some(1), "args: {args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(needle),
            "args {args:?}, got:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = run_cli_in_keys(&dir, &keys, &["get", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("usage: noct get"), "got:\n{stdout}");
    assert!(stdout.contains("nestpkg.lock"), "got:\n{stdout}");
    cleanup(&base);
}
