//! The global, content-addressed dependency store (TOOLCHAIN.md §3,
//! ADR-026, IMPLEMENTATION_PLAN.md Phase 6 exit criterion).
//!
//! *Derived lives with the project, fetched lives global, and the lock
//! is the mapping between them.* This module is the "fetched lives
//! global" half: fetched dependency content is keyed by the
//! `content: sha256:…` hash the lockfile already records, and two
//! projects that depend on the same bytes keep exactly one copy.
//!
//! # Layout
//!
//! ```text
//! <root>/
//!   archives/<hex>.pkg            verified archive bytes (the unit of
//!                                 verification + transfer)
//!   trees/<hex>/                  extracted tree (the unit of
//!                                 compilation), read-only
//!   index/<hex>.record            the store's OWN record: the tree
//!                                 hash the tree must still hash to,
//!                                 plus the stat fingerprint it hashed
//!                                 with
//!   points/<name>-<version>.point one line: the tree path, relative to
//!                                 <root>. This is what lets the module
//!                                 resolver find a content-addressed
//!                                 tree BY NAME (see
//!                                 `compiler::modules::ModuleGraph::load_with`)
//! ```
//!
//! `<root>` is `NOCT_STORE` if set, else the per-OS data home
//! (`store_root()`). The store is shared by every project on the
//! machine, so nothing project-specific may live in it — the `points/`
//! files are keyed by package name and version, never by path.
//!
//! # Both halves are kept
//!
//! The archive is the unit of verification and transfer; the tree is
//! the unit of compilation. Neither substitutes for the other (ADR-026:
//! every mature ecosystem keeps the pair), and the tree is NOT derived
//! on demand — a missing tree is a loud, refetchable problem, never a
//! silent side effect of a build.
//!
//! # Verification, and where the trust root is
//!
//! **The lockfile format does not change** — it records one hash per
//! package, the archive's. So the archive's chain is:
//!
//! ```text
//! lock `content:` hash → archive bytes hashed at fetch → tree
//! unpacked from exactly those bytes → tree hash recorded in <root>
//! ```
//!
//! The archive is hashed ONCE, at fetch (`registry::fetch_locked`),
//! because the index is the only thing that can re-derive the
//! signature, and re-reading the archive on every build would buy
//! nothing the tree hash does not already cover.
//!
//! The TREE's trust root is the store's own `index/<hex>.record`. The
//! build re-verifies the tree against that record before compiling
//! from it, which closes the hole this store exists to fix: a
//! hand-edited extracted tree used to compile and ship. Concretely,
//! tampering with a tree file now fails the build loudly instead of
//! running.
//!
//! # What read-only means here (honestly)
//!
//! Trees are placed read-only: the read-only file attribute on Windows,
//! the owner-write bits dropped elsewhere. **That is a deterrent, not
//! a security boundary** — a local process that can write to the store
//! root can also rewrite `index/<hex>.record`, so the hash check catches
//! drift, corruption, and any tampering that does not reach the
//! record; it does not defend against an attacker who owns the store
//! directory. The real protection for that case is filesystem
//! permissions, which is the operator's call, not ours. The hash is the
//! check; read-only only makes accidental edits fail loudly.
//!
//! # What the fast path is (and is not)
//!
//! Re-hashing every extracted tree on every build would just move the
//! old per-build re-hash cost (TOOLCHAIN.md §3: "archives hash once at
//! fetch") rather than remove it. So a recorded tree also carries a
//! **fingerprint**: a hash over `(relative path, byte length, mtime)`
//! for every file in the tree, taken from the metadata the directory
//! scan already returned. The build walks the tree and compares
//! fingerprints — stats, no file bytes read, no SHA over content. A
//! moved mtime (any write, including a hand edit) invalidates it and
//! the full content hash runs; when the content still matches, the
//! record's fingerprint is refreshed so the next build is fast again.
//!
//! In-process, a fingerprint that a full content hash already accepted
//! is memoized, so a repeat check in the same process (`noct test` runs
//! the gate once per test file) skips the content read even when the
//! record on disk could not be rewritten — a read-only store root, say.
//! The stat walk itself is never memoized away: skipping it is exactly
//! the unsound thing.
//!
//! That makes the fingerprint a *validity* check, not a *security*
//! check: anything that can change file mtimes can also forge it. It
//! is an honest engineering trade, it is measured in the dated
//! amendment under TOOLCHAIN.md §3 (it beats the old per-build archive
//! hash above ~20 files / ~100 KiB and loses below that), and it is one
//! env var from being turned off: `NOCT_STORE_REVERIFY=1` forces the
//! full content hash on every check, and `noct clean` drops the whole
//! store, forcing a refetch. The counters below make the trade
//! measurable rather than asserted in a comment.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::manifest::{LockedPackage, SemVer};
use crate::registry::{sha256_hex, unpack};

// ── Instrumentation ──────────────────────────────────────────────────────────
//
// "Archives hash once at fetch" is the load-bearing performance claim of
// this store, so it is a number, not a promise. Every hash of a
// dependency's bytes is counted here, and `NOCT_STORE_TRACE=1` makes the
// CLI print them (`trace_line`), so a test can prove the second build
// re-hashed nothing.

static ARCHIVE_HASHES: AtomicU64 = AtomicU64::new(0);
static TREE_HASHES: AtomicU64 = AtomicU64::new(0);
static FINGERPRINT_CHECKS: AtomicU64 = AtomicU64::new(0);
static MEMO_HITS: AtomicU64 = AtomicU64::new(0);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Process-wide memo of verified trees: `tree path -> fingerprint that
/// a FULL content hash accepted`. Only ever written after a successful
/// content hash, so it can never make a tree look verified that was
/// not. Lazily created so the static needs no const `HashMap::new`.
static MEMO: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

