//! CLI tests for `noct outdated` (Phase 4).
//!
//! Zero network: a programmatic signed file index (same layout contract
//! as the `registry.rs` harness) plus a per-test `NOCT_KEYS` directory.
//! Asserts the (name, current, latest-satisfying, latest-available)
//! table, the exit-0-even-when-stale rule, and the loud failures.

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
        // Keep the global store out of the developer's machine.
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

fn scratch() -> (PathBuf, PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-outdated-{}-{id}", std::process::id()));
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
fn outdated_reports_newer_major_but_exits_zero() {
    let (base, dir, keys) = scratch();
    let root = build_index(
        &base,
        &[
            ("leaf", "1.0.0", &[], &[("lib.nv", "a")]),
            ("leaf", "1.4.0", &[], &[("lib.nv", "b")]),
            ("leaf", "2.0.0", &[], &[("lib.nv", "c")]),
        ],
    );
    write_app(&dir);
    let idx = root.to_string_lossy().to_string();
    // Caret `1.0.0` locks the highest satisfying 1.x (1.4.0); 2.0.0 is
    // available but out of range.
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf@1.0.0", "--index", &idx]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));

    let out = run_cli_in_keys(&dir, &keys, &["outdated", "--index", &idx]);
    // Informational: exit 0 even though an update exists.
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("name"), "header missing, got:\n{stdout}");
    assert!(stdout.contains("current"), "header missing, got:\n{stdout}");
    assert!(stdout.contains("latest"), "header missing, got:\n{stdout}");
    assert!(stdout.contains("leaf"), "got:\n{stdout}");
    assert!(stdout.contains("1.4.0"), "current + latest-satisfying, got:\n{stdout}");
    assert!(stdout.contains("2.0.0"), "latest-available, got:\n{stdout}");
    cleanup(&base);
}

#[test]
fn outdated_up_to_date_row_and_path_row() {
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "b")])]);
    write_app(&dir);
    let idx = root.to_string_lossy().to_string();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx])
            .status
            .code(),
        Some(0)
    );
    // A path dependency rides along in the lock but has no index row.
    std::fs::create_dir_all(dir.join("sibling")).unwrap();
    std::fs::write(
        dir.join("sibling").join("nestpkg.nvpm"),
        "package:\n    name: sibling\n    version: 0.3.5\n",
    )
    .unwrap();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );

    let out = run_cli_in_keys(&dir, &keys, &["outdated", "--index", &idx]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("leaf"), "got:\n{stdout}");
    assert!(stdout.contains("1.4.0"), "got:\n{stdout}");
    assert!(stdout.contains("sibling"), "got:\n{stdout}");
    assert!(stdout.contains("local"), "path rows are local, got:\n{stdout}");
    cleanup(&base);
}

#[test]
fn outdated_needs_lockfile_and_index_flag() {
    let (base, dir, keys) = scratch();
    write_app(&dir);
    // No lockfile yet: loud error (with a real index dir so the flag
    // check passes and the missing lock is what fails).
    let empty_index = base.join("empty-index");
    std::fs::create_dir_all(&empty_index).unwrap();
    let empty_idx = empty_index.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["outdated", "--index", &empty_idx]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no nestpkg.lock"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Unreadable index dir: loud error too.
    let out = run_cli_in_keys(&dir, &keys, &["outdated", "--index", "somewhere"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot read index"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // No --index flag: loud error naming the flag (no default registry).
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "b")])]);
    let idx = root.to_string_lossy().to_string();
    assert_eq!(
        run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx])
            .status
            .code(),
        Some(0)
    );
    let out = run_cli_in_keys(&dir, &keys, &["outdated"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--index"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&base);
}
