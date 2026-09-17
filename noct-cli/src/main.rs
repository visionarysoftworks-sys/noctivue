//! `noct` — the Noctivue command-line toolchain.
//!
//! Commands (TOOLCHAIN.md §2):
//!
//! | Command               | Phase | Status       |
//! |-----------------------|-------|--------------|
//! | `noct run`            | 1     | stub         |
//! | `noct test`           | 1     | stub         |
//! | `noct ast`            | 1     | stub         |
//! | `noct diagnostics`    | 1     | stub         |
//! | `noct build`          | 3     | straight-line only |
//! | `noct create`         | 4     | scaffolding with --dir |
//! | `noct fmt`            | 4     | v1 trivia canonicalizer |
//! | `noct lint`           | 4     | L-001 default-on |
//! | `noct add`            | 4     | path + registry (--index) |
//! | `noct audit`          | 4     | trust rows from lock |
//! | `noct publish`        | 4     | --dry-run only (no registry) |
//! | `noct doc`            | 4     | stdout per file |
//! | `noct vendor`         | 4     | offline copy of the lock closure |
//! | `noct outdated`       | 4     | index comparison, informational |
//! | `noct completions`    | 4     | shell stub (commands + files) |

mod cmd_ast;
mod cmd_build;
mod cmd_create;
mod cmd_diagnostics;
mod cmd_doc;
mod cmd_fmt;
mod cmd_lint;
mod cmd_add;
mod cmd_audit;
mod cmd_completions;
mod cmd_outdated;
mod cmd_publish;
mod cmd_vendor;
mod cmd_run;
mod cmd_run_vm;
mod cmd_test;
mod fmt_v2;
mod manifest;
mod registry;

use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let exit_code = match args.get(1).map(String::as_str) {
        Some("run")         => cmd_run::run(&args[2..]),
        Some("test")        => cmd_test::run(&args[2..]),
        Some("ast")         => cmd_ast::run(&args[2..]),
        Some("diagnostics") => cmd_diagnostics::run(&args[2..]),
        Some("build")       => cmd_build::run(&args[2..]),
        Some("create")      => cmd_create::run(&args[2..]),
        Some("fmt")         => cmd_fmt::run(&args[2..]),
        Some("lint")        => cmd_lint::run(&args[2..]),
        Some("add")         => cmd_add::run(&args[2..]),
        Some("audit")       => cmd_audit::run(&args[2..]),
        Some("publish")     => cmd_publish::run(&args[2..]),
        Some("doc")         => cmd_doc::run(&args[2..]),
        Some("vendor")      => cmd_vendor::run(&args[2..]),
        Some("outdated")    => cmd_outdated::run(&args[2..]),
        Some("completions") => cmd_completions::run(&args[2..]),
        Some("run-vm")      => cmd_run_vm::run(&args[2..]),
        Some("--version") | Some("-V") => {
            println!("noct {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            0
        }
        Some(unknown) => {
            eprintln!("error: unknown command `{unknown}`\n");
            print_help();
            1
        }
    };

    process::exit(exit_code);
}

fn print_help() {
    println!(
        "noct {version} — the Noctivue toolchain

USAGE:
    noct <command> [options]

COMMANDS:
    run          [Phase 1] Build and execute a .nv program (tree-walking interpreter)
    run-vm       [Phase 2] Build and execute a .nv program via NIR bytecode VM
    test         [Phase 1] Run the test suite
    ast          [Phase 1] Dump the AST as JSON (noct ast --json)
    diagnostics  [Phase 1] Dump compiler diagnostics as JSON
    build        [Phase 3] Compile to a native binary
    create       [Phase 4] Scaffold a new project
    fmt          [Phase 4] Run the official formatter
    lint         [Phase 4] Run the official linter
    add          [Phase 4] Add a dependency
    audit        [Phase 4] Show dependency trust rows
    publish      [Phase 4] Publish to the package registry
    doc          [Phase 4] Generate documentation
    vendor       [Phase 4] Copy the dependency closure into vendor/
    outdated     [Phase 4] Show newer indexed versions (informational)
    completions  [Phase 4] Print a shell completion stub

OPTIONS:
    -h, --help     Print this help message
    -V, --version  Print the toolchain version
",
        version = env!("CARGO_PKG_VERSION")
    );
}