/// Hashing work done by this process, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Times dependency archive bytes were hashed (fetch only).
    pub archive_hashes: u64,
    /// Times an extracted tree's full content was hashed.
    pub tree_hashes: u64,
    /// Times a tree was stat-walked to compare its fingerprint.
    pub fingerprint_checks: u64,
    /// Times a check was satisfied from the in-process memo (no walk,
    /// no read).
    pub memo_hits: u64,
}

/// The counters as they stand.
pub fn counters() -> Counters {
    Counters {
        archive_hashes: ARCHIVE_HASHES.load(Ordering::Relaxed),
        tree_hashes: TREE_HASHES.load(Ordering::Relaxed),
        fingerprint_checks: FINGERPRINT_CHECKS.load(Ordering::Relaxed),
        memo_hits: MEMO_HITS.load(Ordering::Relaxed),
    }
}

/// Count one archive hash (call at the single place archive bytes are
/// hashed).
pub fn count_archive_hash() {
    ARCHIVE_HASHES.fetch_add(1, Ordering::Relaxed);
}

/// `NOCT_STORE_TRACE` diagnostic: one machine-greppable line naming what
/// this process hashed. Returns `None` when the env var is unset, so
/// normal runs stay silent.
pub fn trace_line(label: &str) -> Option<String> {
    if std::env::var_os("NOCT_STORE_TRACE").is_none() {
        return None;
    }
    let c = counters();
    Some(format!(
        "store-verify[{label}]: archives={} trees={} fingerprints={} memo-hits={}",
        c.archive_hashes, c.tree_hashes, c.fingerprint_checks, c.memo_hits
    ))
}

/// `NOCT_STORE_REVERIFY=1`: skip the fingerprint fast path and hash every
/// tree's content on every check. The honest escape hatch for anyone who
/// wants the security check without the stat-walk shortcut.
pub fn full_reverify_requested() -> bool {
    matches!(std::env::var("NOCT_STORE_REVERIFY").as_deref(), Ok("1") | Ok("true"))
}

// ── Store root ───────────────────────────────────────────────────────────────

/// Where the store lives.
///
/// `NOCT_STORE` wins (the `NOCT_KEYS` precedent, for hermetic tests and
/// unusual layouts). Otherwise the per-OS **data** home, mirroring
/// `registry::key_dir`'s per-OS split — the store is cached content, so
/// it is data, not configuration:
///
/// | Platform | Default root                          |
/// |----------|---------------------------------------|
/// | Windows  | `%APPDATA%\noctivue`, else `%USERPROFILE%\.noctivue` |
/// | macOS    | `~/Library/Application Support/noctivue` |
/// | Linux/other | `$XDG_DATA_HOME/noctivue`, else `~/.local/share/noctivue` |
///
/// TOOLCHAIN.md §3 names the shape (`~/.noctivue`, XDG-aware,
/// `NOCT_STORE` override); the per-OS table follows `key_dir` so the two
/// never disagree about what "the per-OS home" means.
pub fn store_root() -> PathBuf {
    if let Ok(dir) = std::env::var("NOCT_STORE") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(default) = platform_root() {
        return default;
    }
    PathBuf::from(".").join(".noctivue")
}

#[cfg(windows)]
fn platform_root() -> Option<PathBuf> {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return Some(PathBuf::from(appdata).join("noctivue"));
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.is_empty() {
            return Some(PathBuf::from(profile).join(".noctivue"));
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn platform_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("noctivue"),
    )
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("noctivue"));
        }
    }
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".local").join("share").join("noctivue"))
}

// ── Hash keys ────────────────────────────────────────────────────────────────

/// The hex half of a `sha256:<hex>` content hash, i.e. the store's key.
/// `None` for anything that is not a well-formed sha256 hash: a key we
/// cannot derive is a package we cannot place, and inventing a
/// directory name from a malformed hash is how path traversal gets in.
pub fn content_hex(content: &str) -> Option<&str> {
    let hex = content.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return None;
    }
    Some(hex)
}

// ── Local state ──────────────────────────────────────────────────────────────

/// What the store holds for one locked package. The two states are
/// exhaustive and both halves are required: half a copy is not a usable
/// copy (the resolver reads the tree; `vendor` re-derives from the
/// archive; the build gate needs the record that makes the tree
/// trustworthy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreState {
    /// Archive, tree, and the record + pointer that make the tree
    /// usable are all present, and the tree verified against the record
    /// just now.
    Ready,
    /// Not usable, and the `String` says why in human words: a missing
    /// half, a missing record, a tree that no longer hashes to what the
    /// store recorded. The reason is what makes a loud refetch
    /// actionable instead of mysterious.
    NeedsFetch(String),
}

// ── The store ────────────────────────────────────────────────────────────────

