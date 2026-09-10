//! CLI tests for env/config/log behavior (Phase 5, items 4-5).
//!
//! - `noct run` auto-loads `./*.nv.env` (ADR-017), process wins.
//! - `log_*` sink selection + level filtering (stderr default,
//!   `NOCT_LOG_SINK=stdout` pinnable, `NOCT_LOG` minimum).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn noct_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_noct"))
}

const CASE_TIMEOUT_SECS: u64 = 15;

fn run_cli_in_env(dir: &Path, extra_env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(noct_bin());
    cmd.args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().expect("failed to spawn noct binary");
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

fn scratch() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("noctivue-envcli-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

fn libs() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let files = [
        "stdlib/core/convert.nv",
        "stdlib/core/compare.nv",
        "stdlib/option/option.nv",
        "stdlib/result/result.nv",
        "stdlib/strings/string.nv",
        "stdlib/strings/format.nv",
        "stdlib/testing/assertions.nv",
        "stdlib/collections/list.nv",
        "stdlib/env/variables.nv",
        "stdlib/env/config.nv",
        "stdlib/env/dotenv.nv",
        "stdlib/log/log.nv",
    ];
    let mut out = String::new();
    for f in files {
        out.push_str(&std::fs::read_to_string(root.join(f)).expect("read lib"));
        out.push('\n');
    }
    out
}

// ── Cases ───────────────────────────────────────────────────────────────────

#[test]
fn run_autoloads_dotenv_with_process_wins() {
    let dir = scratch();
    std::fs::write(
        dir.join("app.nv"),
        libs() + "fn main():\n    println(config_or(\"GREETING\", \"missing\"))\n",
    )
    .unwrap();
    std::fs::write(dir.join("app.nv.env"), "# comment\nGREETING=hello\n").unwrap();

    // Auto-loaded from the file.
    let out = run_cli_in_env(&dir, &[], &["run", "app.nv"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\n");

    // Process environment wins over the file.
    let out = run_cli_in_env(&dir, &[("GREETING", "outer")], &["run", "app.nv"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "outer\n");

    // Malformed lines warn loud and are skipped.
    std::fs::write(
        dir.join("app.nv.env"),
        "GREETING=hi\nMALFORMED LINE\n=noname\n",
    )
    .unwrap();
    let out = run_cli_in_env(&dir, &[], &["run", "app.nv"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ignoring malformed"),
        "got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    cleanup(&dir);
}

#[test]
fn log_sink_and_level_filter() {
    let dir = scratch();
    std::fs::write(
        dir.join("app.nv"),
        libs()
            + "fn main():\n    log_debug(\"dbg\")\n    log_info(\"inf\")\n    log_warn(\"wrn\")\n",
    )
    .unwrap();

    // Stdout sink + DEBUG minimum: all three lines, exact bytes.
    let out = run_cli_in_env(
        &dir,
        &[("NOCT_LOG_SINK", "stdout"), ("NOCT_LOG", "DEBUG")],
        &["run", "app.nv"],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "[DEBUG] dbg\n[INFO] inf\n[WARN] wrn\n"
    );

    // Default INFO minimum: debug filtered, stderr stays the sink.
    let out = run_cli_in_env(&dir, &[("NOCT_LOG_SINK", "stdout")], &["run", "app.nv"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "[INFO] inf\n[WARN] wrn\n"
    );
    cleanup(&dir);
}
