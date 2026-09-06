//! `noct add` — add a dependency to the current package.
//!
//! [Phase 4 / M3] Two source modes:
//!
//! - `--path <dir>`: offline sibling-tree dependency. Both manifests
//!   validated, name/version agreement checked, manifest rewritten
//!   byte-preserving (`upsert_dependency_text`), path entries
//!   regenerated in the lock. Hash/signature skipped — your tree.
//! - `--index <dir>` (registry mode): resolves the full closure
//!   against a file-backed index (`registry::FileIndex`), verifies +
//!   fetches every locked package (TOFU signing), then persists the
//!   upserted request + re-resolved lock. Nothing is written until
//!   fetch+verify succeed. Bare `add name` tracks the latest indexed
//!   version (caret).
//!
//! Every dependency entry declares a trust tier (ADR-015,
//! TOOLCHAIN.md §3). Package integrity hashes MUST be in place
//! before any public registry launch.
//!
//! Usage:
//!   noct add <package>[@version] [--path <path>] [--tier <tier>] [--opt-in]
//!   noct add <package>[@version] --index <dir> [--tier <tier>] [--opt-in] [--rotate-key]

use std::fs;
use std::path::{Path, PathBuf};

pub fn run(args: &[String]) -> i32 {
    let mut package_arg = String::new();
    let mut tier: Option<String> = None;
    let mut opt_in = false;
    let mut source_path: Option<String> = None;
    let mut index_dir: Option<String> = None;
    let mut rotate_key = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tier" => {
                tier = it.next().map(|s| s.clone());
                if tier.is_none() {
                    eprintln!("noct add: --tier requires a value");
                    return 1;
                }
            }
            "--opt-in" => {
                opt_in = true;
            }
            "--path" => {
                source_path = it.next().map(|s| s.clone());
                if source_path.is_none() {
                    eprintln!("noct add: --path requires a value");
                    return 1;
                }
            }
            "--index" => {
                index_dir = it.next().map(|s| s.clone());
                if index_dir.is_none() {
                    eprintln!("noct add: --index requires a directory");
                    return 1;
                }
            }
            "--rotate-key" => {
                rotate_key = true;
            }
            other => {
                if package_arg.is_empty() {
                    package_arg = other.to_string();
                } else {
                    eprintln!("noct add: too many arguments");
                    return 1;
                }
            }
        }
    }

    if package_arg.is_empty() {
        eprintln!("noct add: package name required");
        return 1;
    }

    // Parse package name and optional @version requirement.
    let (package_name, explicit_req) = match package_arg.split_once('@') {
        Some((name, ver)) => (name.to_string(), Some(ver.to_string())),
        None => (package_arg, None),
    };

    let manifest_path = Path::new("nestpkg.nvpm");

    let manifest_content = match fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("noct add: no nestpkg.nvpm in current directory");
            return 1;
        }
    };

    if let Err(e) = crate::manifest::parse_manifest(&manifest_content) {
        eprintln!("noct add: invalid manifest: {}", e);
        return 1;
    }

    let dep_tier = match &tier {
        Some(t) => match crate::manifest::Tier::parse(t, 0) {
            Ok(tier) => tier,
            Err(e) => {
                eprintln!("noct add: invalid tier '{}': {}", t, e);
                return 1;
            }
        },
        None => crate::manifest::Tier::Native,
    };

    // Tier opt-in rules: foreign-runtime REQUIRES --opt-in;
    // other tiers with --opt-in is a stray (error).
    if dep_tier == crate::manifest::Tier::ForeignRuntime && !opt_in {
        eprintln!("noct add: foreign-runtime tier requires --opt-in");
        return 1;
    }
    if dep_tier != crate::manifest::Tier::ForeignRuntime && opt_in {
        eprintln!("noct add: --opt-in is only valid with --tier foreign-runtime");
        return 1;
    }

    match source_path {
        Some(p) => {
            if index_dir.is_some() || rotate_key {
                eprintln!("noct add: --index/--rotate-key are registry-mode flags (not with --path)");
                return 1;
            }
            add_path(&manifest_content, manifest_path, &package_name, explicit_req.as_deref(), dep_tier, opt_in, &p)
        }
        None => {
            if index_dir.is_none() {
                eprintln!(
                    "noct add: registry adds need --index <dir> (no default registry exists; see `noct publish`)"
                );
                return 1;
            }
            add_registry(
                &manifest_content,
                manifest_path,
                &package_name,
                explicit_req.as_deref(),
                dep_tier,
                opt_in,
                index_dir.as_deref().unwrap(),
                rotate_key,
            )
        }
    }
}