/// One content-addressed store rooted at a directory. Cheap to build,
/// holds no state: every operation goes to disk, so two `noct` processes
/// on the same machine see the same store.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store this machine uses (`NOCT_STORE` or the per-OS home).
    pub fn open() -> Store {
        Store::with_root(store_root())
    }

    /// A store at an explicit root. Used by tests and by callers that
    /// already resolved a root.
    pub fn with_root(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn archives_dir(&self) -> PathBuf {
        self.root.join("archives")
    }

    pub fn trees_dir(&self) -> PathBuf {
        self.root.join("trees")
    }

    pub fn index_dir(&self) -> PathBuf {
        self.root.join("index")
    }

    pub fn points_dir(&self) -> PathBuf {
        self.root.join("points")
    }

    /// The root the module resolver is handed (see
    /// `ModuleGraph::load_with`): a store is a valid package root, and
    /// its `points/` entries name the trees.
    pub fn resolution_root(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn archive_path(&self, hex: &str) -> PathBuf {
        self.archives_dir().join(format!("{hex}.pkg"))
    }

    pub fn tree_path(&self, hex: &str) -> PathBuf {
        self.trees_dir().join(hex)
    }

    pub fn record_path(&self, hex: &str) -> PathBuf {
        self.index_dir().join(format!("{hex}.record"))
    }

    pub fn point_path(&self, name: &str, version: SemVer) -> PathBuf {
        self.points_dir().join(format!("{name}-{version}.point"))
    }

    /// The four derived subtrees, in the order `noct clean` reclaims
    /// them. Only these four: a future `noct clean` must never be able
    /// to widen into something that is not derived (and an operator's
    /// own files under the root survive).
    pub fn derived_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.archives_dir(),
            self.trees_dir(),
            self.index_dir(),
            self.points_dir(),
        ]
    }

    // ── Writing ────────────────────────────────────────────────────────────

    /// Put verified dependency content into the store: the archive
    /// bytes, the extracted tree, the record, and the name pointer.
    ///
    /// `archive` MUST already have been verified against the lock's
    /// `content:` hash (and its signature) by `fetch_locked` — this
    /// function deliberately does not re-hash the archive, because
    /// "hash once at fetch" is the whole point and a second pass here
    /// would be the per-build cost this store exists to delete. It is
    /// `pub(crate)` for the same reason: there is exactly one door into
    /// the store, and it is behind verification.
    ///
    /// Placement discipline, in order:
    /// 1. the archive is written whole (temp + rename), read-only;
    /// 2. the tree is unpacked into a temp SIBLING of its final path and
    ///    hashed there — a partial tree is never visible under its own
    ///    name;
    /// 3. the final name is taken by `rename` (atomic on one volume);
    /// 4. the tree is marked read-only;
    /// 5. the record is written LAST. A tree with no record is not
    ///    trusted by anything, so a crash between 3 and 5 leaves an
    ///    inert directory that the next `get` replaces, never a
    ///    half-verified tree a build will read.
    pub(crate) fn put(&self, locked: &LockedPackage, archive: &[u8]) -> Result<(), String> {
        let hex = content_hex(locked.content.as_deref().unwrap_or_default())
            .ok_or_else(|| format!("lock entry `{}` has no usable content hash", locked.name))?
            .to_string();

        // ── 1. archive ────────────────────────────────────────────────────
        let archive_path = self.archive_path(&hex);
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        clear_readonly(&archive_path)?;
        write_atomic(&archive_path, archive)
            .map_err(|e| format!("cannot write {}: {e}", archive_path.display()))?;
        set_readonly(&archive_path, true)
            .map_err(|e| format!("cannot mark {} read-only: {e}", archive_path.display()))?;

        // ── 2. unpack into a temp sibling, and hash it there ──────────────
        let trees = self.trees_dir();
        fs::create_dir_all(&trees)
            .map_err(|e| format!("cannot create {}: {e}", trees.display()))?;
        let temp = self.temp_sibling(&trees, "unpack");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp)
            .map_err(|e| format!("cannot create {}: {e}", temp.display()))?;
        let entries = unpack(archive)
            .map_err(|e| format!("cannot unpack `{}`: {e}", locked.name))?;
        for (rel, bytes) in &entries {
            let target = temp.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            fs::write(&target, bytes)
                .map_err(|e| format!("cannot unpack {rel}: {e}"))?;
        }
        let (files, content, fingerprint) = walk_hashes(&temp)?;
        TREE_HASHES.fetch_add(1, Ordering::Relaxed);

        // ── 3. take the final name atomically ─────────────────────────────
        let dest = self.tree_path(&hex);
        // Whether the tree under the final name is the one we just
        // unpacked. It usually is not: a re-fetch of a good store keeps
        // the tree that is already there, and its fingerprint (not the
        // temp's) is what the record must carry.
        let mut placed = true;
        if dest.exists() {
            // A tree is already there. If it still verifies against the
            // store's record, keep it and throw the temp away (a refetch
            // of a good store is a no-op). If it does not, it is drift:
            // clear the read-only flags and put the verified bytes in.
            if self.verify_tree(locked).is_ok() {
                let _ = fs::remove_dir_all(&temp);
                placed = false;
            } else {
                clear_readonly_tree(&dest)?;
                fs::remove_dir_all(&dest).map_err(|e| {
                    format!("cannot replace {}: {e}", dest.display())
                })?;
                self.place(&temp, &dest)?;
            }
        } else {
            self.place(&temp, &dest)?;
        }

        // ── 4. read-only placement ────────────────────────────────────────
        mark_tree_readonly(&dest)?;

        // ── 5. the record, last: it is what makes the tree trusted ───────
        //    Skipped when the tree that verified is the one still in
        //    place: its record is already on disk and already correct,
        //    and rewriting it with the discarded temp's fingerprint
        //    would send the next build through a pointless full hash.
        if placed {
            let record = Record {
                content: locked.content.clone().unwrap_or_default(),
                tree: content,
                files,
                fingerprint: fingerprint.clone(),
            };
            if let Some(parent) = self.record_path(&hex).parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            write_atomic(&self.record_path(&hex), record.render().as_bytes())
                .map_err(|e| format!("cannot write the store record: {e}"))?;
        }
        memoize(&dest, &fingerprint);

        // ── 6. the by-name pointer the module resolver reads ──────────────
        let point = self.point_path(&locked.name, locked.version);
        if let Some(parent) = point.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        // Always `/`-separated: the resolver reads this on every
        // platform, and a pointer is data, not a `PathBuf` dump.
        write_atomic(&point, format!("trees/{hex}\n").as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", point.display()))?;
        Ok(())
    }

    // ── Reading ────────────────────────────────────────────────────────────

    /// Read one archive out of the store, re-verifying it against the
    /// lock's `content:` hash. Used where the ARCHIVE is the input (not
    /// the tree): `noct vendor` unpacks it to produce a committed tree.
    /// A tampered archive fails closed here, so `vendor` can never commit
    /// attacker bytes.
    pub fn read_archive(&self, locked: &LockedPackage) -> Result<Vec<u8>, String> {
        let Some(hex) = content_hex(locked.content.as_deref().unwrap_or_default()) else {
            return Err(format!(
                "lock entry `{}` has no usable content hash",
                locked.name
            ));
        };
        let path = self.archive_path(hex);
        let bytes = fs::read(&path).map_err(|_| {
            format!(
                "no archive for `{}@{}` in the store ({} missing; run `noct get` to fetch it)",
                locked.name,
                locked.version,
                display(&path)
            )
        })?;
        let expected = locked.content.as_deref().unwrap_or_default();
        count_archive_hash();
        let actual = sha256_hex(&bytes);
        if actual != expected {
            return Err(format!(
                "`{}@{}` store archive hash mismatch: expected {expected}, found {actual} (the store's archive was modified after fetch)",
                locked.name, locked.version
            ));
        }
        Ok(bytes)
    }

    /// The tree's stored path, for callers that need to name it in a
    /// message.
    pub fn tree_path_for(&self, locked: &LockedPackage) -> Option<PathBuf> {
        content_hex(locked.content.as_deref().unwrap_or_default())
            .map(|hex| self.tree_path(hex))
    }

    /// Verify one locked package's tree against the store's record.
    ///
    /// Order of business: cheap validity first (the recorded
    /// fingerprint, memoized in-process), full content hash only when
    /// that is not conclusive, `--frozen` callers simply do not come
    /// here. Every failure is a named, actionable string.
    pub fn verify_tree(&self, locked: &LockedPackage) -> Result<(), String> {
        let hex = content_hex(locked.content.as_deref().unwrap_or_default())
            .ok_or_else(|| format!("lock entry `{}` has no usable content hash", locked.name))?;
        let tree = self.tree_path(hex);
        let record_path = self.record_path(hex);
        let text = fs::read_to_string(&record_path).map_err(|_| {
            format!(
                "the store has no verification record for `{}@{}` ({} missing) — `noct get` re-fetches",
                locked.name,
                locked.version,
                display(&record_path)
            )
        })?;
        let record = Record::parse(&text).map_err(|e| {
            format!(
                "the store's record for `{}@{}` is unreadable ({e}) — `noct get` re-fetches",
                locked.name, locked.version
            )
        })?;
        let expected_content = locked.content.as_deref().unwrap_or_default();
        if record.content != expected_content {
            return Err(format!(
                "the store's record for `{}@{}` is for a different archive ({} ≠ {expected_content})",
                locked.name, locked.version, record.content
            ));
        }
        if !tree.is_dir() {
            return Err(format!(
                "the store's tree for `{}@{}` is missing ({})",
                locked.name,
                locked.version,
                display(&tree)
            ));
        }

        // One sorted listing drives everything below, so the content
        // hash and the fingerprint can never disagree about which files
        // the tree has.
        let files = collect_files(&tree)?;

        // Fast path. A stat walk: no file reads, no SHA over content.
        // The recorded fingerprint covers every file's
        // `(path, length, mtime)`; a memo hit additionally means this
        // exact fingerprint cleared a FULL content hash earlier in this
        // same process (the case where the record itself could not be
        // rewritten — a read-only store root, say).
        if full_reverify_requested() {
            let content = content_hash(&tree, &files)?;
            TREE_HASHES.fetch_add(1, Ordering::Relaxed);
            if content != record.tree {
                return Err(tree_mismatch(
                    &locked.name,
                    &locked.version,
                    &record.tree,
                    &content,
                ));
            }
            memoize(&tree, &content);
            return Ok(());
        }

        let fingerprint = fingerprint(&files);
        FINGERPRINT_CHECKS.fetch_add(1, Ordering::Relaxed);
        if memo_matches(&tree, &fingerprint) {
            MEMO_HITS.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        if fingerprint == record.fingerprint {
            memoize(&tree, &fingerprint);
            return Ok(());
        }

        // The fingerprint moved: a write, a `touch`, or a restore from
        // backup. Only a full content hash can answer now.
        let content = content_hash(&tree, &files)?;
        TREE_HASHES.fetch_add(1, Ordering::Relaxed);
        if content != record.tree {
            return Err(tree_mismatch(
                &locked.name,
                &locked.version,
                &record.tree,
                &content,
            ));
        }
        // Same bytes, different mtimes (a touch, a re-save). Re-record
        // the fingerprint so the next build is fast again — content
        // equal is content equal, and rewriting the record to match what
        // is on disk does not weaken what it attests.
        let refreshed = Record {
            content: record.content.clone(),
            tree: content,
            files: files.len(),
            fingerprint: fingerprint.clone(),
        };
        write_atomic(&record_path, refreshed.render().as_bytes())
            .map_err(|e| format!("cannot refresh the store record: {e}"))?;
        memoize(&tree, &fingerprint);
        Ok(())
    }

    /// Classify one locked package's local state, verifying the tree
    /// against the store's record. Pure read: no writes beyond a
    /// fingerprint refresh, no index, no network, no key store.
    ///
    /// `path` sources have no derived copy at all (the tree is the
    /// user's own directory), so they are never `Ready` — callers skip
    /// them rather than fetch (see `cmd_get`).
    pub fn state(&self, locked: &LockedPackage) -> StoreState {
        let Some(hex) = content_hex(locked.content.as_deref().unwrap_or_default()) else {
            return StoreState::NeedsFetch(format!(
                "lock entry `{}` has no usable content hash — bytes the lock cannot name are bytes nobody can verify",
                locked.name
            ));
        };
        let archive = self.archive_path(hex);
        if !archive.is_file() {
            return StoreState::NeedsFetch(format!(
                "no archive in the store ({} missing)",
                display(&archive)
            ));
        }
        let tree = self.tree_path(hex);
        if !tree.is_dir() {
            return StoreState::NeedsFetch(format!(
                "archive verified at fetch but the extracted tree is missing ({})",
                display(&tree)
            ));
        }
        let point = self.point_path(&locked.name, locked.version);
        if !point.is_file() {
            return StoreState::NeedsFetch(format!(
                "extracted tree present but the store has no by-name pointer for it ({}) — the module resolver could not find it",
                display(&point)
            ));
        }
        match self.verify_tree(locked) {
            Ok(()) => StoreState::Ready,
            Err(e) => StoreState::NeedsFetch(e),
        }
    }

    fn temp_sibling(&self, parent: &Path, tag: &str) -> PathBuf {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        parent.join(format!(".tmp-{tag}-{}-{n}-{stamp}", std::process::id()))
    }

    /// Move a fully written, hashed temp tree onto its final name.
    ///
    /// A shared store makes this genuinely racy: two projects can fetch
    /// the same content at the same moment, and the loser's `rename`
    /// fails with "already exists". That is not a failure — the winner
    /// placed the same verified bytes under the same name — so it is
    /// resolved by verifying what landed, and only reported if THAT
    /// does not check out.
    fn place(&self, temp: &Path, dest: &Path) -> Result<(), String> {
        match fs::rename(temp, dest) {
            Ok(()) => Ok(()),
            Err(e) => {
                if dest.is_dir() {
                    let _ = fs::remove_dir_all(temp);
                    Ok(())
                } else {
                    Err(format!("cannot place {}: {e}", dest.display()))
                }
            }
        }
    }
}

