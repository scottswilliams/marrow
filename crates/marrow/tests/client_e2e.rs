//! The G02a lane exit gate, end to end: generated strict TypeScript performs a
//! real storeless call against the stock runner.
//!
//! Node spawns the runner per the channel law (through the pinned supervision
//! module), calls generated methods, and receives typed results; the child-death
//! boundaries are re-proven through the generated client, with the queued
//! `interrupted` class exercised for real (one call in flight, one queued, then a
//! fail-closed terminate in the same synchronous turn — the reply cannot have
//! been processed, so the classification is deterministic).
//!
//! Each test spawns Node and Unix-socket traffic, which the command sandbox
//! denies, so each is `#[ignore]`d and run explicitly with the sandbox disabled:
//!
//! ```text
//! cargo test -p marrow --test client_e2e -- --ignored --test-threads=1
//! ```
//!
//! Requires `node` (v23.6+ for default type stripping of `.mts`) on PATH and the
//! workspace binaries built (`--all-targets` builds `marrow-runner`).

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

/// The stock runner binary: built into the same deps/../ directory as the CLI
/// test binary by a workspace `--all-targets` build.
fn runner_path() -> PathBuf {
    let path = Path::new(MARROW)
        .parent()
        .expect("binary dir")
        .join("marrow-runner");
    assert!(
        path.is_file(),
        "stock runner not built at {}; run a workspace --all-targets build first",
        path.display()
    );
    path
}

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-e2e-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

const FIXTURE: &str = "struct Point\n\
    \x20   x: int\n\
    \x20   y: int\n\
    \n\
    enum Shape\n\
    \x20   dot\n\
    \x20   circle(radius: int)\n\
    \n\
    pub fn add(a: int, b: int): int\n\
    \x20   return a + b\n\
    \n\
    pub fn shift(p: Point, dx: int): Point\n\
    \x20   return Point(x: p.x + dx, y: p.y)\n\
    \n\
    pub fn grow(s: Shape): Shape\n\
    \x20   match s\n\
    \x20       dot\n\
    \x20           return Shape::dot\n\
    \x20       circle(r)\n\
    \x20           return Shape::circle(radius: r + 1)\n\
    \n\
    pub fn ping()\n\
    \x20   return\n";

/// Build the project, generate the client, compile the image to a file, and
/// return the project directory.
fn prepare(temp: &TempDir) -> PathBuf {
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), FIXTURE);

    let generated = Command::new(MARROW)
        .args(["client", "typescript", "--out", "gen"])
        .current_dir(&project)
        .output()
        .expect("run marrow");
    assert!(
        generated.status.success(),
        "generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );

    // Compile the same source in process and write the canonical image bytes the
    // runner serves.
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        FIXTURE.as_bytes().to_vec(),
    )];
    let captured = marrow_project::capture(
        &manifest,
        files,
        None,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&captured).expect("compile");
    fs::write(project.join("program.image"), &compiled.image.bytes).expect("write image");
    project
}

/// Run a driver script with Node and return its output.
fn node(project: &Path, driver: &str) -> Output {
    Command::new("node")
        .arg(driver)
        .env("MARROW_RUNNER", runner_path())
        .env("MARROW_IMAGE", project.join("program.image"))
        .current_dir(project)
        .output()
        .expect("node not found: the e2e exit gate needs Node v23.6+ on PATH")
}

fn assert_driver_passed(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.lines().any(|line| line == "DRIVER: all passed"),
        "driver failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        !stdout.contains("FAIL"),
        "driver reported a failing assertion:\n{stdout}"
    );
}

/// Shared driver prelude: a tiny assertion harness over the generated client.
const PRELUDE: &str = r#"
import { Client } from "./gen/client.mts";
import * as M from "./gen/marrow-supervisor.mjs";

const RUNNER = process.env.MARROW_RUNNER!;
const IMAGE = process.env.MARROW_IMAGE!;

let failures = 0;
function ok(label: string, cond: boolean, detail?: string) {
  if (cond) {
    console.log(`OK ${label}`);
  } else {
    failures += 1;
    console.log(`FAIL ${label}${detail === undefined ? "" : `: ${detail}`}`);
  }
}
function finish() {
  if (failures === 0) console.log("DRIVER: all passed");
  process.exit(failures === 0 ? 0 : 1);
}
"#;

