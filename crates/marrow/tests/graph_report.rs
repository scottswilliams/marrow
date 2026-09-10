//! End-to-end Graph Report dogfood tests (P02a): a storeless `.mw` program over the
//! final procedural surface (records/enums, the text/collection/generic floor, nested
//! loops, in-source `test`s) travels the real production path through the built binary.
//! The `graph_report` conformance fixture's in-source `test`s run under `marrow test`,
//! and the frozen `report(Text)` export runs under `marrow run` with a multiline input.

mod common;

use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;

use common::Project;

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

/// The frozen `report` output for the canonical rooted chain input `-> a / a -> b /
/// b -> c`, as one multiline UTF-8 text. `marrow run` prints this followed by a
/// newline in text form and carries it verbatim as the `data` field in JSONL.
const CHAIN_REPORT: &str = "Graph Report\n\
     nodes=3 edges=2 root=a malformed=0\n\
     -- degrees --\n\
     \x20 a out=1 in=0 role=source\n\
     \x20 b out=1 in=1 role=internal\n\
     \x20 c out=0 in=1 role=sink\n\
     -- reachable --\n\
     \x20 from a: a, b, c (3/3)\n\
     -- order --\n\
     \x20 a\n  b\n  c\n\
     -- cycle --\n\
     \x20 none";

fn conformance_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance")
        .join(name)
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

