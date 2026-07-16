//! The cross-encoder half of the canonical-JSON known-answer test: the pinned
//! Node supervision module (the non-authoritative JavaScript mirror) must agree
//! byte-for-byte with the authoritative Rust encoder on every shared vector.
//!
//! The vectors are owned and asserted on the Rust side by
//! `marrow-local-wire`'s `canonical_kat` test, which serializes them to
//! `crates/marrow-local-wire/tests/fixtures/*.tsv`. This driver loads the
//! mirror's `encodeCanonical`/`parseCanonical` from the supervisor source and
//! asserts, over the same fixtures, that it reproduces every accepted canonical
//! form and rejects every non-canonical input with the same wire family.
//!
//! Spawning Node is denied by the command sandbox, so the test is `#[ignore]`d
//! and run explicitly with the sandbox disabled:
//!
//! ```text
//! cargo test -p marrow --test wire_kat -- --ignored
//! ```
//!
//! Requires `node` (v23.6+) on PATH.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The pinned supervision module: the mirror under test.
fn supervisor_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/supervisor/marrow-supervisor.mjs")
}

/// The directory holding the fixtures the Rust KAT generates.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .join("marrow-local-wire/tests/fixtures")
}

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-wire-kat-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
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

/// The driver loads the mirror by path and checks it against both fixtures.
const DRIVER: &str = r##"
import process from "node:process";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { join } from "node:path";

const M = await import(pathToFileURL(process.env.MARROW_SUPERVISOR).href);
const DIR = process.env.MARROW_KAT_DIR;

let failures = 0;
function ok(label, cond, detail) {
  if (cond) {
    console.log(`OK ${label}`);
  } else {
    failures += 1;
    console.log(`FAIL ${label}${detail === undefined ? "" : `: ${detail}`}`);
  }
}

function lines(name) {
  return readFileSync(join(DIR, name), "utf8")
    .split("\n")
    .filter((line) => line.length > 0 && !line.startsWith("#"));
}

// Accepted: the mirror parses each canonical form and its encoder reproduces the
// identical bytes. parseCanonical itself rejects any input its own encoder does
// not consider canonical, so a byte disagreement fails here.
for (const line of lines("canonical_kat.tsv")) {
  const tab = line.indexOf("\t");
  const key = line.slice(0, tab);
  const canonical = line.slice(tab + 1);
  const bytes = Buffer.from(canonical, "utf8");
  let value;
  try {
    value = M.parseCanonical(bytes);
  } catch (error) {
    ok(key, false, `parse threw ${error}`);
    continue;
  }
  const reencoded = M.encodeCanonical(value);
  ok(key, reencoded.equals(bytes), `re-encoded to ${reencoded.toString("utf8")}`);
}

// Rejects: the mirror refuses each input with the agreed wire family.
for (const line of lines("noncanonical_kat.tsv")) {
  const parts = line.split("\t");
  const key = parts[0];
  const family = parts[1];
  const input = parts.slice(2).join("\t");
  try {
    M.parseCanonical(Buffer.from(input, "utf8"));
    ok(key, false, "accepted a non-canonical input");
  } catch (error) {
    ok(
      key,
      error instanceof M.WireFormatError && error.code === `wire.${family}`,
      String(error),
    );
  }
}

if (failures === 0) console.log("DRIVER: all passed");
process.exit(failures === 0 ? 0 : 1);
"##;

/// The JavaScript mirror agrees with the authoritative Rust encoder on every
/// shared known-answer vector, in both the accept and reject directions.
#[test]
#[ignore = "spawns Node; run with the sandbox disabled"]
fn mirror_agrees_with_authoritative_encoder_on_kat_vectors() {
    let temp = TempDir::new();
    let driver = temp.root.join("driver.mjs");
    fs::write(&driver, DRIVER).expect("write driver");

    let output = Command::new("node")
        .arg(&driver)
        .env("MARROW_SUPERVISOR", supervisor_path())
        .env("MARROW_KAT_DIR", fixtures_dir())
        .current_dir(&temp.root)
        .output()
        .expect("node not found: the wire KAT cross-check needs Node v23.6+ on PATH");
    assert_driver_passed(&output);
}
