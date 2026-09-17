//! CLI tests for `noct publish --dry-run` (package manager, item 2).
//!
//! There is no registry and no signature verification (TOOLCHAIN.md
//! §3 hard precondition), so a real publish is a typed refusal that
//! writes nothing. `--dry-run` validates manifest + lockfile locally:
//! parse, presence rules, and lock-current drift. Harness mirrors
//! `add.rs` (scratch temp dirs, `run_cli_in`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 10;

fn run_cli_in(dir: &Path, args: &[&str]) -> Output {
    let child = Command::new(noct_bin())
        .args(args)
        .current_dir(dir)
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

fn run_cli_in_keys(dir: &Path, keys: &Path, args: &[&str]) -> Output {
    let child = Command::new(noct_bin())
        .args(args)
        .current_dir(dir)
        .env("NOCT_KEYS", keys)
        .env_remove("NOCT_SIGNING_KEY")
        .env_remove("NOCT_KEY_ID")
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

fn run_cli_in_unsigned(dir: &Path, args: &[&str]) -> Output {
    // Inherit parent env but force unsigned: a CI machine with a stray
    // NOCT_SIGNING_KEY must not flip these assertions.
    let child = Command::new(noct_bin())
        .args(args)
        .current_dir(dir)
        .env_remove("NOCT_SIGNING_KEY")
        .env_remove("NOCT_KEY_ID")
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

fn scratch_dir() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-publish-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn scratch_with_keys() -> (PathBuf, PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-publish-{}-{id}", std::process::id()));
    let dir = base.join("work");
    let keys = base.join("keys");
    std::fs::create_dir_all(&dir).expect("create work dir");
    std::fs::create_dir_all(&keys).expect("create keys dir");
    (base, dir, keys)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Snapshot every regular file under `root` (relative `/`-paths to
/// bytes). Used to prove `--dry-run` writes nothing to index/cache.
fn snapshot_tree(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if let Ok(bytes) = std::fs::read(&path) {
                    out.insert(rel, bytes);
                }
            }
        }
    }
    out
}

// ── Fixture index helpers (mirrors registry.rs harness) ─────────────────────

const TEST_SEED_HEX: &str = "4242424242424242424242424242424242424242424242424242424242424242";
const TEST_KEY_ID: &str = "key:4242";

fn test_sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut out = String::from("sha256:");
    for b in Sha256::digest(bytes) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn test_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

type TestPkg<'a> =
    (&'a str, &'a str, &'a [(&'a str, &'a str, &'a str)], &'a [(&'a str, &'a str)]);

fn build_test_index(base: &Path, pkgs: &[TestPkg]) -> PathBuf {
    use ed25519_dalek::{Signer, SigningKey};
    let root = base.join("index");
    let seed: [u8; 32] = [0x42; 32];
    let sk = SigningKey::from_bytes(&seed);
    let pubkey = test_hex(&sk.verifying_key().to_bytes());
    std::fs::create_dir_all(root.join("_keys")).unwrap();
    std::fs::write(
        root.join("_keys").join(format!("{TEST_KEY_ID}.pub")),
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
        let hash = test_sha256_hex(&tarball);
        let payload = format!("noctivue-publish-v1:{name}@{version}:{hash}");
        let sig = test_hex(&sk.sign(payload.as_bytes()).to_bytes());
        std::fs::write(vdir.join("pkg.bin"), &tarball).unwrap();
        std::fs::write(vdir.join("pkg.hash"), format!("{hash}\n")).unwrap();
        std::fs::write(vdir.join("pkg.sig"), format!("{sig}\n")).unwrap();
        std::fs::write(vdir.join("pkg.key"), format!("{TEST_KEY_ID}\n")).unwrap();
    }
    root
}

/// Minimal JSON validity check (no new dependencies): parses objects,
/// arrays, strings (with escapes), numbers, true/false/null, and
/// whitespace; errors on trailing content. Panics with a message on
/// invalid JSON — the caller asserts success.
fn assert_valid_json(text: &str) {
    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }
    impl<'a> P<'a> {
        fn ws(&mut self) {
            while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
        }
        fn value(&mut self) {
            self.ws();
            assert!(self.i < self.b.len(), "unexpected end of JSON");
            match self.b[self.i] {
                b'{' => self.object(),
                b'[' => self.array(),
                b'"' => self.string(),
                b't' => self.lit("true"),
                b'f' => self.lit("false"),
                b'n' => self.lit("null"),
                c if c == b'-' || c.is_ascii_digit() => self.number(),
                c => panic!("unexpected JSON byte `{c}` at offset {}", self.i),
            }
            self.ws();
        }
        fn object(&mut self) {
            self.i += 1; // {
            self.ws();
            if self.i < self.b.len() && self.b[self.i] == b'}' {
                self.i += 1;
                return;
            }
            loop {
                self.ws();
                assert!(self.i < self.b.len() && self.b[self.i] == b'"', "object key must be a string");
                self.string();
                self.ws();
                assert!(self.i < self.b.len() && self.b[self.i] == b':', "object needs `:`");
                self.i += 1;
                self.value();
                self.ws();
                assert!(self.i < self.b.len(), "unterminated object");
                if self.b[self.i] == b'}' {
                    self.i += 1;
                    return;
                }
                assert!(self.b[self.i] == b',', "object needs `,` or `}}`");
                self.i += 1;
            }
        }
        fn array(&mut self) {
            self.i += 1; // [
            self.ws();
            if self.i < self.b.len() && self.b[self.i] == b']' {
                self.i += 1;
                return;
            }
            loop {
                self.value();
                self.ws();
                assert!(self.i < self.b.len(), "unterminated array");
                if self.b[self.i] == b']' {
                    self.i += 1;
                    return;
                }
                assert!(self.b[self.i] == b',', "array needs `,` or `]`");
                self.i += 1;
            }
        }
        fn string(&mut self) {
            assert!(self.b[self.i] == b'"');
            self.i += 1;
            while self.i < self.b.len() {
                match self.b[self.i] {
                    b'"' => {
                        self.i += 1;
                        return;
                    }
                    b'\\' => {
                        self.i += 1;
                        assert!(self.i < self.b.len(), "dangling escape in JSON string");
                        match self.b[self.i] {
                            b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                                self.i += 1;
                            }
                            b'u' => {
                                assert!(self.i + 4 < self.b.len(), "bad \\u escape in JSON");
                                for k in 1..=4 {
                                    assert!(
                                        (self.b[self.i + k] as char).is_ascii_hexdigit(),
                                        "bad \\u escape in JSON"
                                    );
                                }
                                self.i += 5;
                            }
                            c => panic!("bad JSON escape `\\{c}`"),
                        }
                    }
                    _ => {
                        self.i += 1;
                    }
                }
            }
            panic!("unterminated JSON string");
        }
        fn lit(&mut self, s: &str) {
            assert!(self.b[self.i..].starts_with(s.as_bytes()), "bad JSON literal near offset {}", self.i);
            self.i += s.len();
        }
        fn number(&mut self) {
            let start = self.i;
            if self.i < self.b.len() && self.b[self.i] == b'-' {
                self.i += 1;
            }
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                self.i += 1;
            }
            if self.i < self.b.len() && self.b[self.i] == b'.' {
                self.i += 1;
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            }
            if self.i < self.b.len() && matches!(self.b[self.i], b'e' | b'E') {
                self.i += 1;
                if self.i < self.b.len() && matches!(self.b[self.i], b'+' | b'-') {
                    self.i += 1;
                }
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            }
            assert!(self.i > start, "bad JSON number");
        }
    }
    let mut p = P { b: text.as_bytes(), i: 0 };
    p.value();
    p.ws();
    assert!(p.i == p.b.len(), "trailing content after JSON value");
}