// ── The record ───────────────────────────────────────────────────────────────

/// What the store knows about one extracted tree.
///
/// This is the store's own trust root for the tree (the lockfile cannot
/// carry a second hash without changing its format, which this change
/// does not do). It is deliberately plain text: an operator can read why
/// a tree was rejected without a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    /// The archive hash this tree was unpacked from (the lock's
    /// `content:` value).
    content: String,
    /// The tree's own content hash, as recorded at unpack.
    tree: String,
    files: usize,
    /// Hash over `(relative path, length, mtime)` per file, as of the
    /// last full content hash. The fast path's validity token.
    fingerprint: String,
}

impl Record {
    fn render(&self) -> String {
        format!(
            "# noctivue store tree record v1\ncontent: {}\ntree: {}\nfiles: {}\nfingerprint: {}\n",
            self.content, self.tree, self.files, self.fingerprint
        )
    }

    fn parse(text: &str) -> Result<Record, String> {
        let field = |key: &str| -> Option<String> {
            text.lines().find_map(|l| {
                let rest = l.strip_prefix(key)?.strip_prefix(": ")?;
                Some(rest.trim().to_string())
            })
        };
        let content = field("content").ok_or("no `content:` line")?;
        let tree = field("tree").ok_or("no `tree:` line")?;
        let fingerprint = field("fingerprint").ok_or("no `fingerprint:` line")?;
        let files = field("files")
            .ok_or("no `files:` line")?
            .parse::<usize>()
            .map_err(|e| format!("bad `files:` line: {e}"))?;
        Ok(Record {
            content,
            tree,
            files,
            fingerprint,
        })
    }
}

