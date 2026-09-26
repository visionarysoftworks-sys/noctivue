//! CLI tests for `noct clean` (Phase 1a + the global content store).
//!
//! The command is only interesting for what it does NOT delete, so
//! most of these cases assert the preserved set rather than the removed
//! one. `NOCT_KEYS` and `NOCT_STORE` are pinned inside the scratch dir
//! on every invocation, so the real user key store and the real global
//! content store are never touched.

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

/// A store path as `noct clean` prints it: `/`-separated, so the
/// assertions compare against the message, not against `Path::display`.
fn store_path(dir: &Path, sub: &str) -> String {
    store_of(dir)
        .join(sub)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `(base, project, keys)`.
fn scratch() -> (PathBuf, PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("noctivue-clean-{}-{id}", std::process::id()));
    let dir = base.join("work");
    let keys = base.join("keys");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&keys).unwrap();
    (base, dir, keys)
}

fn cleanup(base: &PathBuf) {
    let _ = std::fs::remove_dir_all(base);
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
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

/// Every file under `root` as `(relative path, bytes)`, sorted — the
/// byte-identity witness for "clean changed nothing".
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

/// One signed `leaf 1.4.0` in a file index; returns its root.
fn build_index(base: &Path) -> PathBuf {
    let root = base.join("index");
    let sk = SigningKey::from_bytes(&SEED);
    std::fs::create_dir_all(root.join("_keys")).unwrap();
    std::fs::write(
        root.join("_keys").join(format!("{KEY_ID}.pub")),
        format!("{}\n", hex(&sk.verifying_key().to_bytes())),
    )
    .unwrap();
    let vdir = root.join("leaf").join("1.4.0");
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(
        vdir.join("manifest.nvpm"),
        "package:\n    name: leaf\n    version: 1.4.0\n",
    )
    .unwrap();
    let bytes = tarball(&[("lib/main.nv", "hi")]);
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
    root
}

/// A project with a real manifest + lock, a real fetched dependency, a
/// committed `vendor/` tree, and a `path`-source sibling OUTSIDE the
/// project root. Returns the byte snapshots `clean` must not disturb.
struct Seeded {
    manifest: String,
    lock: String,
    vendor: Vec<(String, Vec<u8>)>,
    outside: Vec<(String, Vec<u8>)>,
}

fn seed_full(base: &Path, dir: &Path, keys: &Path) -> Seeded {
    let index = build_index(base);
    std::fs::write(
        dir.join("nestpkg.nvpm"),
        "package:\n    name: myapp\n    version: 0.1.0\n",
    )
    .unwrap();
    let idx = index.to_string_lossy().to_string();
    let out = run_cli_in_keys(dir, keys, &["add", "leaf", "--index", &idx]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "setup add failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A path dependency that is not even inside the project root.
    let outside = base.join("other").join("sibling");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        outside.join("nestpkg.nvpm"),
        "package:\n    name: sibling\n    version: 0.3.5\n",
    )
    .unwrap();
    std::fs::write(outside.join("lib.nv"), "sib").unwrap();
    let out = run_cli_in_keys(dir, keys, &["add", "sibling", "--path", "../other/sibling"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "setup add --path failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The committed offline escape hatch.
    let out = run_cli_in_keys(dir, keys, &["vendor"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "setup vendor failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    Seeded {
        manifest: read(dir, "nestpkg.nvpm"),
        lock: read(dir, "nestpkg.lock"),
        vendor: snapshot(&dir.join("vendor")),
        outside: snapshot(&outside),
    }
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn clean_reclaims_the_store_and_preserves_everything_else() {
    let (base, dir, keys) = scratch();
    let seeded = seed_full(&base, &dir, &keys);
    // Fetched content is global now: the store holds both halves plus
    // the record and the pointer.
    let store = store_of(&dir);
    assert!(store.join("archives").is_dir());
    assert!(store.join("trees").is_dir());
    assert!(store.join("index").is_dir());
    assert!(store.join("points").is_dir());
    assert!(stored_tree(&dir, "leaf", "1.4.0").join("lib/main.nv").is_file());
    // A legacy in-tree pair from before the move, which `clean` also
    // reclaims (and never reads).
    std::fs::create_dir_all(dir.join(".noct/cache")).unwrap();
    std::fs::create_dir_all(dir.join(".noct/packages/leaf-1.4.0/lib")).unwrap();
    std::fs::write(dir.join(".noct/cache/leaf-1.4.0.pkg"), b"old").unwrap();
    std::fs::write(dir.join(".noct/packages/leaf-1.4.0/lib/main.nv"), "old").unwrap();

    let out = run_cli_in_keys(&dir, &keys, &["clean"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The store is the primary target, and each line says whose bytes
    // they are: reclaiming it affects every project on the machine.
    for sub in ["archives", "trees", "index", "points"] {
        assert!(
            stdout.contains(&format!("removed {}", store_path(&dir, sub))),
            "must reclaim the store's {sub}, got:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("the global store, shared by every project"),
        "must say the scope is machine-wide:\n{stdout}"
    );
    assert!(stdout.contains("removed .noct/cache"), "got:\n{stdout}");
    assert!(stdout.contains("removed .noct/packages"), "got:\n{stdout}");
    assert!(stdout.contains("reclaimed"), "got:\n{stdout}");
    assert!(
        stdout.contains("preserved:"),
        "must say what survived:\n{stdout}"
    );
    assert!(stdout.contains("vendor/"), "must name vendor/:\n{stdout}");

    // Derived data gone...
    assert!(!store.join("archives").exists());
    assert!(!store.join("trees").exists());
    assert!(!store.join("index").exists());
    assert!(!store.join("points").exists());
    assert!(!dir.join(".noct/cache").exists());
    assert!(!dir.join(".noct/packages").exists());
    // ...but `.noct/` itself kept, and the inputs byte-identical.
    assert!(dir.join(".noct").is_dir(), ".noct/ itself must survive");
    assert_eq!(read(&dir, "nestpkg.nvpm"), seeded.manifest);
    assert_eq!(read(&dir, "nestpkg.lock"), seeded.lock);
    assert_eq!(snapshot(&dir.join("vendor")), seeded.vendor);
    assert_eq!(
        snapshot(&base.join("other").join("sibling")),
        seeded.outside
    );

    // No temp trash left behind by the rename-then-delete dance.
    let strays: Vec<String> = std::fs::read_dir(dir.join(".noct"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".noct-clean-"))
        .collect();
    assert!(strays.is_empty(), "trash left behind: {strays:?}");
    cleanup(&base);
}

#[test]
fn clean_dry_run_changes_nothing() {
    let (base, dir, keys) = scratch();
    seed_full(&base, &dir, &keys);
    let before = snapshot(&store_of(&dir));
    assert!(!before.is_empty());

    let out = run_cli_in_keys(&dir, &keys, &["clean", "--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("dry-run: would remove {}", store_path(&dir, "trees"))),
        "got:\n{stdout}"
    );
    // A store-based project has no legacy in-tree trees at all, and the
    // report says so rather than staying silent about them.
    assert!(
        stdout.contains("dry-run: .noct/packages absent"),
        "got:\n{stdout}"
    );
    assert!(stdout.contains("file"), "must report counts:\n{stdout}");
    assert!(stdout.contains("nothing was changed"), "got:\n{stdout}");
    assert!(stdout.contains("preserved:"), "got:\n{stdout}");

    assert_eq!(snapshot(&store_of(&dir)), before, "dry-run must not write");
    assert!(stored_tree(&dir, "leaf", "1.4.0").join("lib/main.nv").is_file());
    cleanup(&base);
}

#[test]
fn clean_with_nothing_to_clean_is_not_an_error() {
    let (base, dir, keys) = scratch();
    // A bare directory.
    let out = run_cli_in_keys(&dir, &keys, &["clean"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("nothing to clean"), "got:\n{stdout}");
    assert!(stdout.contains("preserved:"), "got:\n{stdout}");
    // Running it twice must stay a success (idempotent by construction).
    let out = run_cli_in_keys(&dir, &keys, &["clean"]);
    assert_eq!(out.status.code(), Some(0));
    let out = run_cli_in_keys(&dir, &keys, &["clean", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("absent"));

    // A project with inputs but no derived store at all: the inputs
    // stay, and it is still not an error.
    std::fs::write(
        dir.join("nestpkg.nvpm"),
        "package:\n    name: myapp\n    version: 0.1.0\n",
    )
    .unwrap();
    std::fs::write(dir.join("nestpkg.lock"), "lock_version: 1\npackages:\n").unwrap();
    std::fs::create_dir_all(dir.join("vendor/leaf-1.4.0/lib")).unwrap();
    std::fs::write(dir.join("vendor/leaf-1.4.0/lib/main.nv"), "hi").unwrap();
    let before = snapshot(&dir);
    let out = run_cli_in_keys(&dir, &keys, &["clean"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("nothing to clean"));
    assert_eq!(
        snapshot(&dir),
        before,
        "nothing to clean must change nothing"
    );
    cleanup(&base);
}

#[test]
fn clean_preserves_other_noct_content() {
    let (base, dir, keys) = scratch();
    seed_full(&base, &dir, &keys);
    // `.noct/build` is build output, not dependency data (TOOLCHAIN.md
    // §3 keeps it in-tree), and is out of `clean`'s scope by design.
    std::fs::create_dir_all(dir.join(".noct/build")).unwrap();
    std::fs::write(dir.join(".noct/build/app.bin"), b"\x00\x01").unwrap();

    let out = run_cli_in_keys(&dir, &keys, &["clean"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!store_of(&dir).join("trees").exists());
    assert!(!dir.join(".noct/cache").exists());
    assert!(!dir.join(".noct/packages").exists());
    assert_eq!(read(&dir, ".noct/build/app.bin"), "\u{0}\u{1}");
    cleanup(&base);
}

#[test]
fn clean_removes_a_manually_built_layout() {
    let (base, dir, keys) = scratch();
    // No `add`: prove the target list is the layout, not a package
    // manager artifact. The legacy in-tree pair, built by hand.
    std::fs::create_dir_all(dir.join(".noct/cache")).unwrap();
    std::fs::create_dir_all(dir.join(".noct/packages/thing-1.0.0/lib")).unwrap();
    std::fs::write(dir.join(".noct/cache/thing-1.0.0.pkg"), b"bytes").unwrap();
    std::fs::write(dir.join(".noct/packages/thing-1.0.0/lib/main.nv"), "src").unwrap();

    let out = run_cli_in_keys(&dir, &keys, &["clean", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("2 files"),
        "must count both halves:\n{stdout}"
    );
    assert!(stdout.contains("8 B"), "must size the tree:\n{stdout}");
    assert!(dir.join(".noct/cache/thing-1.0.0.pkg").is_file());

    let out = run_cli_in_keys(&dir, &keys, &["clean"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!dir.join(".noct/cache").exists());
    assert!(!dir.join(".noct/packages").exists());
    assert!(dir.join(".noct").is_dir());
    cleanup(&base);
}

#[test]
fn clean_rejects_unknown_flags_and_prints_usage() {
    let (base, dir, keys) = scratch();
    let out = run_cli_in_keys(&dir, &keys, &["clean", "--force"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown flag"), "got:\n{stderr}");
    assert!(
        stderr.contains("--dry-run"),
        "must echo the usage:\n{stderr}"
    );

    let out = run_cli_in_keys(&dir, &keys, &["clean", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("usage: noct clean"), "got:\n{stdout}");
    assert!(
        stdout.contains("vendor/"),
        "usage must state the guarantee:\n{stdout}"
    );
    cleanup(&base);
}
