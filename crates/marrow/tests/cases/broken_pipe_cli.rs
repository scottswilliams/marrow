//! A downstream reader that closes the pipe early (`marrow ... | head -1`) must not
//! make the CLI panic. Rust ignores `SIGPIPE`, so a stdout write to a closed pipe
//! returns `EPIPE`; the CLI treats that as a clean exit. These tests drive the real
//! binary end to end: large `data dump` output must exit cleanly, and a noisy `run`
//! must stop before later saved-data writes commit.

use crate::support;
use crate::support_data;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use support::write;
use support_data::marrow;

/// A native-store project whose `seed` writes enough records that `data dump` cannot
/// fit its output in the OS pipe buffer. A reader taking only the first line leaves
/// the rest unwritten, so the dump is guaranteed to attempt a write into the closed
/// pipe — the condition that used to panic.
const SEED_SOURCE: &str = "\
module app

resource Item
    required value: int
store ^items(id: int): Item

pub fn seed()
    transaction
        for i in 1..=500
            var it: Item
            it.value = i
            ^items(i) = it
";

fn seeded_dump_project(name: &str) -> support::TempProject {
    let project = support::temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", SEED_SOURCE);
    });
    let dir = project.to_str().unwrap();
    let seeded = marrow(&["run", "--entry", "app::seed", dir]);
    assert_eq!(seeded.status.code(), Some(0), "seed run: {seeded:?}");
    project
}

fn noisy_run_project(name: &str) -> support::TempProject {
    let payload = "x".repeat(4096);
    let source = format!(
        "\
module app

resource Marker
    required value: int
store ^marker(id: int): Marker

pub fn noisy()
    for i in 1..=500
        print(\"{payload}\")
    var marker: Marker
    marker.value = 1
    transaction
        ^marker(1) = marker
"
    );
    support::temp_project_uncommitted(name, |root| {
        write(root, "marrow.json", support::native_config());
        write(root, "src/app.mw", &source);
    })
}

/// Spawn `marrow data dump <dir> --format <format>`, consume a small prefix of stdout
/// with `consume`, drop the read end to close the pipe, then wait for the child.
/// `consume` reads far less than the dump emits, so output is still pending in the
/// child when the pipe closes — the condition that forces a write into a closed pipe.
fn dump_then_close_pipe(
    dir: &str,
    format: &str,
    consume: impl FnOnce(&mut dyn Read),
) -> (std::process::ExitStatus, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["data", "dump", "--format", format, dir])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn marrow data dump");

    let mut stdout = child.stdout.take().expect("piped stdout");
    consume(&mut stdout);
    // Drop the read end so the pipe closes while the child is still writing.
    drop(stdout);

    let output = child.wait_with_output().expect("wait for marrow data dump");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    (output.status, stderr)
}

fn run_then_close_pipe(dir: &str) -> (std::process::ExitStatus, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["run", "--entry", "app::noisy", dir])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn marrow run");

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut prefix = [0u8; 16];
    let read = stdout.read(&mut prefix).expect("read run prefix");
    assert!(read > 0, "run produced no output to consume");
    drop(stdout);

    let output = child.wait_with_output().expect("wait for marrow run");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    (output.status, stderr)
}

fn assert_clean_exit(status: std::process::ExitStatus, stderr: &str, label: &str) {
    assert!(
        !stderr.contains("panicked"),
        "{label} panicked on a closed pipe; stderr:\n{stderr}"
    );
    // A clean broken-pipe exit succeeds; the consumer closing the pipe is normal.
    assert_eq!(
        status.code(),
        Some(0),
        "{label} status: {status:?}, stderr:\n{stderr}"
    );
}

#[test]
fn text_dump_into_an_early_closed_pipe_does_not_panic() {
    // The text path emits one line per record through `println!`, whose EPIPE panic
    // message is "failed printing to stdout: Broken pipe ...". A consumer like
    // `head -1` reads one line and leaves.
    let project = seeded_dump_project("bpipe-text");
    let dir = project.to_str().unwrap();
    let (status, stderr) = dump_then_close_pipe(dir, "text", |stdout| {
        let mut line = String::new();
        let read = BufRead::read_line(&mut BufReader::new(stdout), &mut line)
            .expect("read first dump line");
        assert!(read > 0, "dump produced no output to consume");
    });

    assert_clean_exit(status, &stderr, "data dump");
}

#[test]
fn json_dump_into_an_early_closed_pipe_does_not_panic() {
    // The streaming JSON path is a single long line; read only a small byte prefix so
    // the rest stays pending in the child when the pipe closes.
    let project = seeded_dump_project("bpipe-json");
    let dir = project.to_str().unwrap();
    let (status, stderr) = dump_then_close_pipe(dir, "json", |stdout| {
        let mut prefix = [0u8; 16];
        let read = stdout.read(&mut prefix).expect("read dump prefix");
        assert!(read > 0, "dump produced no output to consume");
    });

    assert_clean_exit(status, &stderr, "data dump --format json");
}

#[test]
fn run_output_into_an_early_closed_pipe_exits_before_later_writes() {
    let project = noisy_run_project("bpipe-run");
    let dir = project.to_str().unwrap();

    let (status, stderr) = run_then_close_pipe(dir);

    assert_clean_exit(status, &stderr, "run");
    let get = support_data::json(marrow(&[
        "data",
        "get",
        "--format",
        "json",
        dir,
        "^marker(1)",
    ]));
    assert_eq!(get["presence"], serde_json::json!("absent"), "{get}");
}
