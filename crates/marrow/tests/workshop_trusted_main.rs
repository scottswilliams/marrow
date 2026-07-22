//! The G03 exit-gate journey: the Workshop client completes add / list / correct /
//! rollback and **restart** over a real persistent native store (F02) driven entirely
//! through the trusted Node main (the pinned supervision module) and the generated
//! strict TypeScript client.
//!
//! The trusted main owns every process/store/path choice: it provisions the store, spawns
//! the runner as a native attached session (`attach --image --store`) without a shell and
//! with the child's stdin closed, and drives the generated client — whose per-export methods
//! take only domain arguments, so the calling code selects no store, image, path, grant, or
//! ceiling. Restart is proven by closing the session and opening a fresh attached session
//! that reads back the committed data — each attach is a separate runner process over the
//! persisted store.
//!
//! A companion journey proves the term-3 (D08) refusal end to end through the Node path: a
//! store provisioned under a read-only image refuses an attached image whose demand exceeds
//! the accepted ceiling with a typed `MarrowReject` carrying `store.demand_exceeds_ceiling`,
//! and the source-vocabulary refusal sentence arrives on the runner's byte-log (stderr).
//!
//! Each test spawns Node, a runner process, and Unix-socket traffic, which the command
//! sandbox denies, so each is `#[ignore]`d and run explicitly with the sandbox disabled:
//!
//! ```text
//! cargo test -p marrow --test workshop_trusted_main -- --ignored --test-threads=1
//! ```
//!
//! Requires `node` (v23.6+ for `.mts` type stripping) on PATH and a workspace
//! `--all-targets` build (which builds `marrow` and `marrow-runner`).

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

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
        let root = std::env::temp_dir().join(format!(
            "marrow-workshop-tm-{name}-{}-{nanos}",
            std::process::id()
        ));
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

/// The stable identity ledger the fixtures below share, so every variant keeps one durable
/// contract and interface while only its demand or code differs.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Asset 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id field Asset.tag 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id field Asset.name 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id field Asset.location 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id product Tally 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id field Tally.count 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id root assets 3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a\n\
     id key assets.id 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id root tallies 4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a\n\
     id key tallies.name 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     high-water 0\n\
     end\n";

/// The durable shape shared by every fixture: a tool-crib catalog over two roots — `^assets`
/// holds the tools and `^tallies` holds application counters keyed by name — so a mutating
/// export can write both in one `transaction`, committing or rolling back together.
const SHAPE: &str = "resource Asset {\n\
    \x20   required tag: string\n\
    \x20   required name: string\n\
    \x20   location: string\n\
    }\n\
    \n\
    resource Tally {\n\
    \x20   required count: int\n\
    }\n\
    \n\
    store ^assets[id: int]: Asset\n\
    store ^tallies[name: string]: Tally\n";

/// The Workshop journey program: cross-root create, reads, a cross-root correction, and a
/// cross-root rollback — the whole add / list / correct / rollback surface, all transferable.
fn journey_source() -> String {
    format!(
        "{SHAPE}\n\
        pub fn add(id: int, tag: string, name: string): bool {{\n\
        \x20   transaction {{\n\
        \x20       if exists(^assets[id]) {{ return false }}\n\
        \x20       ^assets[id] = Asset(tag: tag, name: name)\n\
        \x20       const prior = ^tallies[\"catalogued\"].count ?? 0\n\
        \x20       ^tallies[\"catalogued\"].count = prior + 1\n\
        \x20   }}\n\
        \x20   return true\n\
        }}\n\
        \n\
        pub fn assetName(id: int): string? {{\n\
        \x20   return ^assets[id].name\n\
        }}\n\
        \n\
        pub fn catalogued(): int {{\n\
        \x20   return ^tallies[\"catalogued\"].count ?? 0\n\
        }}\n\
        \n\
        pub fn moveCount(): int {{\n\
        \x20   return ^tallies[\"moves\"].count ?? 0\n\
        }}\n\
        \n\
        pub fn recordMove(id: int, location: string) {{\n\
        \x20   transaction {{\n\
        \x20       ^assets[id].location = location\n\
        \x20       const prior = ^tallies[\"moves\"].count ?? 0\n\
        \x20       ^tallies[\"moves\"].count = prior + 1\n\
        \x20   }}\n\
        }}\n\
        \n\
        pub fn location(id: int): string? {{\n\
        \x20   return ^assets[id].location\n\
        }}\n\
        \n\
        pub fn present(id: int): bool {{\n\
        \x20   return exists(^assets[id])\n\
        }}\n"
    )
}