const APP_MANIFEST: &str = "package:\n    name: myapp\n    version: 0.1.0\n";
const SIBLING_MANIFEST: &str = "package:\n    name: sibling\n    version: 0.3.5\n";

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn publish_dry_run_ready_on_fresh_project() {
    // No deps, no lock: nothing to lock, ready.
    let dir = scratch_dir();
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("manifest ok"), "got:\n{stdout}");
    assert!(stdout.contains("nothing to lock"), "got:\n{stdout}");
    assert!(
        stdout.contains("ready to publish (dry run"),
        "got:\n{stdout}"
    );
    cleanup(&dir);
}

#[test]
fn publish_dry_run_ready_after_add() {
    let dir = scratch_dir();
    std::fs::create_dir_all(dir.join("sibling")).expect("sibling dir");
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), SIBLING_MANIFEST)
        .expect("write sibling");
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lockfile current (1 packages)"),
        "got:\n{stdout}"
    );
    cleanup(&dir);
}

#[test]
fn publish_dry_run_reports_drift_and_missing_lock() {
    let dir = scratch_dir();
    std::fs::create_dir_all(dir.join("sibling")).expect("sibling dir");
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), SIBLING_MANIFEST)
        .expect("write sibling");
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );
    // Hand-edit the requirement past the locked version: drift.
    let edited = std::fs::read_to_string(dir.join("nestpkg.nvpm"))
        .expect("read")
        .replace("=0.3.5", "=9.9.9");
    std::fs::write(dir.join("nestpkg.nvpm"), edited).expect("rewrite");
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("drift"), "got:\n{stderr}");
    assert!(stderr.contains("sibling"), "got:\n{stderr}");

    // Deps but no lock at all: missing lock.
    std::fs::remove_file(dir.join("nestpkg.lock")).expect("drop lock");
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("lockfile missing"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&dir);
}