// ── Tree walking, hashing, fingerprints ──────────────────────────────────────

/// The one tree hash error message, so a drifted tree reads the same
/// whether the build gate, `get`, or `vendor` found it.
fn tree_mismatch(name: &str, version: &SemVer, expected: &str, actual: &str) -> String {
    format!(
        "extracted tree hash mismatch for `{name}@{version}`: the store recorded {expected} but the tree on disk hashes to {actual} (the tree was modified after it was verified — `noct get` re-fetches it, it is never patched in place)"
    )
}

/// One file of a tree: its sorted relative path plus the metadata that
/// came with the directory scan.
///
/// The metadata is deliberately the `DirEntry`'s, never a fresh
/// `fs::metadata(path)` query: the scan already carries length and
/// timestamps, and re-opening every file by path turned the warm path
/// from a stat walk into N file opens — measured at an order of
/// magnitude slower on Windows (see the dated amendment in
/// TOOLCHAIN.md §3 for the numbers).
type Entry = (String, fs::Metadata);

/// Every file in a tree, sorted, with scan-time metadata.
///
/// A non-regular entry (a symlink, say) is a hard error, not a skipped
/// row: the unpacker never produces one, so finding one means the tree
/// was edited by something other than us, and a walker that quietly
/// ignores what it does not understand is a walker that certifies a
/// tree it never looked at.
fn collect_files(root: &Path) -> Result<Vec<Entry>, String> {
    let mut out: Vec<Entry> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read an entry of {}: {e}", dir.display()))?;
            let kind = entry
                .file_type()
                .map_err(|e| format!("cannot stat {}: {e}", entry.path().display()))?;
            let path = entry.path();
            if kind.is_symlink() {
                return Err(format!(
                    "unexpected symlink inside a dependency tree: {} (the unpacker never creates one — this tree was modified by something else)",
                    path.display()
                ));
            }
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|_| "walked outside the tree".to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let meta = entry
                    .metadata()
                    .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
                out.push((rel, meta));
            } else {
                return Err(format!(
                    "unexpected non-regular entry inside a dependency tree: {}",
                    path.display()
                ));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Both hashes in one pass — the expensive one, used at placement and
/// whenever a fingerprint moved: `(file count, content hash,
/// fingerprint)`.
fn walk_hashes(root: &Path) -> Result<(usize, String, String), String> {
    let files = collect_files(root)?;
    let content = content_hash(root, &files)?;
    let fingerprint = fingerprint(&files);
    Ok((files.len(), content, fingerprint))
}

/// The security hash: canonical `path\0len\0bytes` per file, sorted.
/// Empty directories contribute nothing — the unpacker never creates
/// one and no module resolves through one.
fn content_hash(root: &Path, files: &[Entry]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for (rel, meta) in files {
        let abs = root.join(rel);
        let bytes = fs::read(&abs).map_err(|e| format!("cannot read {}: {e}", abs.display()))?;
        hasher.update(head(rel, meta.len()).as_bytes());
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{}", hex_of(hasher.finalize())))
}

/// The validity token: canonical `path\0len\0mtime` per file, sorted,
/// from scan-time metadata only — no file bytes are read and no file is
/// re-opened, which is the whole reason the warm path is cheap.
fn fingerprint(files: &[Entry]) -> String {
    let mut hasher = Sha256::new();
    for (rel, meta) in files {
        hasher.update(head(rel, meta.len()).as_bytes());
        hasher.update(mtime_nanos(meta).to_string().as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{}", hex_of(hasher.finalize()))
}

/// The per-entry frame both hashes share, so the two digests can never
/// be fed the same bytes from two different trees.
fn head(rel: &str, len: u64) -> String {
    format!("{rel}\0{len}\0")
}

fn mtime_nanos(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

// ── In-process memo ──────────────────────────────────────────────────────────

/// Remember that this exact fingerprint cleared a full content hash in
/// this process.
fn memoize(tree: &Path, fingerprint: &str) {
    if let Ok(mut guard) = MEMO.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(tree.to_path_buf(), fingerprint.to_string());
    }
}

fn memo_matches(tree: &Path, fingerprint: &str) -> bool {
    match MEMO.lock() {
        Ok(guard) => guard
            .as_ref()
            .and_then(|m| m.get(tree))
            .is_some_and(|seen| seen == fingerprint),
        Err(_) => false,
    }
}

// ── Read-only placement ──────────────────────────────────────────────────────

/// Mark one file read-only (Windows read-only attribute; elsewhere the
/// owner/group/other write bits dropped).
fn set_readonly(path: &Path, readonly: bool) -> Result<(), String> {
    let mut perms = fs::metadata(path)
        .map_err(|e| format!("cannot read permissions of {}: {e}", path.display()))?
        .permissions();
    perms.set_readonly(readonly);
    fs::set_permissions(path, perms)
        .map_err(|e| format!("cannot set permissions on {}: {e}", path.display()))
}

fn clear_readonly(path: &Path) -> Result<(), String> {
    if path.exists() {
        set_readonly(path, false)?;
    }
    Ok(())
}

/// Mark a whole placed tree read-only: files first, then directories
/// (a directory must stay writable while its children are being
/// visited).
///
/// A DETERRENT, not a boundary — see the module docs. On Windows the
/// attribute on a file is a real obstacle to an accidental overwrite;
/// on a directory it is mostly cosmetic. The hash is the check.
pub(crate) fn mark_tree_readonly(root: &Path) -> Result<(), String> {
    let mut dirs = Vec::new();
    mark_tree_readonly_inner(root, &mut dirs)?;
    for dir in dirs.iter().rev() {
        set_readonly(dir, true)
            .map_err(|e| format!("cannot mark {} read-only: {e}", dir.display()))?;
    }
    Ok(())
}

fn mark_tree_readonly_inner(dir: &Path, dirs: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read an entry of {}: {e}", dir.display()))?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
            .is_dir()
        {
            mark_tree_readonly_inner(&path, dirs)?;
        } else {
            set_readonly(&path, true)
                .map_err(|e| format!("cannot mark {} read-only: {e}", path.display()))?;
        }
    }
    dirs.push(dir.to_path_buf());
    Ok(())
}

/// Undo [`mark_tree_readonly`] over a whole tree. Needed by `noct clean`
/// (Windows refuses to delete a read-only file) and by the repair path
/// that replaces a drifted tree. Children before parents, for the same
/// reason they are marked in the other order.
pub(crate) fn clear_readonly_tree(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let mut dirs = Vec::new();
    clear_readonly_inner(root, &mut dirs)?;
    for dir in dirs.iter().rev() {
        set_readonly(dir, false)
            .map_err(|e| format!("cannot clear read-only on {}: {e}", dir.display()))?;
    }
    Ok(())
}

fn clear_readonly_inner(dir: &Path, dirs: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read an entry of {}: {e}", dir.display()))?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
            .is_dir()
        {
            clear_readonly_inner(&path, dirs)?;
        } else {
            set_readonly(&path, false)
                .map_err(|e| format!("cannot clear read-only on {}: {e}", path.display()))?;
        }
    }
    dirs.push(dir.to_path_buf());
    Ok(())
}

// ── Small filesystem helpers ────────────────────────────────────────────────

/// Write a file whole or not at all: temp sibling, then rename. The
/// only writes that matter (the archive, the record, the pointer) all
/// go through this, so a crash can never leave a half-written record
/// that reads as a valid one.
fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let temp = parent.join(format!(
        ".tmp-write-{}-{n}",
        std::process::id()
    ));
    fs::write(&temp, bytes).map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
    match fs::rename(&temp, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(format!("cannot place {}: {e}", dest.display()))
        }
    }
}

