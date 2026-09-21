//! `marrow run`: the text rendering of a returned `Result`, the one diagnostic form,
//! and the command-line surface every subcommand shares — usage on `--help`, and one
//! usage-error form.

use crate::common::{MARROW_BIN, Project, marrow_in, stage_toolchain, staged_marrow_in, write};
use marrow_test_support::Scratch;

const HALF: &str = "\
pub fn half(n: int): Result<int, string> {
    if n % 2 == 1 {
        return err(\"odd\")
    }
    return ok(n / 2)
}

pub fn wrapped(n: int): Option<Result<int, string>> {
    return some(half(n))
}
";

/// A top-level `ok(v)` renders as `v` alone and exits 0. A top-level `err(e)` is the
/// program's own failure report: `error: <e>` on standard error, nothing on standard
/// output, exit 1. JSONL keeps the exact `Result` record in both cases, and only the
/// exit status tells them apart.
#[test]
fn a_top_level_result_unwraps_in_text_and_an_err_exits_one() {
    let workspace = Project::single(HALF).materialize("top-level-result");

    let ok = workspace.marrow(&["run", "half", "--", "4"]);
    assert_eq!(ok.code(), Some(0), "{ok:?}");
    assert_eq!(ok.stdout_text(), "2\n");
    assert!(ok.stderr.is_empty(), "{ok:?}");

    let err = workspace.marrow(&["run", "half", "--", "3"]);
    assert_eq!(err.code(), Some(1), "{err:?}");
    assert!(err.stdout.is_empty(), "{err:?}");
    assert_eq!(err.stderr_text(), "error: odd\n");

    let jsonl = workspace.marrow(&["run", "half", "--format", "jsonl", "--", "3"]);
    assert_eq!(jsonl.code(), Some(1), "{jsonl:?}");
    assert_eq!(
        jsonl.stdout_text(),
        "{\"data\":{\"enum\":\"Result\",\"member\":\"err\",\"payload\":[\"odd\"]},\"kind\":\"run\",\"outcome\":\"value\"}\n"
    );
    assert!(jsonl.stderr.is_empty(), "{jsonl:?}");
}

/// Only the outermost value is unwrapped: a `Result` nested in another value keeps its
/// constructor spelling and is an ordinary value, exit 0.
#[test]
fn a_nested_result_keeps_its_constructor_spelling() {
    let output = Project::single(HALF).run_cli("nested-result", &["run", "wrapped", "--", "3"]);
    assert_eq!(output.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout_text(), "Option::some(Result::err(odd))\n");
}

/// A source diagnostic prints as `file:line:column: code: message`, the form `check`
/// prints, whichever command reports it: `run` and `test` on standard output, `fmt`
/// on standard error, byte for byte the line `check` prints for the same source.
#[test]
fn run_test_and_fmt_print_a_diagnostic_in_the_one_form() {
    let workspace = Project::single("pub fn oops(): int {\n    return \"nope\"\n}\n")
        .materialize("diagnostic-form");
    for args in [["run", "oops"].as_slice(), ["test"].as_slice()] {
        let output = workspace.marrow(args);
        assert_eq!(output.code(), Some(1), "{output:?}");
        let stdout = output.stdout_text();
        assert!(
            stdout.starts_with("src/main.mw:2:12: check.type: "),
            "{args:?}: {stdout}"
        );
        assert_eq!(stdout.lines().count(), 1, "{args:?}: {stdout}");
    }

    let unparsable = Project::single("pub fn oops(): int {\n    return @@@\n}\n")
        .materialize("parse-diagnostic-form");
    let checked = unparsable.marrow(&["check"]);
    let formatted = unparsable.marrow(&["fmt", "--check", "."]);
    assert_eq!(formatted.code(), Some(1), "{formatted:?}");
    let first = formatted.stderr_text().lines().next().map(str::to_string);
    assert!(
        first.as_deref().is_some_and(
            |line| line.starts_with("src/main.mw:2:") && line.contains(": parse.syntax: ")
        ),
        "{formatted:?}"
    );
    assert_eq!(
        first.as_deref(),
        checked.stderr_text().lines().next(),
        "fmt and check print the same line for the same parse error"
    );
}

