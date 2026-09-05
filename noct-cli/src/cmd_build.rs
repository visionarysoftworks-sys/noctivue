//! `noct build` — compile a .nv program to a native binary (Phase 3).
//!
//! Usage:
//!   noct build <file.nv> [-o <out.exe>] [--release]
//!
//! ## Strategy: cargo piggyback (NOT raw link.exe)
//!
//! Linking a Cranelift-emitted COFF object against `runtime-native` (a Rust
//! crate using std) with raw `link.exe` would require re-resolving the
//! Rust std staticlibs, the MSVC/SDK lib paths, and the CRT entry — all of
//! which cargo already knows. So `noct build` generates a tiny shim crate
//! in a temp dir and lets `cargo build` do the final link:
//!
//! ```text
//! <staging>/module.obj   <- compile_to_object (this process)
//! <staging>/Cargo.toml   <- bin crate + `runtime-native` path dependency
//! <staging>/src/main.rs  <- extern `noctivue_main`; exit(code)
//! <staging>/build.rs     <- cargo:rustc-link-arg=<staging>/module.obj
//! ```
//!
//! The shim's `main` calls the exported `noctivue_main` (the driver's
//! entry convention: `() -> ()`, exit code always 0 on return — matching
//! the interpreter) and exits. `runtime-native` arrives as a normal rlib
//! path dependency, so its symbols resolve like any Rust dependency.
//!
//! Consequences, stated plainly:
//! - `cargo` must be on PATH (or `CARGO` set) — a toolchain requirement,
//!   reported loudly, not a silent fallback to anything else.
//! - `runtime-native`'s source must sit next to this crate's source at
//!   `../runtime-native` (compile-time `CARGO_MANIFEST_DIR`-relative). True
//!   for every checkout build; registry installs are a post-M5 problem.
//! - First build compiles `runtime-native` (~tens of seconds); later builds
//!   reuse the shared target cache below and take seconds.
//! - The staging dir is deleted on success but KEPT on failure, with its
//!   path printed, so a broken link is debuggable instead of vanishing.

use compiler::backends::cranelift::driver::compile_to_object;
use compiler::diagnostics::DiagnosticSink;
use compiler::{lexer, parser, resolver, typeck};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Cargo target cache shared across ALL `noct build` invocations on this
/// machine: `<temp>/noctivue-target-cache`. Without this, every build
/// would recompile `runtime-native` from scratch (fresh staging dir =
/// fresh fingerprints). Cargo's target-dir locking serializes concurrent
/// builds safely; stale cache is cargo's own invalidation problem, and
/// cargo is good at it.
fn shared_target_dir() -> PathBuf {
    std::env::temp_dir().join("noctivue-target-cache")
}

/// Absolute path to the `runtime-native` crate source, baked at compile
/// time from this crate's manifest location. `canonicalize` normalizes the
/// `..` for the generated TOML (a literal `../` inside the temp crate's
/// manifest would resolve against the TEMP dir, not here — the classic
/// mistake; absolutize at the source).
fn runtime_native_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("runtime-native");
    match dir.canonicalize() {
        Ok(abs) => abs,
        Err(_) => dir,
    }
}

fn usage() {
    eprintln!("usage: noct build <file.nv>... [-o <out.exe>] [--release]");
    eprintln!("  multiple files concatenate in order, entry point last (tank libraries first)");
}