/// The Graph Report fixture's in-source `test`s pass end to end: parsing a line-based
/// directed-graph encoding, per-node degree/role classification, a bounded
/// reachability fixpoint, a layered topological order, and cycle detection all report
/// `passed` through the production `marrow test` path.
#[test]
fn graph_report_conformance_fixture_passes_on_the_production_path() {
    let output = run_in(
        &conformance_dir("graph_report"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "graph_report fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":13"#), "{summary}");
}

/// The frozen `report(Text)` export travels the full production path under `marrow
/// run` and renders one deterministic multiline UTF-8 report. Text form prints the
/// report and a trailing newline; JSONL carries the identical text as the `data`
/// field of a `value` outcome, so the two renderings agree on the same bytes.
#[test]
fn report_renders_a_deterministic_multiline_report_through_run() {
    let dir = conformance_dir("graph_report");
    let input = "-> a\na -> b\nb -> c";

    let text = run_in(&dir, &["run", "graph_report.report", "--", input]);
    assert!(
        text.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        format!("{CHAIN_REPORT}\n"),
    );

    let jsonl = run_in(
        &dir,
        &[
            "run",
            "graph_report.report",
            "--format",
            "jsonl",
            "--",
            input,
        ],
    );
    assert!(jsonl.status.success(), "{jsonl:?}");
    // The report's only JSON-significant character is the newline, so escaping it is
    // exactly the `\n` substitution; the canonical run record keys are in byte order.
    let escaped = CHAIN_REPORT.replace('\n', "\\n");
    let expected = format!("{{\"data\":\"{escaped}\",\"kind\":\"run\",\"outcome\":\"value\"}}\n");
    assert_eq!(String::from_utf8_lossy(&jsonl.stdout), expected);
}

/// A graph with a cycle reaches the `report` export's cycle-detection path through
/// `marrow run`: the acyclic prefix appears in the topological order and the cyclic
/// nodes are named on the `-- cycle --` line, evidencing the bounded Kahn traversal
/// runs on the real VM, not only under `marrow test`.
#[test]
fn report_detects_a_cycle_through_run() {
    let dir = conformance_dir("graph_report");
    let input = "a -> b\nb -> c\nc -> a\nd -> a";

    let jsonl = run_in(
        &dir,
        &[
            "run",
            "graph_report.report",
            "--format",
            "jsonl",
            "--",
            input,
        ],
    );
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    assert!(stdout.contains(r#""outcome":"value""#), "{stdout}");
    // Only `d` (in-degree 0) is emittable; a, b, c form the reported cycle.
    assert!(
        stdout.contains(r"-- order --\n  d\n-- cycle --\n  a, b, c"),
        "{stdout}"
    );
}

struct ChildGuard(Option<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                child.kill().ok();
            }
            child.wait().ok();
        }
    }
}

fn run_with_stdin(dir: &Path, args: &[&str], input: &[u8]) -> Output {
    thread::scope(|scope| {
        // Error cleanup kills and reaps the child before the scope joins its reader.
        let mut child = ChildGuard(Some(
            Command::new(MARROW)
                .args(args)
                .current_dir(dir)
                .env("NO_COLOR", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn marrow with piped stdin"),
        ));
        let process = child.0.as_mut().expect("child remains owned");
        let mut stdin = process.stdin.take().expect("piped stdin");
        let mut stdout_pipe = process.stdout.take().expect("piped stdout");
        let mut stderr_pipe = process.stderr.take().expect("piped stderr");
        let written = stdin.write_all(input);
        drop(stdin);
        let stderr_reader = scope.spawn(move || {
            let mut stderr = Vec::new();
            stderr_pipe
                .read_to_end(&mut stderr)
                .expect("read marrow stderr");
            stderr
        });
        let mut stdout = Vec::new();
        stdout_pipe
            .read_to_end(&mut stdout)
            .expect("read marrow stdout");
        let status = process.wait().expect("reap marrow child");
        let stderr = stderr_reader.join().expect("join stderr reader");
        child.0 = None;
        // An early argument refusal may close stdin; retain the child's own outcome.
        if let Err(error) = written {
            assert_eq!(error.kind(), ErrorKind::BrokenPipe, "write stdin: {error}");
        }
        Output {
            status,
            stdout,
            stderr,
        }
    })
}

#[test]
fn report_accepts_a_multiline_string_from_stdin() {
    let dir = conformance_dir("graph_report");
    let escaped = CHAIN_REPORT.replace('\n', "\\n");
    for (format, expected) in [
        ("text", format!("{CHAIN_REPORT}\n")),
        (
            "jsonl",
            format!("{{\"data\":\"{escaped}\",\"kind\":\"run\",\"outcome\":\"value\"}}\n"),
        ),
    ] {
        let output = run_with_stdin(
            &dir,
            &["run", "graph_report.report", "--stdin", "--format", format],
            b"-> a\na -> b\nb -> c\n",
        );
        assert!(output.status.success(), "{format} stdin run: {output:?}");
        assert_eq!(output.stdout, expected.as_bytes(), "{format} report");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

const TEXT_BYTES_LIMIT: usize = 65_536;

const STRING_IDENTITY_SOURCE: &str = r#"pub fn echo(s: string): string {
    return s
}
"#;

#[test]
fn a_string_at_the_raw_output_limit_survives_json_escaping() {
    let workspace = Project::single(STRING_IDENTITY_SOURCE).materialize("text-output-exact");
    let input = "\u{01}".repeat(TEXT_BYTES_LIMIT);
    let output = workspace.marrow(&["run", "echo", "--format", "jsonl", "--", &input]);
    let expected = format!(
        "{{\"data\":\"{}\",\"kind\":\"run\",\"outcome\":\"value\"}}\n",
        "\\u0001".repeat(TEXT_BYTES_LIMIT),
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout.as_slice(), expected.as_bytes());
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn an_oversized_string_json_error_has_a_failed_process_status() {
    let workspace = Project::single(STRING_IDENTITY_SOURCE).materialize("text-output-excess");
    let input = "a".repeat(TEXT_BYTES_LIMIT + 1);
    let output = workspace.marrow(&["run", "echo", "--format", "jsonl", "--", &input]);
    assert_eq!(
        output.stdout.as_slice(),
        b"{\"code\":\"io.write\",\"kind\":\"run\",\"outcome\":\"error\"}\n",
        "oversized returned text must produce the operational error",
    );
    assert!(
        !output.status.success(),
        "an output error must not keep the invocation's success status: {:?}",
        output.status,
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn aggregate_text_keeps_its_separate_output_policy() {
    let source = r#"enum E {
    x(s: string)
}

pub fn wrap(s: string): E {
    return E::x(s: s)
}
"#;
    let workspace = Project::single(source).materialize("aggregate-text-output");
    let input = "a".repeat(TEXT_BYTES_LIMIT - 5);
    let output = workspace.marrow(&["run", "wrap", "--", &input]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout.as_slice(),
        format!("E::x({input})\n").as_bytes()
    );
    assert!(output.stderr.is_empty(), "{output:?}");

    let output = workspace.marrow(&["run", "wrap", "--format", "jsonl", "--", &input]);
    assert_eq!(
        output.stdout.as_slice(),
        b"{\"code\":\"io.write\",\"kind\":\"run\",\"outcome\":\"error\"}\n",
    );
    assert!(!output.status.success());
    assert!(output.stderr.is_empty(), "{output:?}");
}

const NON_STRING_RESULT_SOURCE: &str = "pub fn answer(input: string): int {\n    return 37\n}\n";

#[test]
fn stdin_refusal_precedes_invocation_and_a_non_string_result_is_allowed() {
    let workspace = Project::single(NON_STRING_RESULT_SOURCE).materialize("stdin-result");
    for (input, code, expected) in [
        (
            b"\0\r\ncaf\xc3\xa9\n".as_slice(),
            0,
            b"{\"data\":37,\"kind\":\"run\",\"outcome\":\"value\"}\n".as_slice(),
        ),
        (
            b"bad\xff".as_slice(),
            1,
            b"{\"code\":\"io.read\",\"kind\":\"run\",\"outcome\":\"error\"}\n".as_slice(),
        ),
    ] {
        let output = run_with_stdin(
            workspace.dir(),
            &["run", "answer", "--stdin", "--format", "jsonl"],
            input,
        );
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        assert_eq!(output.stdout, expected, "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn a_closed_output_pipe_returns_failure_without_panicking() {
    let workspace = Project::single(NON_STRING_RESULT_SOURCE).materialize("closed-output");
    let mut child = ChildGuard(Some(
        Command::new(MARROW)
            .args(["run", "answer", "--stdin"])
            .current_dir(workspace.dir())
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn marrow with an output pipe"),
    ));
    let process = child.0.as_mut().expect("child remains owned");
    let mut input = process.stdin.take().expect("piped stdin");
    // EOF releases the invocation only after its output pipe has no reader.
    drop(process.stdout.take().expect("piped stdout"));
    input.write_all(b"input").expect("write bounded stdin");
    drop(input);
    let mut stderr = Vec::new();
    process
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_end(&mut stderr)
        .expect("read stderr");
    let status = process.wait().expect("reap marrow child");
    child.0 = None;
    assert_eq!(status.code(), Some(1), "{status:?}: {stderr:?}");
    assert!(stderr.starts_with(b"io.write:"), "{stderr:?}");
}
