//! CLI tests for registry `noct add --index` (Phase 4, Slices B+C).
//!
//! Zero network: a programmatic file index (see `registry::FileIndex`
//! for the layout contract) plus a per-test `NOCT_KEYS` directory
//! passed to the child process (never the parent env — tests share
//! one process). Covers resolve→verify→fetch→lock, TOFU + rotation,
//! transitivity, and every fail-closed mode.

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

fn scratch() -> (PathBuf, PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-regcli-{}-{id}", std::process::id()));
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

fn index_arg(root: &Path) -> String {
    root.to_string_lossy().to_string()
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn registry_add_resolves_locks_fetches() {
    let (base, dir, keys) = scratch();
    let root = build_index(
        &base,
        &[("leaf", "1.4.0", &[], &[("lib/main.nv", "hi")])],
    );
    write_app(&dir);
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("new publisher key key:4242"), "TOFU must print, got:\n{stdout}");
    let manifest = std::fs::read_to_string(dir.join("nestpkg.nvpm")).unwrap();
    assert!(manifest.contains("leaf:"), "got:\n{manifest}");
    let lock = std::fs::read_to_string(dir.join("nestpkg.lock")).unwrap();
    assert!(lock.contains("version: 1.4.0"), "got:\n{lock}");
    assert!(lock.contains("signed_by: key:4242"), "got:\n{lock}");
    assert!(lock.contains("tier: native"), "got:\n{lock}");
    assert!(dir.join(".noct/cache/leaf-1.4.0.pkg").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join(".noct/packages/leaf-1.4.0/lib/main.nv")).unwrap(),
        "hi"
    );
    // Idempotent re-run: TOFU silent the second time.
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("new publisher key"));
    cleanup(&base);
}

#[test]
fn registry_add_walks_transitives_and_picks_highest() {
    let (base, dir, keys) = scratch();
    let root = build_index(
        &base,
        &[
            ("leaf", "1.0.0", &[], &[("lib.nv", "a")]),
            ("leaf", "1.4.0", &[], &[("lib.nv", "b")]),
            ("mid", "0.5.0", &[("leaf", "1.0.0", "native")], &[("lib.nv", "m")]),
        ],
    );
    write_app(&dir);
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "mid", "--index", &idx]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let lock = std::fs::read_to_string(dir.join("nestpkg.lock")).unwrap();
    assert!(lock.contains("version: 1.4.0"), "highest satisfying leaf, got:\n{lock}");
    assert!(lock.contains("version: 0.5.0"), "got:\n{lock}");
    cleanup(&base);
}

#[test]
fn registry_add_fails_closed_on_tamper() {
    // Corrupt signature.
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    std::fs::write(root.join("leaf/1.4.0/pkg.sig"), "00".repeat(64) + "\n").unwrap();
    write_app(&dir);
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("signature mismatch"), "got:\n{stderr}");
    assert!(!dir.join("nestpkg.lock").exists(), "failed add writes nothing");

    // Corrupt tarball (hash mismatch).
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    std::fs::write(root.join("leaf/1.4.0/pkg.bin"), b"evil").unwrap();
    write_app(&dir);
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hash mismatch"));
    assert!(!dir.join("nestpkg.lock").exists());
    cleanup(&base);
}

#[test]
fn registry_add_rotation_needs_flag() {
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    write_app(&dir);
    let idx = index_arg(&root);
    assert_eq!(run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]).status.code(), Some(0));
    // Attacker swaps the stored key: fail closed, cite --rotate-key.
    std::fs::write(keys.join(KEY_ID), "ab".repeat(32) + "\n").unwrap();
    std::fs::remove_file(dir.join("nestpkg.lock")).unwrap();
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--rotate-key"));
    // Explicit rotation re-records and succeeds.
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx, "--rotate-key"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    cleanup(&base);
}

#[test]
fn registry_add_rejects_transitive_foreign_runtime() {
    let (base, dir, keys) = scratch();
    let root = build_index(
        &base,
        &[
            ("py_model", "1.0.0", &[], &[("lib.nv", "p")]),
            ("warez", "1.0.0", &[("py_model", "1.0.0", "foreign-runtime")], &[("lib.nv", "w")]),
        ],
    );
    write_app(&dir);
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "warez", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("foreign-runtime"), "got:\n{stderr}");
    assert!(stderr.contains("warez"), "must name the chain, got:\n{stderr}");
    assert!(!dir.join("nestpkg.lock").exists());
    cleanup(&base);
}

#[test]
fn registry_add_conflicts_are_loud() {
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "b")])]);
    write_app(&dir);
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf@=9.9.9", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no indexed version satisfies"));
    let out = run_cli_in_keys(&dir, &keys, &["add", "ghost", "--index", &idx]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown package"));
    cleanup(&base);
}