pub fn run(args: &[String]) -> i32 {
    let mut inputs: Vec<&str> = Vec::new();
    let mut out: Option<&str> = None;
    let mut release = false;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--out" => match it.next() {
                Some(v) => out = Some(v.as_str()),
                None => {
                    eprintln!("noct build: {a} needs a value");
                    usage();
                    return 1;
                }
            },
            "--release" => release = true,
            "-h" | "--help" => {
                usage();
                return 0;
            }
            other if other.starts_with('-') => {
                eprintln!("noct build: unknown flag `{other}`");
                usage();
                return 1;
            }
            other => {
                inputs.push(other);
            }
        }
    }
    if inputs.is_empty() {
        usage();
        return 1;
    }

    // ── 1. Frontend → NIR (same front door as `run-vm`) ────────────────
    // Multi-file like `run`/`run-vm`: libraries first, entry last.
    let (source, files) = match crate::cmd_run::read_sources(&inputs) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut sink = DiagnosticSink::new();
    let tokens = lexer::lex(&source, &mut sink);
    let program = parser::parse(&tokens, &mut sink);
    let program = resolver::resolve(program, &mut sink);
    let module = typeck::typecheck(program, &mut sink);
    if sink.has_errors() {
        crate::cmd_run::print_diagnostics_multi(&files, &sink);
        return 1;
    }
    let nir_module = compiler::nir::lowering::lower(module);

    // ── 2. Staging dir + object emission ───────────────────────────────
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let staging = std::env::temp_dir().join(format!(
        "noctivue-build-{}-{id}",
        std::process::id()
    ));
    if let Err(e) = std::fs::create_dir_all(staging.join("src")) {
        eprintln!("noct build: cannot create staging dir {}: {e}", staging.display());
        return 1;
    }
    // `.obj`, not `.o`: the bytes are COFF either way, but link.exe (via
    // rustc-link-arg) treats `.obj` as an object input unconditionally,
    // while `.o` relies on content sniffing.
    let obj_path = staging.join("module.obj");
    if let Err(e) = compile_to_object(&nir_module, &obj_path) {
        eprintln!("noct build: codegen failed: {e}");
        let _ = std::fs::remove_dir_all(&staging);
        return 1;
    }

    // ── 3. Shim crate generation ───────────────────────────────────────
    if let Err(e) = write_shim(&staging, &obj_path) {
        eprintln!("noct build: cannot write shim crate: {e}");
        let _ = std::fs::remove_dir_all(&staging);
        return 1;
    }

    // ── 4. Cargo link ──────────────────────────────────────────────────
    // Stdio inherited: a first-time `runtime-native` compile takes a
    // while, and live cargo progress beats a hung-looking silence.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let profile: &[&str] = if release { &["--release"] } else { &[] };
    let status = std::process::Command::new(&cargo)
        .arg("build")
        .arg("--quiet")
        .args(profile)
        .current_dir(&staging)
        .env("CARGO_TARGET_DIR", shared_target_dir())
        .env("CARGO_TERM_COLOR", "never")
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("noct build: cargo link failed (exit {s}) — staging kept at {}", staging.display());
            return 1;
        }
        Err(e) => {
            eprintln!("noct build: cannot run `{cargo} build`: {e} (is cargo on PATH?)");
            return 1;
        }
    }

    // ── 5. Deliver the binary ──────────────────────────────────────────
    let built_exe = shared_target_dir()
        .join(if release { "release" } else { "debug" })
        .join(format!("noctivue_shim{}", std::env::consts::EXE_SUFFIX));
    let out_path = match out {
        // Mirror rustc: `-o foo` on Windows means `foo.exe`.
        Some(o) => ensure_exe_suffix(&PathBuf::from(o)),
        None => {
            // Default output takes the ENTRY file's stem (multi-file
            // convention: libraries first, entry last — `inputs` is
            // non-empty here, checked above).
            let stem = Path::new(inputs.last().expect("inputs non-empty"))
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "a".to_string());
            PathBuf::from(format!("{stem}{}", std::env::consts::EXE_SUFFIX))
        }
    };
    match std::fs::copy(&built_exe, &out_path) {
        Ok(n) => {
            println!("Built {} ({n} bytes)", out_path.display());
        }
        Err(e) => {
            eprintln!(
                "noct build: cannot copy {} to {}: {e}",
                built_exe.display(),
                out_path.display()
            );
            return 1;
        }
    }

    // Staging sources served their purpose; the shared target cache (real
    // build time) lives elsewhere and is intentionally kept.
    let _ = std::fs::remove_dir_all(&staging);
    0
}

fn ensure_exe_suffix(p: &Path) -> PathBuf {
    if std::env::consts::EXE_SUFFIX.is_empty() {
        return p.to_path_buf();
    }
    match p.extension() {
        Some(_) => p.to_path_buf(),
        None => p.with_extension(std::env::consts::EXE_SUFFIX.trim_start_matches('.')),
    }
}

fn write_shim(staging: &Path, obj_path: &Path) -> std::io::Result<()> {
    let runtime_dir = runtime_native_dir();
    // TOML needs `\\` on Windows — `to_string_lossy` gives single
    // backslashes, which TOML would read as escapes. Debug-escape the path
    // (`{:?}`) to get valid TOML string syntax on every platform.
    let manifest = format!(
        "[package]\nname = \"noctivue_shim\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nruntime-native = {{ path = {:?} }}\n",
        runtime_dir.to_string_lossy().into_owned()
    );
    let mut f = std::fs::File::create(staging.join("Cargo.toml"))?;
    f.write_all(manifest.as_bytes())?;

    let mut f = std::fs::File::create(staging.join("src").join("main.rs"))?;
    f.write_all(
        b"// GENERATED by `noct build` - do not edit.\n\
          \n\
          // The Cranelift object (linked via build.rs) calls the runtime's\n\
          // extern-C symbols, but the linker only pulls archive members the\n\
          // Rust code references - an unreferenced rlib dependency never\n\
          // reaches the link line (LNK2001 on every rt symbol). This rooted\n\
          // static forces the reference; `#[used]` keeps the linker from\n\
          // collecting it as dead. Any one symbol roots the whole object\n\
          // (single-object rlib), resolving all twelve imports. (A plain\n\
          // `as usize` cast would read better but is illegal in const\n\
          // context; a fn-pointer-typed static is the const-legal form.)\n\
          #[used]\n\
          static ROOT_RUNTIME: unsafe extern \"C\" fn(*const u8) =\n\
              runtime_native::noctivue_rt_print;\n\
          // Calls the exported `noctivue_main` (driver entry convention:\n\
          // `() -> ()`, matching the interpreter's ignore-the-return rule)\n\
          // and exits 0 on return. Traps never return: the runtime prints\n\
          // and exits nonzero itself.\n\
          extern \"C\" {\n    fn noctivue_main();\n}\n\
          fn main() {\n    unsafe { noctivue_main() };\n}\n",
    )?;

    // Absolute object path: the build script runs with CWD = staging, but
    // absolutize anyway — relative link-args break the moment cargo wraps
    // the invocation. The `{{}}` renders a literal `{}` into the GENERATED
    // build.rs (NOT `{:?}`: cargo passes `rustc-link-arg` values to the
    // linker verbatim, so quotes from `{:?}` would become part of the
    // filename and link.exe would fail with LNK1104 — learned the hard
    // way). The outer `{:?}` still emits the path as a quoted Rust string
    // literal in the generated source, which is correct there.
    let mut f = std::fs::File::create(staging.join("build.rs"))?;
    write!(
        f,
        "fn main() {{\n    println!(\"cargo:rerun-if-changed=build.rs\");\n    println!(\"cargo:rustc-link-arg={{}}\", {:?});\n}}\n",
        obj_path.to_string_lossy().into_owned(),
    )?;
    Ok(())
}