/// A read-only image over the same shape: its demand union is the accepted ceiling a store
/// provisioned under it records — it only reads `^assets`.
fn read_only_source() -> String {
    format!("{SHAPE}\npub fn peek(id: int): string? {{\n    return ^assets[id].name\n}}\n")
}

/// The broadened image: `peek` keeps its signature but now also writes `^assets.location`, so
/// its demand exceeds the read-only accepted ceiling by a write the ceiling does not admit.
fn broadened_source() -> String {
    format!(
        "{SHAPE}\n\
        pub fn peek(id: int): string? {{\n\
        \x20   var result = ^assets[id].name\n\
        \x20   transaction {{\n\
        \x20       place slot = ^assets[id]\n\
        \x20       if exists(slot) {{ slot.location = \"seen\" }}\n\
        \x20   }}\n\
        \x20   return result\n\
        }}\n"
    )
}

/// Compile `source` (with the shared ids) into the canonical image bytes the runner serves.
fn compile_image(source: &str) -> Vec<u8> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes
}

/// Write `source` as a project, generate the strict TypeScript client into `gen/`, and write
/// the compiled image beside it. Returns the project directory.
fn prepare(temp: &TempDir, dir: &str, source: &str) -> PathBuf {
    let project = temp.join(dir);
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), source);
    write(&project.join("marrow.ids"), IDS);

    let generated = Command::new(MARROW)
        .args(["client", "typescript", "--out", "gen"])
        .current_dir(&project)
        .output()
        .expect("run marrow client");
    assert!(
        generated.status.success(),
        "client generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );

    fs::write(project.join("program.image"), compile_image(source)).expect("write image");
    project
}