#[test]
fn publish_without_flag_is_typed_refusal_writing_nothing() {
    let dir = scratch_dir();
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    let out = run_cli_in(&dir, &["publish"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no package registry"), "got:\n{stderr}");
    assert!(
        stderr.contains("signing"),
        "must cite the signing precondition, got:\n{stderr}"
    );
    assert!(
        stderr.contains("--dry-run"),
        "must point at dry-run, got:\n{stderr}"
    );
    assert!(
        !dir.join("nestpkg.lock").exists(),
        "refusal must write nothing"
    );
    cleanup(&dir);
}

#[test]
fn publish_dry_run_needs_a_manifest() {
    let dir = scratch_dir();
    let out = run_cli_in(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no nestpkg.nvpm"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&dir);
}

// ── Publish-integrity additions ─────────────────────────────────────────────

#[test]
fn publish_dry_run_publishes_nothing_byte_identical() {
    // Full pipeline (manifest parse, tarball assembly, hash, signature
    // check) must not write to the index or the project cache.
    let (base, dir, keys) = scratch_with_keys();
    let root = build_test_index(&base, &[("leaf", "1.4.0", &[], &[("lib.nv", "hi")])]);
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    let idx = root.to_string_lossy().to_string();
    let out = run_cli_in_keys(&dir, &keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "add failed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let before_index = snapshot_tree(&root);
    let before_noct = snapshot_tree(&dir.join(".noct"));
    let before_manifest = std::fs::read(dir.join("nestpkg.nvpm")).expect("read manifest");
    let before_lock = std::fs::read(dir.join("nestpkg.lock")).expect("read lock");
    assert!(!before_index.is_empty(), "index snapshot must be non-empty");
    assert!(!before_noct.is_empty(), "cache snapshot must be non-empty");

    let out = run_cli_in_keys(&dir, &keys, &["publish", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "dry-run failed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // What WOULD be published: name, version, tier, hash, signer state.
    assert!(stdout.contains("would publish"), "got:\n{stdout}");
    assert!(stdout.contains("myapp"), "got:\n{stdout}");
    assert!(stdout.contains("0.1.0"), "got:\n{stdout}");
    assert!(stdout.contains("tier native"), "got:\n{stdout}");
    assert!(stdout.contains("sha256:"), "got:\n{stdout}");
    assert!(
        stdout.contains("UNSIGNED"),
        "native unsigned dry-run must warn, got:\n{stdout}"
    );
    assert!(
        stdout.contains("ready to publish (dry run"),
        "got:\n{stdout}"
    );

    let after_index = snapshot_tree(&root);
    let after_noct = snapshot_tree(&dir.join(".noct"));
    let after_manifest = std::fs::read(dir.join("nestpkg.nvpm")).expect("re-read manifest");
    let after_lock = std::fs::read(dir.join("nestpkg.lock")).expect("re-read lock");
    assert_eq!(before_index, after_index, "dry-run mutated the index");
    assert_eq!(before_noct, after_noct, "dry-run mutated .noct/cache");
    assert_eq!(before_manifest, after_manifest, "dry-run mutated the manifest");
    assert_eq!(before_lock, after_lock, "dry-run mutated the lockfile");
    cleanup(&base);
}

#[test]
fn publish_dry_run_refuses_unsigned_non_native_tier() {
    // Non-native tiers vouch for foreign code: an unsigned dry-run is a
    // clean refusal (non-zero + named reason), never a silent publish.
    let dir = scratch_dir();
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");

    let out = run_cli_in_unsigned(&dir, &["publish", "--dry-run", "--tier", "c-shim"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "unsigned c-shim dry-run must fail. stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unsigned"), "must say unsigned, got:\n{stderr}");
    assert!(stderr.contains("c-shim"), "must name the tier, got:\n{stderr}");
    assert!(
        stderr.contains("NOCT_SIGNING_KEY"),
        "must say how to sign, got:\n{stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("ready to publish"),
        "refusal must not claim readiness"
    );
    assert!(
        !dir.join(".noct").exists(),
        "refused dry-run must not create .noct"
    );

    // Same tier WITH a signing key succeeds and names the signer.
    let out = {
        let child = Command::new(noct_bin())
            .args(["publish", "--dry-run", "--tier", "c-shim"])
            .current_dir(&dir)
            .env("NOCT_SIGNING_KEY", TEST_SEED_HEX)
            .env("NOCT_KEY_ID", "key:test")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn noct binary");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(CASE_TIMEOUT_SECS)) {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => panic!("failed waiting on noct binary: {e}"),
            Err(_) => panic!("TIMEOUT: `noct publish --dry-run --tier c-shim` hung"),
        }
    };
    assert_eq!(
        out.status.code(),
        Some(0),
        "signed c-shim dry-run must succeed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("would publish"), "got:\n{stdout}");
    assert!(stdout.contains("tier c-shim"), "got:\n{stdout}");
    assert!(stdout.contains("signed_by key:test"), "got:\n{stdout}");

    // Native unsigned stays a warning-success (existing fresh-project
    // behavior), and a garbage tier is a clean error, never a panic.
    let out = run_cli_in_unsigned(&dir, &["publish", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("UNSIGNED"),
        "native unsigned must warn"
    );
    let out = run_cli_in_unsigned(&dir, &["publish", "--dry-run", "--tier", "water"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid tier"), "got:\n{stderr}");
    assert!(!stderr.contains("panicked"), "must not panic, got:\n{stderr}");
    cleanup(&dir);
}

#[test]
fn publish_sbom_contains_root_and_path_dep_with_correct_hashes() {
    // SBOM covers the closure: root (computed tarball hash) plus every
    // locked dependency (path entries carry null hash/signer by design).
    let dir = scratch_dir();
    std::fs::create_dir_all(dir.join("sibling")).expect("sibling dir");
    std::fs::write(dir.join("nestpkg.nvpm"), APP_MANIFEST).expect("write manifest");
    std::fs::write(dir.join("sibling").join("nestpkg.nvpm"), SIBLING_MANIFEST)
        .expect("write sibling");
    assert_eq!(
        run_cli_in(&dir, &["add", "sibling", "--path", "sibling"])
            .status
            .code(),
        Some(0)
    );

    // Explicit `--dry-run --sbom` form.
    let out = run_cli_in_unsigned(&dir, &["publish", "--dry-run", "--sbom", "sbom.json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "sbom dry-run failed. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sbom_path = dir.join("sbom.json");
    assert!(sbom_path.is_file(), "SBOM file must be written");
    let text = std::fs::read_to_string(&sbom_path).expect("read sbom");
    assert_valid_json(&text);
    assert!(text.contains("\"root\""), "SBOM must have root, got:\n{text}");
    assert!(text.contains("\"packages\""), "SBOM must have packages, got:\n{text}");
    assert!(text.contains("\"myapp\""), "SBOM must name the root, got:\n{text}");
    assert!(text.contains("\"0.1.0\""), "SBOM must version the root, got:\n{text}");
    assert!(text.contains("\"sibling\""), "SBOM must list the path dep, got:\n{text}");
    assert!(text.contains("\"0.3.5\""), "SBOM must version the path dep, got:\n{text}");

    // Root hash correctness: with no *.nv sources the tarball is exactly
    // pack([("nestpkg.nvpm", manifest-bytes)]); recompute and compare to
    // the SBOM's root content_hash.
    let manifest_bytes = std::fs::read(dir.join("nestpkg.nvpm")).expect("read manifest");
    let mut tarball = Vec::new();
    let path = "nestpkg.nvpm";
    tarball.extend_from_slice(&(path.len() as u32).to_le_bytes());
    tarball.extend_from_slice(path.as_bytes());
    tarball.extend_from_slice(&(manifest_bytes.len() as u64).to_le_bytes());
    tarball.extend_from_slice(&manifest_bytes);
    let expected = test_sha256_hex(&tarball);
    assert!(
        text.contains(&expected),
        "SBOM root hash must equal recomputed tarball hash {expected}, got:\n{text}"
    );
    // Path dep correctness: null hash + null signer (nothing to verify).
    let sib_idx = text.find("\"sibling\"").expect("sibling entry");
    let sib_slice = &text[sib_idx..];
    assert!(
        sib_slice.contains("\"content_hash\": null"),
        "path dep must carry null content_hash, got:\n{sib_slice}"
    );
    assert!(
        sib_slice.contains("\"signed_by\": null"),
        "path dep must carry null signed_by, got:\n{sib_slice}"
    );

    // Bare `--sbom` (no --dry-run) implies a dry run and behaves the same.
    let out = run_cli_in_unsigned(&dir, &["publish", "--sbom", "sbom2.json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "bare --sbom must imply dry-run. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("sbom2.json").is_file());
    let text2 = std::fs::read_to_string(dir.join("sbom2.json")).expect("read sbom2");
    assert_valid_json(&text2);
    assert!(text2.contains(&expected), "second SBOM must carry the same root hash");
    cleanup(&dir);
}
