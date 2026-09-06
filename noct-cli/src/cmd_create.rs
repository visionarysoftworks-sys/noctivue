//! `noct create` — scaffold a new Noctivue project.
//!
//! Implements the minimal slice of the project-creation spec: one
//! project model (no project types), manifest-driven, deterministic,
//! refusing to touch existing paths. What is NOT built here is exactly
//! what the spec defers: targets/capabilities sections (those systems
//! do not exist — the manifest holds no such keys), platform
//! directories (minimal stays minimal), templates composition,
//! `--force`, and Noctide wiring (which must reuse this same model
//! when it lands).
//!
//! Generated tree (`noct create <name>`, run in an empty directory):
//!
//! ```text
//! <name>/
//! ├── nestpkg.nvpm      (canonical `serialize_manifest` output)
//! ├── lib/
//! │   └── main.nv       (valid, immediately runnable)
//! ├── tests/
//! │   └── main_test.nv  (assert-based smoke, runnable via `run`;
//! !                      project-aware `test` discovery is future)
//! ├── docs/
//! │   └── overview.md
//! └── README.md
//! ```
//!
//! No lockfile is generated (package-manager design: the lock is
//! established by `add`/`build`, never invented empty). No
//! dependencies are added (`add` owns that). The standard library
//! needs no manifest entry (it is not a third-party dependency).
//!
//! Usage:
//!   noct create <project-name> [--dir <path>] [--force]

use std::path::PathBuf;

use crate::manifest::{parse_manifest, serialize_manifest, Manifest, Package, SemVer};

// ── entry point ────────────────────────────────────────────────────────────

/// Entry point called from `main.rs`.
pub fn run(args: &[String]) -> i32 {
    let mut dir = PathBuf::from(".");
    let mut force = false;
    let mut name: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--dir" {
            i += 1;
            if i >= args.len() {
                eprintln!("error: `--dir` requires a path argument");
                return 1;
            }
            dir = PathBuf::from(&args[i]);
        } else if let Some(p) = a.strip_prefix("--dir=") {
            dir = PathBuf::from(p);
        } else if a == "--force" || a == "-f" {
            force = true;
        } else if a.starts_with('-') {
            eprintln!("error: `noct create` does not recognize flag `{}`", a);
            eprintln!("usage: noct create <project-name> [--dir <path>] [--force]");
            return 1;
        } else {
            if name.is_none() {
                name = Some(a.as_str());
            } else {
                eprintln!("error: unexpected positional argument `{}`", a);
                eprintln!("usage: noct create <project-name> [--dir <path>] [--force]");
                return 1;
            }
        }
        i += 1;
    }

    let name = match name {
        Some(n) => n,
        None => {
            eprintln!("error: missing project name");
            eprintln!("usage: noct create <project-name> [--dir <path>] [--force]");
            return 1;
        }
    };

    // Single authoritative rule (create spec §4 — consumed, not redefined).
    if let Err(message) = crate::manifest::validate_project_name(name) {
        eprintln!("error: invalid project name: {message}");
        return 1;
    }

    let root = dir.join(name);
    if root.exists() {
        if force {
            std::fs::remove_dir_all(&root)
                .map_err(|e| {
                    eprintln!("error: cannot remove existing `{}`: {e}", root.display());
                    1
                })
                .ok();
        } else {
            eprintln!(
                "error: destination `{}` already exists (refusing to touch it; \
                 pass --force to overwrite)",
                root.display()
            );
            return 1;
        }
    }

    match scaffold(name, &root) {
        Ok(files) => {
            println!("created  {} ({} files)", root.display(), files);
            println!("run it:  noct run {}/lib/main.nv", root.display());
            0
        }
        Err(message) => {
            eprintln!("error: {message}");
            1
        }
    }
}

// ── scaffold ───────────────────────────────────────────────────────────────

/// Write the canonical tree. Steps are ordered manifest-first (create
/// spec §8: the manifest is authoritative, the filesystem materializes
/// it) and each failure names its step — a partial tree may remain and
/// the message says where it stopped.
fn scaffold(name: &str, root: &std::path::Path) -> Result<usize, String> {
    let manifest = Manifest {
        package: Package {
            name: name.to_string(),
            version: SemVer {
                major: 0,
                minor: 1,
                patch: 0,
            },
            description: Some("Created with `noct create`.".to_string()),
            authors: Vec::new(),
            license: None,
            edition: None,
        },
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
    };
    let files: &[(&str, String)] = &[
        ("nestpkg.nvpm", serialize_manifest(&manifest)),
        ("lib/main.nv", main_source(name)),
        ("tests/main_test.nv", test_source(name)),
        ("docs/overview.md", docs_source(name)),
        ("README.md", readme_source(name)),
    ];

    for dir in ["lib", "tests", "docs"] {
        std::fs::create_dir_all(root.join(dir))
            .map_err(|e| format!("cannot create `{}/{dir}`: {e}", root.display()))?;
    }
    let mut count = 0;
    for (rel, content) in files {
        std::fs::write(root.join(rel), content.as_bytes())
            .map_err(|e| format!("cannot write `{}/{rel}`: {e}", root.display()))?;
        count += 1;
    }
    // Self-check (create spec §26 "Validate Project"): the generated
    // manifest must parse back to itself — a generator that emits
    // unreadable manifests is broken by definition.
    let written = std::fs::read_to_string(root.join("nestpkg.nvpm"))
        .map_err(|e| format!("cannot re-read manifest: {e}"))?;
    let back =
        parse_manifest(&written).map_err(|e| format!("generated manifest does not parse: {e}"))?;
    if back != manifest {
        return Err("generated manifest does not round-trip (generator bug)".to_string());
    }
    Ok(count)
}

// ── source generators ──────────────────────────────────────────────────────

/// Minimal valid entry point: runs under `run`/`build`/`test` today
/// (plain `println`, no stdlib imports needed — the prelude covers it).
fn main_source(name: &str) -> String {
    format!(
        "//! {name} — created by `noct create`.\nmain():\n    println(\"Hello from {name}!\")\n"
    )
}

/// Minimal assert-based smoke. Runnable today via
/// `noct run tests/main_test.nv`; project-aware `test` discovery
/// (running a project's own `tests/` dir) does not exist yet, so the
/// file documents its own invocation instead of pretending otherwise.
fn test_source(name: &str) -> String {
    format!(
        "//! Smoke test for {name} (run: noct run tests/main_test.nv).\nfn double(x: Int) -> Int:\n    x * 2\n\nmain():\n    assert(double(21) == 42, \"double works\")\n    println(\"tests ok\")\n"
    )
}

fn docs_source(name: &str) -> String {
    format!("# {name} — overview\n\nShort project documentation lives here.\n")
}

fn readme_source(name: &str) -> String {
    format!("# {name}\n\nCreated with `noct create`.\n\nRun it:\n\n    noct run lib/main.nv\n")
}