// ── Path mode ─────────────────────────────────────────────────────────────────

/// Offline sibling dependency: validate both manifests, check
/// name/version agreement, upsert the request, regenerate the path
/// lock entry (idempotent — re-runs are byte-identical).
fn add_path(
    manifest_content: &str,
    manifest_path: &Path,
    package_name: &str,
    explicit_req: Option<&str>,
    dep_tier: crate::manifest::Tier,
    opt_in: bool,
    source_path: &str,
) -> i32 {
    // Read the sibling manifest.
    let sibling_manifest_path = Path::new(source_path).join("nestpkg.nvpm");
    let sibling_content = match fs::read_to_string(&sibling_manifest_path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "noct add: no manifest found at {}",
                sibling_manifest_path.display()
            );
            return 1;
        }
    };

    let sibling_manifest = match crate::manifest::parse_manifest(&sibling_content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("noct add: invalid sibling manifest: {}", e);
            return 1;
        }
    };

    // Verify name matches.
    if sibling_manifest.package.name != package_name {
        eprintln!(
            "noct add: name mismatch — requested '{}', found '{}' in {}",
            package_name,
            sibling_manifest.package.name,
            sibling_manifest_path.display()
        );
        return 1;
    }

    let sibling_version = sibling_manifest.package.version;

    // Determine the version requirement (explicit, else pin the
    // sibling's version exactly).
    let req = if let Some(ref v) = explicit_req {
        match crate::manifest::VersionReq::parse(v, 0) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("noct add: invalid version '{}': {}", v, e);
                return 1;
            }
        }
    } else {
        crate::manifest::VersionReq::Exact(sibling_version)
    };

    // Check version satisfaction if an explicit requirement was given.
    if explicit_req.is_some() && !crate::manifest::req_satisfied(req, sibling_version) {
        eprintln!(
            "noct add: sibling version {} does not satisfy requirement '{}'",
            sibling_version,
            explicit_req.unwrap()
        );
        return 1;
    }

    // Build the dependency.
    let dep = crate::manifest::Dependency {
        name: package_name.to_string(),
        req,
        tier: dep_tier,
        opt_in,
        source: crate::manifest::Source::Path(source_path.to_string()),
    };

    // Upsert into manifest text (preserves formatting, idempotent).
    let updated_manifest = match crate::manifest::upsert_dependency_text(
        manifest_content,
        "dependencies",
        &dep,
    ) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("noct add: failed to update manifest: {}", e);
            return 1;
        }
    };

    // Write manifest.
    if let Err(e) = fs::write(manifest_path, &updated_manifest) {
        eprintln!("noct add: cannot write {}: {}", manifest_path.display(), e);
        return 1;
    }

    // Build or update lockfile (path entry regenerated; anything else
    // rides through untouched).
    let lock_path = Path::new("nestpkg.lock");
    let mut lockfile = match fs::read_to_string(lock_path) {
        Ok(content) => match crate::manifest::parse_lockfile(&content) {
            Ok(lock) => lock,
            Err(_) => crate::manifest::Lockfile {
                lock_version: 1,
                packages: Vec::new(),
            },
        },
        Err(_) => crate::manifest::Lockfile {
            lock_version: 1,
            packages: Vec::new(),
        },
    };

    // Remove existing entry for this package (idempotent update).
    lockfile.packages.retain(|p| p.name != package_name);

    // Add the new locked package.
    lockfile.packages.push(crate::manifest::LockedPackage {
        name: package_name.to_string(),
        version: sibling_version,
        source: crate::manifest::Source::Path(source_path.to_string()),
        content: None,
        tier: dep_tier,
        signed_by: None,
        experimental: false,
    });

    let lock_text = crate::manifest::serialize_lockfile(&lockfile);
    if let Err(e) = fs::write(lock_path, &lock_text) {
        eprintln!("noct add: cannot write {}: {}", lock_path.display(), e);
        return 1;
    }

    println!(
        "Added dependency '{}' to {}",
        package_name,
        manifest_path.display()
    );
    0
}

// ── Registry mode ─────────────────────────────────────────────────────────────

