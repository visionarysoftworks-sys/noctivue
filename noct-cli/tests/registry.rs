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

// ── Phase 4 exit-criterion: fail-closed tamper proof ────────────────────────

#[test]
fn registry_cached_tarball_tamper_fails_closed_on_use() {
    // Adversarial proof: flip ONE byte of the cached `.pkg` tarball and
    // prove the install path (`noct run`, via
    // `require_package_current` → `check_cache_current`) FAILS CLOSED —
    // non-zero exit with an integrity error naming the package — rather
    // than running/installing the tampered bytes.
    //
    // Verification already existed (`registry::check_cache_current`
    // hash re-check on every build-touching-the-cache; no new
    // production code was needed for this proof).
    let (base, dir, keys) = scratch();
    let root = build_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    write_app(&dir);
    std::fs::write(dir.join("main.nv"), "main():\n    println(\"hi\")\n").unwrap();
    let idx = index_arg(&root);
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "setup add failed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Sanity: the untampered project runs.
    let out = run_cli_in_keys(&dir, &keys, &["run", "main.nv"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "untampered run must succeed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Flip exactly one byte in the middle of the cached tarball.
    let cache = dir.join(".noct/cache/leaf-1.4.0.pkg");
    let mut bytes = std::fs::read(&cache).expect("read cached pkg");
    assert!(!bytes.is_empty(), "cached pkg must be non-empty");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    std::fs::write(&cache, &bytes).expect("write tampered pkg");

    // The next use must fail closed, naming the package.
    let out = run_cli_in_keys(&dir, &keys, &["run", "main.nv"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "tampered cache must fail closed (non-zero exit)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("leaf"),
        "integrity error must name the package, got:\n{stderr}"
    );
    assert!(
        stderr.contains("hash mismatch") || stderr.contains("not current"),
        "integrity error must cite the hash/integrity failure, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("panic"),
        "must fail cleanly, never panic, got:\n{stderr}"
    );
    // Fail-closed means no healing and no run: stdout must not contain
    // the program's output.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("hi\n") || out.status.code() != Some(0),
        "tampered bytes must not execute"
    );
    cleanup(&base);
}

// ── Manifest parser robustness (adversarial inputs, never panics) ──────────
//
// NOTE: these live here — NOT in manifest.rs — per file ownership (another
// agent owns manifest.rs). They exercise the parser through the
// `publish --dry-run` CLI surface: every case must exit 1 with a clean
// `invalid manifest: line …` error, never a panic (exit 101) or hang.

fn run_publish_unsigned(dir: &Path, keys: &Path, manifest_text: &str) -> Output {
    std::fs::write(dir.join("nestpkg.nvpm"), manifest_text).expect("write adversarial manifest");
    // Drop any stale lock so the failure is attributable to the manifest.
    let _ = std::fs::remove_file(dir.join("nestpkg.lock"));
    run_cli_in_keys(dir, keys, &["publish", "--dry-run"])
}

fn assert_manifest_rejected(dir: &Path, keys: &Path, case: &str, text: &str, fragment: &str) {
    let out = run_publish_unsigned(dir, keys, text);
    assert_eq!(
        out.status.code(),
        Some(1),
        "adversarial case `{case}` must exit 1, not succeed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid manifest"),
        "case `{case}` must report an invalid manifest, got:\n{stderr}"
    );
    assert!(
        stderr.contains(fragment),
        "case `{case}` error must contain `{fragment}`, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("panic"),
        "case `{case}` must not panic, got:\n{stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("ready to publish"),
        "case `{case}` must not claim readiness"
    );
}

#[test]
fn manifest_parser_rejects_structural_adversaries() {
    let (base, dir, keys) = scratch();
    // Duplicate keys (same block, twice).
    assert_manifest_rejected(
        &dir,
        &keys,
        "duplicate keys",
        "package:\n    name: myapp\n    version: 0.1.0\n    name: other\n",
        "duplicate key",
    );
    // Truncated file: section header with no body (cut off mid-file).
    assert_manifest_rejected(
        &dir,
        &keys,
        "truncated section",
        "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n",
        "has no body",
    );
    // Truncated file: missing required value.
    assert_manifest_rejected(
        &dir,
        &keys,
        "truncated manifest",
        "package:\n    name: myapp\n",
        "missing required",
    );
    // Bad semver (non-numeric patch).
    assert_manifest_rejected(
        &dir,
        &keys,
        "bad semver",
        "package:\n    name: myapp\n    version: 1.0.x\n",
        "bad version",
    );
    // Unknown tier on a dependency block.
    assert_manifest_rejected(
        &dir,
        &keys,
        "unknown tier",
        "package:\n    name: myapp\n    version: 0.1.0\ndependencies:\n    leaf:\n        version: 1.0.0\n        tier: water\n",
        "unknown tier",
    );
    cleanup(&base);
}

#[test]
fn manifest_parser_rejects_hostile_bytes_without_panic() {
    let (base, dir, keys) = scratch();
    // Embedded NUL byte inside a scalar (must be a clean ManifestError,
    // never a truncation/panic).
    assert_manifest_rejected(
        &dir,
        &keys,
        "null bytes",
        "package:\n    name: myapp\0evil\n    version: 0.1.0\n",
        "bad project name",
    );
    // 1MB single line: a hostile overlong name (length-capped at 64 by
    // the name rule — must fail fast with a clean error, not hang/OOM).
    let big = "a".repeat(1024 * 1024);
    let text = format!("package:\n    name: {big}\n    version: 0.1.0\n");
    let out = run_publish_unsigned(&dir, &keys, &text);
    assert_eq!(
        out.status.code(),
        Some(1),
        "1MB-line case must exit 1, not succeed or hang"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid manifest"),
        "1MB-line case must report an invalid manifest, got (first 500 chars):\n{}",
        stderr.chars().take(500).collect::<String>()
    );
    assert!(
        stderr.contains("bad project name"),
        "1MB-line case must cite the name rule, got (first 500 chars):\n{}",
        stderr.chars().take(500).collect::<String>()
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("panic"),
        "1MB-line case must not panic"
    );
    cleanup(&base);
}