fn node(project: &Path, driver: &str, runner: &Path, extra: &[(&str, &Path)]) -> Output {
    let mut command = Command::new("node");
    command
        .arg(driver)
        .env("MARROW_RUNNER", runner)
        .env("MARROW_IMAGE", project.join("program.image"))
        .current_dir(project);
    for (key, value) in extra {
        command.env(key, value);
    }
    command
        .output()
        .expect("node not found: the trusted-main journey needs Node v23.6+ on PATH")
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

/// A tiny assertion harness over the generated client and the trusted main.
const PRELUDE: &str = r#"
import { Client } from "./gen/client.mts";
import * as M from "./gen/marrow-supervisor.mjs";

const RUNNER = process.env.MARROW_RUNNER!;
const IMAGE = process.env.MARROW_IMAGE!;
const STORE = process.env.MARROW_STORE!;

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

/// The exit gate: the Workshop journey through the trusted Node main over a real persistent
/// store, including a restart between two attached sessions.
#[test]
#[ignore = "spawns Node + a runner + Unix sockets; run with the sandbox disabled"]
fn workshop_journey_through_the_trusted_main() {
    let temp = TempDir::new("journey");
    let project = prepare(&temp, "app", &journey_source());
    let store = temp.join("store");
    let runner = runner_path();

    let driver = format!(
        "{PRELUDE}\n{}",
        r#"
// The trusted main provisions the store (choosing the destination itself), then opens a
// native attached session. The renderer/client selects nothing but domain arguments.
await M.provision({ runner: RUNNER, image: IMAGE, store: STORE });

const first = await Client.launch({ runner: RUNNER, image: IMAGE, store: STORE });

// add: one cross-root transaction commits ^assets and ^tallies together.
ok("add", (await first.add(1n, "T-100", "Cordless Drill")) === true);
// list/read the committed state back.
ok("assetName", (await first.assetName(1n)) === "Cordless Drill");
ok("catalogued", (await first.catalogued()) === 1n);
ok("moveCount-0", (await first.moveCount()) === 0n);

// correct: a cross-root move advances the moves tally with the location.
await first.recordMove(1n, "Bay 3");
ok("location", (await first.location(1n)) === "Bay 3");
ok("moveCount-1", (await first.moveCount()) === 1n);

// rollback: an unguarded move on an absent asset faults required-missing and rolls the whole
// cross-root region back — the moves tally is not advanced by a move that never landed.
try {
  await first.recordMove(2n, "Bay 9");
  ok("rollback", false, "a move on an absent asset did not fault");
} catch (error) {
  ok("rollback", error instanceof M.MarrowFault && error.code === "run.required_missing", String(error));
}
ok("rollback-location", (await first.location(1n)) === "Bay 3");
ok("rollback-moveCount", (await first.moveCount()) === 1n);
ok("rollback-absent", (await first.present(2n)) === false);
ok("rollback-catalogued", (await first.catalogued()) === 1n);

// restart: close the session, then open a FRESH attached session (a separate runner process
// over the persisted store) and read the committed data back.
await first.close();

const second = await Client.launch({ runner: RUNNER, image: IMAGE, store: STORE });
ok("restart-assetName", (await second.assetName(1n)) === "Cordless Drill");
ok("restart-location", (await second.location(1n)) === "Bay 3");
ok("restart-catalogued", (await second.catalogued()) === 1n);
ok("restart-moveCount", (await second.moveCount()) === 1n);
await second.close();

finish();
"#
    );
    write(&project.join("driver_journey.mts"), &driver);
    assert_driver_passed(&node(
        &project,
        "driver_journey.mts",
        &runner,
        &[("MARROW_STORE", &store)],
    ));
}

/// The term-3 (D08) refusal end to end through the Node path: a store provisioned under the
/// read-only image refuses an attached image whose demand exceeds the accepted ceiling, with a
/// typed `MarrowReject` carrying `store.demand_exceeds_ceiling`; the source-vocabulary sentence
/// arrives on the runner's byte-log.
#[test]
#[ignore = "spawns Node + a runner + Unix sockets; run with the sandbox disabled"]
fn a_broadened_image_is_refused_through_the_trusted_main() {
    let temp = TempDir::new("refuse");
    // The store is provisioned under the read-only image (its demand is the accepted ceiling).
    let read_only = prepare(&temp, "readonly", &read_only_source());
    // The client is generated from the broadened image, so it pins that image's identity and
    // matches the runner the trusted main attaches with it.
    let broadened = prepare(&temp, "broadened", &broadened_source());
    let store = temp.join("store");
    let runner = runner_path();

    // Provision the store under the read-only image via the runner CLI.
    let provisioned = Command::new(&runner)
        .args(["provision", "--image"])
        .arg(read_only.join("program.image"))
        .arg("--store")
        .arg(&store)
        .arg("--yes")
        .output()
        .expect("run provision");
    assert!(
        provisioned.status.success(),
        "provision failed: {}",
        String::from_utf8_lossy(&provisioned.stderr)
    );

    let driver = format!(
        "{PRELUDE}\n{}",
        r#"
// The trusted main owns the byte-log pipe; capture it to prove the source-vocabulary sentence
// reaches it.
let logged = "";
const log = (chunk: Uint8Array) => { logged += Buffer.from(chunk).toString("utf8"); };

// Attach the broadened image to the store provisioned under the narrower read-only ceiling.
// The runner refuses before opening the store and serves a typed reject over the channel.
const client = await Client.launch({ runner: RUNNER, image: IMAGE, store: STORE, log });
try {
  await client.peek(1n);
  ok("reject", false, "the broadened image was not refused");
} catch (error) {
  ok(
    "reject",
    error instanceof M.MarrowReject && error.code === "store.demand_exceeds_ceiling",
    String(error),
  );
}
// The full source-vocabulary refusal sentence reaches the trusted main's byte-log.
ok("byte-log", logged.includes("store.demand_exceeds_ceiling") && logged.includes("writes ^assets.location"), logged);
client.terminate();
finish();
"#
    );
    write(&broadened.join("driver_refuse.mts"), &driver);
    assert_driver_passed(&node(
        &broadened,
        "driver_refuse.mts",
        &runner,
        &[("MARROW_STORE", &store)],
    ));
}