const COMMANDS: [&str; 13] = [
    "init", "fmt", "check", "run", "import", "doctor", "apply", "recover", "backup", "restore",
    "test", "client", "image",
];

/// Every subcommand accepts `--help` and `-h` before its own options and prints its
/// usage on standard output with exit 0, without reading a project.
#[test]
fn every_subcommand_prints_its_usage_on_help() {
    let empty = Scratch::new("help");
    for command in COMMANDS {
        for flag in ["--help", "-h"] {
            let output = marrow_in(&empty, &[command, flag]);
            assert_eq!(output.code(), Some(0), "{command} {flag}: {output:?}");
            assert!(output.stderr.is_empty(), "{command} {flag}: {output:?}");
            let stdout = output.stdout_text();
            assert!(
                stdout.starts_with(&format!("Usage:\n  marrow {command}")),
                "{command} {flag}: {stdout}"
            );
        }
    }
    let help = marrow_in(&empty, &["client", "typescript", "--help"]);
    assert_eq!(help.code(), Some(0), "{help:?}");
}

/// After `--`, `--help` is an argument to the export, not a request for usage.
#[test]
fn help_after_the_separator_is_a_positional_argument() {
    let output = Project::single("pub fn echo(s: string): string {\n    return s\n}\n")
        .run_cli("help-positional", &["run", "echo", "--", "--help"]);
    assert_eq!(output.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout_text(), "--help\n");
}

/// One usage-error form for every command: the problem, then the command's own help,
/// exit 2 — an unknown option, a flag without its value, a repeated flag, an unknown
/// format, and a missing required flag.
#[test]
fn usage_errors_name_the_problem_and_the_commands_help() {
    let empty = Scratch::new("usage");
    let cases: [(&[&str], &str); 6] = [
        (
            &["run", "--bogus"],
            "unknown option `--bogus`; run marrow run --help for usage\n",
        ),
        (
            &["doctor", "--store"],
            "`--store` needs a value; run marrow doctor --help for usage\n",
        ),
        (
            &["doctor", "--store", "a", "--store", "b"],
            "marrow doctor takes one `--store`; run marrow doctor --help for usage\n",
        ),
        (
            &["test", "--format", "yaml"],
            "`--format` must be `text` or `jsonl`; run marrow test --help for usage\n",
        ),
        (
            &["import", "--jsonl", "rows.jsonl", "--root", "notes"],
            "`--store` must name the store directory; run marrow import --help for usage\n",
        ),
        (
            &["check", "a", "b"],
            "marrow check takes one project directory; run marrow check --help for usage\n",
        ),
    ];
    for (args, expected) in cases {
        let output = marrow_in(&empty, args);
        assert_eq!(output.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert_eq!(output.stderr_text(), expected, "{args:?}");
    }
}

/// A flag given twice is a usage error for every command: exit 2, nothing on standard
/// output, one rule rather than a silent last-value-wins.
#[test]
fn a_repeated_flag_is_refused() {
    let workspace =
        Project::single("test \"t\" {\n    assert true\n}\n").materialize("repeated-flag");
    let cases: [&[&str]; 4] = [
        &["test", "--format", "text", "--format", "jsonl"],
        &["test", "--filter", "t", "--filter", "t"],
        &["run", "main", "--store", "a", "--store", "b"],
        &[
            "import", "--store", "s", "--jsonl", "a", "--jsonl", "b", "--root", "r",
        ],
    ];
    for args in cases {
        let output = workspace.marrow(args);
        assert_eq!(output.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(
            output.stderr_text().contains("takes one `--"),
            "{args:?}: {output:?}"
        );
    }
}

/// Usage is answered before the arguments are decoded as text: `--help` beside an
/// argument that is not UTF-8 still prints the usage, while the same argument after
/// `--` is the export's and is refused as undecodable.
#[cfg(unix)]
#[test]
fn help_is_recognized_before_arguments_are_decoded_as_text() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::process::Command;

    let undecodable = OsStr::from_bytes(b"\xff");
    let help = Command::new(MARROW_BIN)
        .args([OsStr::new("check"), undecodable, OsStr::new("--help")])
        .output()
        .expect("run marrow binary");
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    assert!(
        String::from_utf8_lossy(&help.stdout).starts_with("Usage:\n  marrow check"),
        "{help:?}"
    );

    let positional = Command::new(MARROW_BIN)
        .args([
            OsStr::new("run"),
            OsStr::new("echo"),
            OsStr::new("--"),
            undecodable,
            OsStr::new("--help"),
        ])
        .output()
        .expect("run marrow binary");
    assert_eq!(positional.status.code(), Some(1), "{positional:?}");
    assert!(
        String::from_utf8_lossy(&positional.stderr).starts_with("config.invalid: "),
        "{positional:?}"
    );
}

const COUNTER_SOURCE: &str = "\
resource Counter {
    required value: int
}