fn hex_of(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `a/b/c` rather than `a\\b\\c`: store paths appear in messages that
/// people compare against docs.
fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noctivue-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn locked(content: &str) -> LockedPackage {
        LockedPackage {
            name: "leaf".to_string(),
            version: SemVer::parse("1.4.0", 0).unwrap(),
            source: crate::manifest::Source::Registry,
            content: Some(content.to_string()),
            tier: crate::manifest::Tier::Native,
            signed_by: Some("key:4242".to_string()),
            experimental: false,
        }
    }

    fn tarball(files: &[(&str, &str)]) -> (Vec<u8>, String) {
        let bytes = crate::registry::pack(
            &files
                .iter()
                .map(|(p, c)| (*p, c.as_bytes()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let hash = sha256_hex(&bytes);
        (bytes, hash)
    }

    #[test]
    fn content_hex_only_accepts_a_real_sha256() {
        assert_eq!(content_hex(&format!("sha256:{}", "a".repeat(64))), Some("a".repeat(64).as_str()));
        assert!(content_hex("sha256:short").is_none());
        assert!(content_hex(&format!("sha256:{}", "A".repeat(64))).is_none());
        assert!(content_hex(&"a".repeat(64)).is_none());
        // A key that could escape the store directory is refused, not
        // sanitised into something plausible.
        assert!(content_hex("sha256:../../evil").is_none());
    }

    #[test]
    fn put_places_both_halves_atomically_and_read_only() {
        let dir = scratch("put");
        let store = Store::with_root(dir.join("store"));
        let (bytes, hash) = tarball(&[("lib/main.nv", "hi")]);
        let pkg = locked(&hash);

        assert!(matches!(store.state(&pkg), StoreState::NeedsFetch(_)));
        store.put(&pkg, &bytes).unwrap();

        let hex = content_hex(&hash).unwrap().to_string();
        assert!(store.archive_path(&hex).is_file());
        let tree = store.tree_path(&hex);
        assert_eq!(
            fs::read_to_string(tree.join("lib/main.nv")).unwrap(),
            "hi"
        );
        assert_eq!(store.state(&pkg), StoreState::Ready);

        // Read-only placement: the file's write bit is gone. A
        // deterrent, not a boundary (see the module docs) — but the
        // check is the hash, and this is the cheap half of it.
        let perms = fs::metadata(tree.join("lib/main.nv")).unwrap().permissions();
        assert!(perms.readonly(), "extracted files must land read-only");

        // No temp siblings survive a successful placement.
        let strays: Vec<String> = fs::read_dir(store.trees_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".tmp-"))
            .collect();
        assert!(strays.is_empty(), "partial trees left visible: {strays:?}");

        // A re-put of identical content is a no-op: the verified tree
        // stays, the temp is discarded, and the record keeps describing
        // the tree that is actually in place.
        let record_before = fs::read_to_string(store.record_path(content_hex(&hash).unwrap()))
            .expect("record");
        store.put(&pkg, &bytes).unwrap();
        assert_eq!(store.state(&pkg), StoreState::Ready);
        assert_eq!(
            fs::read_to_string(store.record_path(content_hex(&hash).unwrap())).unwrap(),
            record_before,
            "a no-op re-put must not rewrite the record from a discarded temp"
        );
        let strays: Vec<String> = fs::read_dir(store.trees_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".tmp-"))
            .collect();
        assert!(strays.is_empty(), "a re-put left a temp behind: {strays:?}");

        // The by-name pointer names the tree relative to the root.
        let point = store.point_path("leaf", pkg.version);
        assert_eq!(
            fs::read_to_string(&point).unwrap().trim(),
            format!("trees/{hex}")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tampered_tree_fails_closed() {
        let dir = scratch("tamper");
        let store = Store::with_root(dir.join("store"));
        let (bytes, hash) = tarball(&[("lib/main.nv", "hi")]);
        let pkg = locked(&hash);
        store.put(&pkg, &bytes).unwrap();

        // The blocker this store exists to fix: a hand-edited extracted
        // tree must not compile.
        let hex = content_hex(&hash).unwrap().to_string();
        let target = store.tree_path(&hex).join("lib/main.nv");
        clear_readonly(&target).unwrap();
        fs::write(&target, "evil():\n    1\n").unwrap();

        let err = store.verify_tree(&pkg).unwrap_err();
        assert!(err.contains("extracted tree hash mismatch"), "{err}");
        assert!(err.contains("leaf@1.4.0"), "{err}");
        assert!(matches!(store.state(&pkg), StoreState::NeedsFetch(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tampered_archive_fails_closed_on_read() {
        let dir = scratch("archive");
        let store = Store::with_root(dir.join("store"));
        let (bytes, hash) = tarball(&[("lib.nv", "hi")]);
        let pkg = locked(&hash);
        store.put(&pkg, &bytes).unwrap();
        // The archive is the unit of transfer: reading it re-hashes it
        // against the lock, so drift there fails closed too.
        let hex = content_hex(&hash).unwrap().to_string();
        clear_readonly(&store.archive_path(&hex)).unwrap();
        fs::write(store.archive_path(&hex), b"evil").unwrap();
        let err = store.read_archive(&pkg).unwrap_err();
        assert!(err.contains("hash mismatch"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_half_is_a_named_problem() {
        let dir = scratch("halves");
        let store = Store::with_root(dir.join("store"));
        let (bytes, hash) = tarball(&[("lib.nv", "hi")]);
        let pkg = locked(&hash);
        store.put(&pkg, &bytes).unwrap();
        let hex = content_hex(&hash).unwrap().to_string();

        // Archive gone.
        clear_readonly(&store.archive_path(&hex)).unwrap();
        fs::remove_file(store.archive_path(&hex)).unwrap();
        match store.state(&pkg) {
            StoreState::Ready => panic!("no archive — must not be Ready"),
            StoreState::NeedsFetch(why) => assert!(why.contains("no archive"), "{why}"),
        }
        store.put(&pkg, &bytes).unwrap();

        // Tree gone.
        clear_readonly_tree(&store.tree_path(&hex)).unwrap();
        fs::remove_dir_all(store.tree_path(&hex)).unwrap();
        match store.state(&pkg) {
            StoreState::Ready => panic!("no tree — must not be Ready"),
            StoreState::NeedsFetch(why) => assert!(why.contains("extracted tree is missing"), "{why}"),
        }
        store.put(&pkg, &bytes).unwrap();

        // Record gone: a tree with nothing to verify against is not
        // trusted, ever.
        fs::remove_file(store.record_path(&hex)).unwrap();
        match store.state(&pkg) {
            StoreState::Ready => panic!("no record — must not be Ready"),
            StoreState::NeedsFetch(why) => assert!(why.contains("no verification record"), "{why}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warm_verification_hashes_content_once_per_process() {
        let dir = scratch("warm");
        let store = Store::with_root(dir.join("store"));
        let (bytes, hash) = tarball(&[("lib.nv", "hi")]);
        let pkg = locked(&hash);
        store.put(&pkg, &bytes).unwrap();

        // Reset the counters so the fetch's own hashes do not pollute the
        // measurement, then verify repeatedly: the tree must be hashed
        // at most once, with every later check served by the fingerprint
        // or the memo.
        ARCHIVE_HASHES.store(0, Ordering::Relaxed);
        TREE_HASHES.store(0, Ordering::Relaxed);
        FINGERPRINT_CHECKS.store(0, Ordering::Relaxed);
        MEMO_HITS.store(0, Ordering::Relaxed);
        for _ in 0..5 {
            store.verify_tree(&pkg).unwrap();
        }
        let c = counters();
        assert_eq!(c.archive_hashes, 0, "archives never re-hashed at verify time");
        assert_eq!(c.tree_hashes, 0, "warm checks must not re-read the tree");
        assert!(c.fingerprint_checks + c.memo_hits >= 5, "{c:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_touch_invalidates_the_fingerprint_but_not_the_content() {
        let dir = scratch("touch");
        let store = Store::with_root(dir.join("store"));
        let (bytes, hash) = tarball(&[("lib.nv", "hi")]);
        let pkg = locked(&hash);
        store.put(&pkg, &bytes).unwrap();
        let hex = content_hex(&hash).unwrap().to_string();
        let target = store.tree_path(&hex).join("lib.nv");

        // A write that preserves the bytes (a restore from backup, a
        // re-save) moves the fingerprint, so the full hash runs, agrees,
        // and the record is refreshed — loud, cheap, and not an error.
        let original = fs::read(&target).unwrap();
        clear_readonly(&target).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&target, &original).unwrap();
        set_readonly(&target, true).unwrap();

        assert!(store.verify_tree(&pkg).is_ok());
        assert_eq!(store.state(&pkg), StoreState::Ready);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_root_honours_the_env_override() {
        let dir = scratch("root");
        let previous = std::env::var("NOCT_STORE").ok();
        std::env::set_var("NOCT_STORE", dir.join("elsewhere"));
        assert_eq!(store_root(), dir.join("elsewhere"));
        match previous {
            Some(v) => std::env::set_var("NOCT_STORE", v),
            None => std::env::remove_var("NOCT_STORE"),
        }
        assert_ne!(store_root(), dir.join("elsewhere"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn derived_dirs_are_only_the_store_and_never_the_root() {
        let store = Store::with_root("store");
        let got = store.derived_dirs();
        assert_eq!(
            got,
            vec![
                PathBuf::from("store").join("archives"),
                PathBuf::from("store").join("trees"),
                PathBuf::from("store").join("index"),
                PathBuf::from("store").join("points"),
            ]
        );
        // The root itself is never a target: an operator's own files
        // under it must survive `noct clean`.
        assert!(!got.iter().any(|p| p == Path::new("store")));
    }
}