/// The happy path: launch per the channel law, every export shape round-trips,
/// a runtime fault arrives typed and source-mapped, and close is clean.
#[test]
#[ignore = "spawns Node + Unix sockets; run with the sandbox disabled"]
fn generated_client_performs_real_storeless_calls() {
    let temp = TempDir::new("happy");
    let project = prepare(&temp);
    let driver = format!(
        "{PRELUDE}\n{}",
        r#"
const client = await Client.launch({ runner: RUNNER, image: IMAGE });

const sum = await client.add(2n, 3n);
ok("add", sum === 5n, String(sum));

const moved = await client.shift({ x: 1n, y: 2n }, 10n);
ok("shift", moved.x === 11n && moved.y === 2n, `${moved.x},${moved.y}`);

const grown = await client.grow({ member: "circle", payload: [4n] });
ok("grow", grown.member === "circle" && grown.payload[0] === 5n);

const dot = await client.grow({ member: "dot", payload: [] });
ok("grow-dot", dot.member === "dot" && dot.payload.length === 0);

const nothing = await client.ping();
ok("ping", nothing === undefined);

try {
  await client.add(9223372036854775807n, 1n);
  ok("fault", false, "overflow did not fault");
} catch (error) {
  ok(
    "fault",
    error instanceof M.MarrowFault && error.code === "run.overflow",
    String(error),
  );
}

// The session survives a fault: the next call still works.
const again = await client.add(20n, 1n);
ok("add-after-fault", again === 21n);

// Client-side validation mirrors the wire grammar: a lossy number is refused
// before any byte is sent.
try {
  // @ts-expect-error deliberately wrong argument type
  await client.add(1, 2n);
  ok("validate", false, "a number was accepted for an int");
} catch (error) {
  ok("validate", error instanceof TypeError, String(error));
}

await client.close();
ok("close", true);
finish();
"#
    );
    write(&project.join("driver_happy.mts"), &driver);
    assert_driver_passed(&node(&project, "driver_happy.mts"));
}

/// The child-death boundaries through the generated client. A dispatched call
/// classifies `outcome_unknown`, a queued call `interrupted` (exercised for
/// real: terminate runs in the same synchronous turn, before any reply I/O
/// callback can settle the in-flight call), and a call after death
/// `not_started`.
#[test]
#[ignore = "spawns Node + Unix sockets; run with the sandbox disabled"]
fn death_boundaries_classify_through_the_generated_client() {
    let temp = TempDir::new("death");
    let project = prepare(&temp);
    let driver = format!(
        "{PRELUDE}\n{}",
        r#"
const client = await Client.launch({ runner: RUNNER, image: IMAGE });

// Issue two calls in one synchronous turn: the first is dispatched to the
// serial worker, the second waits in the bounded queue. Then kill the runner
// fail-closed in the same turn — no reply event can have been processed yet.
const dispatched = client.add(1n, 2n);
const queued = client.add(3n, 4n);
client.terminate();

try {
  await dispatched;
  ok("dispatched", false, "a reply arrived after terminate");
} catch (error) {
  ok(
    "dispatched",
    error instanceof M.MarrowLossError && error.loss === "outcome_unknown",
    String(error),
  );
}

try {
  await queued;
  ok("queued", false, "a queued call resolved after terminate");
} catch (error) {
  ok(
    "queued",
    error instanceof M.MarrowLossError && error.loss === "interrupted",
    String(error),
  );
}

try {
  await client.add(5n, 6n);
  ok("after-death", false, "a dead session accepted a call");
} catch (error) {
  ok(
    "after-death",
    error instanceof M.MarrowLossError && error.loss === "not_started",
    String(error),
  );
}

finish();
"#
    );
    write(&project.join("driver_death.mts"), &driver);
    assert_driver_passed(&node(&project, "driver_death.mts"));
}

/// Death before the handshake: a runner that exits before serving (a missing
/// image) fails the launch itself — the `not_started` boundary, since no call
/// was ever admitted.
#[test]
#[ignore = "spawns Node + Unix sockets; run with the sandbox disabled"]
fn a_failed_launch_is_not_started() {
    let temp = TempDir::new("badlaunch");
    let project = prepare(&temp);
    let driver = format!(
        "{PRELUDE}\n{}",
        r#"
try {
  await Client.launch({ runner: RUNNER, image: IMAGE + ".missing" });
  ok("launch", false, "a missing image launched");
} catch (error) {
  ok(
    "launch",
    error instanceof M.LaunchError && error.loss === "not_started",
    String(error),
  );
}
finish();
"#
    );
    write(&project.join("driver_badlaunch.mts"), &driver);
    assert_driver_passed(&node(&project, "driver_badlaunch.mts"));
}