store ^counters[id: int]: Counter

pub fn setOdd(id: int, v: int): Result<int, string> {
    transaction {
        ^counters[id] = Counter(value: v)
        if v % 2 == 1 {
            return err(\"odd\")
        }
        return ok(v)
    }
}

pub fn divide(id: int, d: int): int {
    transaction {
        ^counters[id] = Counter(value: 99)
        return 10 / d
    }
}

pub fn valueOf(id: int): int? {
    return ^counters[id].value
}
";

const COUNTER_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A durable export whose transaction block exits with `err` has committed: the run
/// reports `error: e` on standard error and exits 1, the written value is readable
/// afterwards, and JSONL carries the exact `value` record with `member` `err`. A fault
/// inside the block is the contrast: a `fault` record, and the write is discarded.
#[test]
fn a_durable_top_level_err_commits_and_exits_one() {
    let toolchain = stage_toolchain();
    let temp = Scratch::new("durable-err");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), COUNTER_SOURCE);
    write(&project.join(".marrow/ids"), COUNTER_IDS);
    write(&project.join("seed.jsonl"), "{\"id\":1,\"value\":0}\n");
    let store = temp.join("store");
    let store = store.to_str().expect("store path");
    let imported = staged_marrow_in(
        &toolchain,
        &project,
        &[
            "import",
            "--store",
            store,
            "--jsonl",
            "seed.jsonl",
            "--root",
            "counters",
            "--keys",
            "id",
        ],
    );
    assert!(imported.success(), "{}", imported.stderr_text());
    let run = |args: &[&str]| staged_marrow_in(&toolchain, &project, args);

    let err = run(&["run", "setOdd", "--store", store, "--", "1", "3"]);
    assert_eq!(err.code(), Some(1), "{err:?}");
    assert!(err.stdout.is_empty(), "{err:?}");
    assert_eq!(err.stderr_text(), "error: odd\n");
    let committed = run(&["run", "valueOf", "--store", store, "--", "1"]);
    assert_eq!(committed.stdout_text(), "3\n", "{committed:?}");

    let jsonl = run(&[
        "run", "setOdd", "--store", store, "--format", "jsonl", "--", "1", "5",
    ]);
    assert_eq!(jsonl.code(), Some(1), "{jsonl:?}");
    assert_eq!(
        jsonl.stdout_text(),
        "{\"data\":{\"enum\":\"Result\",\"member\":\"err\",\"payload\":[\"odd\"]},\"kind\":\"run\",\"outcome\":\"value\"}\n"
    );

    let fault = run(&[
        "run", "divide", "--store", store, "--format", "jsonl", "--", "1", "0",
    ]);
    assert_eq!(fault.code(), Some(1), "{fault:?}");
    assert!(
        fault.stdout_text().contains("\"outcome\":\"fault\""),
        "{fault:?}"
    );
    let discarded = run(&["run", "valueOf", "--store", store, "--", "1"]);
    assert_eq!(discarded.stdout_text(), "5\n", "{discarded:?}");
}
