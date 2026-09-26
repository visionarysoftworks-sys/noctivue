//! `noct completions <shell>` — print a shell completion stub to stdout.
//!
//! [Phase 4] Honest and small on purpose: the stubs complete command
//! names plus file/path arguments only. Per-command flags are NOT
//! completed (claiming otherwise would be a lie — the CLI parses flags
//! per command in Rust, and the stubs do not replicate that table).
//!
//! Usage:
//!   noct completions <bash|elvish|fish|powershell|zsh>

const SUPPORTED: &str = "bash, elvish, fish, powershell, zsh";

pub fn run(args: &[String]) -> i32 {
    let mut shell: Option<&str> = None;
    for arg in args {
        if arg == "--help" || arg == "-h" {
            print_usage();
            return 0;
        }
        if arg.starts_with('-') {
            eprintln!("noct completions: unknown flag `{arg}` (usage: noct completions <shell>)");
            eprintln!("supported shells: {SUPPORTED}");
            return 1;
        }
        if shell.is_none() {
            shell = Some(arg.as_str());
        } else {
            eprintln!("noct completions: too many arguments (usage: noct completions <shell>)");
            return 1;
        }
    }
    let Some(shell) = shell else {
        eprintln!("noct completions: shell name required (usage: noct completions <shell>)");
        eprintln!("supported shells: {SUPPORTED}");
        return 1;
    };
    match shell {
        "bash" => print!("{BASH_STUB}"),
        "zsh" => print!("{ZSH_STUB}"),
        "fish" => print!("{FISH_STUB}"),
        "powershell" => print!("{POWERSHELL_STUB}"),
        "elvish" => print!("{ELVISH_STUB}"),
        other => {
            eprintln!("noct completions: unsupported shell `{other}` (usage: noct completions <shell>)");
            eprintln!("supported shells: {SUPPORTED}");
            return 1;
        }
    }
    0
}

fn print_usage() {
    println!("usage: noct completions <shell>");
    println!();
    println!("Print a shell completion stub to stdout.");
    println!("supported shells: {SUPPORTED}");
    println!();
    println!("The stubs complete command names and file paths only;");
    println!("per-command flags are not completed.");
}

const BASH_STUB: &str = r#"# noct shell completion (bash).
# Completes command names and file paths only; per-command flags are not completed.
_noct_complete() {
    local cur cmds
    cmds="run run-vm test ast diagnostics build create fmt lint add audit publish doc vendor outdated completions"
    cur="${COMP_WORDS[COMP_CWORD]}"
    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=($(compgen -W "$cmds" -- "$cur"))
    else
        COMPREPLY=($(compgen -f -- "$cur"))
    fi
}
complete -F _noct_complete -o filenames noct
"#;

const ZSH_STUB: &str = r#"#compdef noct
# noct shell completion (zsh).
# Completes command names and file paths only; per-command flags are not completed.
_noct() {
    local -a cmds
    cmds=(run run-vm test ast diagnostics build create fmt lint add audit publish doc vendor outdated completions)
    if (( CURRENT == 2 )); then
        _describe 'noct command' cmds
    else
        _files
    fi
}
compdef _noct noct
"#;

const FISH_STUB: &str = r#"# noct shell completion (fish).
# Completes command names and file paths only; per-command flags are not completed.
complete -c noct -f -n '__fish_use_subcommand' -a "run run-vm test ast diagnostics build create fmt lint add audit publish doc vendor outdated completions"
complete -c noct -F -n 'not __fish_use_subcommand'
"#;

const POWERSHELL_STUB: &str = r#"# noct shell completion (powershell).
# Completes command names and file paths only; per-command flags are not completed.
# (First-argument command names are completed below; later arguments fall
# through to the shell's default filename completion.)
Register-ArgumentCompleter -Native -CommandName noct -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $cmds = @('run','run-vm','test','ast','diagnostics','build','create','fmt','lint','add','audit','publish','doc','vendor','outdated','completions')
    if ($commandAst.ToString() -match '^noct\s+\S*\s*$') {
        $cmds | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
        }
    }
}
"#;

const ELVISH_STUB: &str = r#"# noct shell completion (elvish).
# Completes command names and file paths only; per-command flags are not completed.
set edit:completion:arg-completer[noct] = {|@words|
    var cmds = [run run-vm test ast diagnostics build create fmt lint add audit publish doc vendor outdated completions]
    if (== (count $words) 2) {
        put $@cmds
    } else {
        edit:complete-filename $words[-1]
    }
}
"#;
