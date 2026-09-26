//! CLI tests for `noct vendor` (Phase 4).
//!
//! Offline only: every case builds a scratch project, populates the
//! global content store + lock via `add` (path mode or a programmatic
//! signed file index, following the `registry.rs` harness), runs
//! `vendor`, and asserts on exit codes plus exact `vendor/` tree bytes.
//! `NOCT_KEYS` and `NOCT_STORE` are pinned inside the scratch dir, so
//! the real user key store and the real global store are untouched.

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
        .env("NOCT_STORE", store_of(dir))
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

fn store_of(dir: &Path) -> PathBuf {
    dir.parent().unwrap_or(dir).join("store")
}

/// The store's tree for one locked package, via its by-name pointer.
fn stored_tree(dir: &Path, name: &str, version: &str) -> PathBuf {
    let point = store_of(dir)
        .join("points")
        .join(format!("{name}-{version}.point"));
    let relative = std::fs::read_to_string(&point)
        .unwrap_or_else(|e| panic!("read {}: {e}", point.display()));
    store_of(dir).join(relative.trim())
}

/// Drop a locked package's whole store entry (what `noct clean` does to
/// one package): archive, tree, record and pointer.
fn evict(dir: &Path, name: &str, version: &str) {
    let store = store_of(dir);
    let tree = stored_tree(dir, name, version);
    let hex = tree
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for stale in [tree.clone(), store.join("archives").join(format!("{hex}.pkg"))] {
        make_writable_recursive(&stale);
        if stale.is_dir() {
            std::fs::remove_dir_all(&stale).unwrap();
        } else if stale.exists() {
            std::fs::remove_file(&stale).unwrap();
        }
    }
    std::fs::remove_file(store.join("index").join(format!("{hex}.record"))).ok();
    std::fs::remove_file(
        store
            .join("points")
            .join(format!("{name}-{version}.point")),
    )
    .ok();
}

/// Clear read-only flags the store placed (they are a deterrent, not a
/// boundary — a test has to be able to play the role of a local editor
/// to prove the hash catches it).
fn make_writable_recursive(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable_recursive(&entry.path());
            }
        }
    }
    let mut perms = meta.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn scratch() -> (PathBuf, PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-vendor-{}-{id}", std::process::id()));
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
type Pkg<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str, &'a str)], &'a [(&'a str, &'a str)]);