/// `add <name>[@req] --index <dir>`: resolve the full registry
/// closure, verify + fetch every locked package (TOFU), then persist
/// manifest (upserted request) + lock (re-resolved, path entries
/// preserved). Nothing is written until fetch+verify succeed.
fn add_registry(
    manifest_content: &str,
    manifest_path: &Path,
    package_name: &str,
    explicit_req: Option<&str>,
    dep_tier: crate::manifest::Tier,
    opt_in: bool,
    index_dir: &str,
    rotate_key: bool,
) -> i32 {
    use crate::manifest::{Dependency, Source, Tier, VersionReq};
    use crate::registry::{fetch_locked, FileIndex, Registry};

    if let Err(message) = crate::manifest::validate_project_name(package_name) {
        eprintln!("noct add: invalid package name: {message}");
        return 1;
    }

    let index = FileIndex::new(PathBuf::from(index_dir));
    // Bare `add name` tracks the latest indexed version (caret).
    let req = match explicit_req {
        Some(v) => match VersionReq::parse(v, 0) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("noct add: invalid version '{v}': {e}");
                return 1;
            }
        },
        None => match index.versions(package_name).map(|mut vs| vs.pop()) {
            Ok(Some(latest)) => VersionReq::Caret(latest),
            _ => {
                eprintln!("noct add: unknown package `{package_name}` (index has no such package)");
                return 1;
            }
        },
    };

    // Resolve the closure with the request applied (in memory first).
    let mut manifest = match crate::manifest::parse_manifest(manifest_content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("noct add: invalid manifest: {e}");
            return 1;
        }
    };
    manifest.dependencies.retain(|d| d.name != package_name);
    manifest.dependencies.push(Dependency {
        name: package_name.to_string(),
        req,
        tier: dep_tier,
        opt_in,
        source: Source::Registry,
    });
    let resolved = match crate::registry::resolve(&manifest, &index) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("noct add: {e}");
            return 1;
        }
    };

    // Fetch + verify everything BEFORE writing anything.
    let project = Path::new(".");
    let keys = crate::registry::key_dir();
    for locked in &resolved {
        if let Err(e) = fetch_locked(locked, &index, project, &keys, rotate_key) {
            eprintln!("noct add: {e}");
            return 1;
        }
    }

    // Persist the request (byte-preserving upsert, idempotent).
    let rendered = Dependency {
        name: package_name.to_string(),
        req,
        tier: dep_tier,
        opt_in,
        source: Source::Registry,
    };
    let updated_manifest =
        match crate::manifest::upsert_dependency_text(manifest_content, "dependencies", &rendered) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("noct add: failed to update manifest: {e}");
                return 1;
            }
        };
    if let Err(e) = fs::write(manifest_path, &updated_manifest) {
        eprintln!("noct add: cannot write {}: {e}", manifest_path.display());
        return 1;
    }

    // Merge: resolved registry entries + still-required path entries,
    // canonical (name-sorted) order.
    let mut lockfile = match fs::read_to_string(Path::new("nestpkg.lock"))
        .ok()
        .and_then(|text| crate::manifest::parse_lockfile(&text).ok())
    {
        Some(lock) => lock,
        None => crate::manifest::Lockfile {
            lock_version: 1,
            packages: Vec::new(),
        },
    };
    let required_paths: Vec<String> = manifest
        .dependencies
        .iter()
        .filter_map(|d| match &d.source {
            Source::Path(_) => Some(d.name.clone()),
            _ => None,
        })
        .collect();
    lockfile.packages.retain(|p| {
        matches!(p.source, Source::Path(_)) && required_paths.contains(&p.name)
    });
    lockfile.packages.extend(resolved.iter().cloned());
    lockfile.packages.sort_by(|a, b| a.name.cmp(&b.name));
    for pkg in lockfile.packages.iter_mut() {
        if pkg.tier == Tier::CxxShim {
            pkg.experimental = true;
        }
    }
    if let Err(e) = fs::write(
        Path::new("nestpkg.lock"),
        &crate::manifest::serialize_lockfile(&lockfile),
    ) {
        eprintln!("noct add: cannot write nestpkg.lock: {e}");
        return 1;
    }

    println!(
        "Added dependency '{}' to {} ({} locked packages)",
        package_name,
        manifest_path.display(),
        lockfile.packages.len()
    );
    0
}