/// Build a signed file index; returns its root.
fn build_index(base: &Path, pkgs: &[Pkg]) -> PathBuf {
    let root = base.join("index");
    let sk = SigningKey::from_bytes(&SEED);
    let pubkey = hex(&sk.verifying_key().to_bytes());
    std::fs::create_dir_all(root.join("_keys")).unwrap();
    std::fs::write(root.join("_keys").join(format!("{KEY_ID}.pub")), format!("{pubkey}\n")).unwrap();
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
                    let opt = if *tier == "foreign-runtime" { "\n        opt_in: true" } else { "" };
                    manifest.push_str(&format!("    {dname}:\n        version: {req}\n        tier: {tier}{opt}\n"));
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

fn write_app(dir: &Path) {
    std::fs::write(dir.join("nestpkg.nvpm"), "package:\n    name: myapp\n    version: 0.1.0\n").unwrap();
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn vendor_copies_registry_closure_from_the_store() {
    let (base, dir, keys) = scratch();
    let root = build_index(
        &base,
        &[
            ("leaf", "1.0.0", &[], &[("lib.nv", "a")]),
            ("leaf", "1.4.0", &[], &[("lib/main.nv", "hi")]),
            ("mid", "0.5.0", &[("leaf", "1.0.0", "native")], &[("lib.nv", "m")]),
        ],
    );
    write_app(&dir);
    let idx = root.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["add", "mid", "--index", &idx]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));

    let out = run_cli_in_keys(&dir, &keys, &["vendor"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Full closure: direct + transitive, highest satisfying leaf.
    assert!(stdout.contains("vendored mid 0.5.0"), "got:\n{stdout}");
    assert!(stdout.contains("vendored leaf 1.4.0"), "got:\n{stdout}");
    assert_eq!(
        std::fs::read_to_string(dir.join("vendor/mid-0.5.0/lib.nv")).unwrap(),
        "m"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("vendor/leaf-1.4.0/lib/main.nv")).unwrap(),
        "hi"
    );

    // Idempotent re-run: same bytes, exit 0.
    let out = run_cli_in_keys(&dir, &keys, &["vendor"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(dir.join("vendor/leaf-1.4.0/lib/main.nv")).unwrap(),
        "hi"
    );
    cleanup(&base);
}

#[test]
fn vendor_custom_dir_and_path_dep() {
    let (base, dir, keys) = scratch();
    std::fs::write(dir.join("nestpkg.nvpm"), "package:\n    name: myapp\n    version: 0.1.0\n").unwrap();
    std::fs::create_dir_all(dir.join("sibling")).unwrap();
    std::fs::write(
        dir.join("sibling").join("nestpkg.nvpm"),
        "package:\n    name: sibling\n    version: 0.3.5\n",
    )
    .unwrap();
    std::fs::write(dir.join("sibling").join("lib.nv"), "sib").unwrap();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );

    let out = run_cli_in_keys(&dir, &keys, &["vendor", "--dir", "third_party"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("vendored sibling 0.3.5"), "got:\n{stdout}");
    assert!(stdout.contains("third_party"), "must name the override dir, got:\n{stdout}");
    assert_eq!(
        std::fs::read_to_string(dir.join("third_party/sibling-0.3.5/lib.nv")).unwrap(),
        "sib"
    );
    assert!(!dir.join("vendor").exists(), "default dir must stay untouched");
    cleanup(&base);
}

#[test]
fn vendor_missing_bytes_is_loud() {
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    write_app(&dir);
    let idx = root.to_string_lossy().to_string();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx])
            .status
            .code(),
        Some(0)
    );
    // Evict the whole store entry (what `noct clean` does machine-wide).
    evict(&dir, "leaf", "1.4.0");

    let out = run_cli_in_keys(&dir, &keys, &["vendor"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no local bytes"), "got:\n{stderr}");
    assert!(stderr.contains("leaf@1.4.0"), "must name the package, got:\n{stderr}");
    cleanup(&base);
}

#[test]
fn vendor_never_vendors_bytes_the_lock_does_not_name() {
    // Fail-closed (P-003 §6) in the store model, and it is a STRONGER
    // statement than "it errors": tampered bytes never reach `vendor/`
    // in any form.
    //
    // 1. A tampered extracted tree: the tree fails its hash, so
    //    `vendor` re-unpacks the verified archive instead and says so.
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    write_app(&dir);
    let idx = root.to_string_lossy().to_string();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx])
            .status
            .code(),
        Some(0)
    );
    let tree_file = stored_tree(&dir, "leaf", "1.4.0").join("lib.nv");
    make_writable_recursive(&tree_file);
    std::fs::write(&tree_file, "evil").unwrap();

    let out = run_cli_in_keys(&dir, &keys, &["vendor"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a drifted tree is repaired from the verified archive, not fatal:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("not usable"), "must announce the repair:\n{stdout}");
    assert!(
        stdout.contains("vendored leaf 1.4.0"),
        "got:\n{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("vendor/leaf-1.4.0/lib.nv")).unwrap(),
        "hi",
        "the verified bytes, never the edited ones"
    );
    cleanup(&base);

    // 2. A tampered archive with no usable tree: reading it re-hashes it
    //    against the lock, so there is nothing left to vendor and the
    //    command fails closed naming the mismatch.
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    write_app(&dir);
    let idx = root.to_string_lossy().to_string();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx])
            .status
            .code(),
        Some(0)
    );
    let tree = stored_tree(&dir, "leaf", "1.4.0");
    let hex = tree.file_name().unwrap().to_string_lossy().into_owned();
    make_writable_recursive(&tree);
    std::fs::remove_dir_all(&tree).unwrap();
    let archive = store_of(&dir).join("archives").join(format!("{hex}.pkg"));
    make_writable_recursive(&archive);
    std::fs::write(&archive, b"evil").unwrap();

    let out = run_cli_in_keys(&dir, &keys, &["vendor"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("hash mismatch"), "got:\n{stderr}");
    assert!(stderr.contains("leaf@1.4.0"), "must name the package, got:\n{stderr}");
    assert!(
        !dir.join("vendor/leaf-1.4.0/lib.nv").exists(),
        "tampered bytes must never land in vendor/"
    );
    cleanup(&base);
}

#[test]
fn vendor_needs_a_lockfile_and_rejects_bad_flags() {
    let (base, dir, keys) = scratch();
    write_app(&dir);
    let out = run_cli_in_keys(&dir, &keys, &["vendor"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no nestpkg.lock"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_cli_in_keys(&dir, &keys, &["vendor", "--bogus"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unknown flag"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&base);
}
