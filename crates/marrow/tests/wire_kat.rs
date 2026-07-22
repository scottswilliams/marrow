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
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { join } from "node:path";

const instrumented = join(process.cwd(), "marrow-supervisor-kat.mjs");
const supervisorSource = readFileSync(process.env.MARROW_SUPERVISOR, "utf8");
writeFileSync(
  instrumented,
  `${supervisorSource}\nexport { FrameReader as __katFrameReader, decodeOneFrame as __katDecodeOneFrame, isLaunchDescriptor as __katIsLaunchDescriptor, isReady as __katIsReady };\n`,
);
const M = await import(pathToFileURL(instrumented).href);
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

function driveReply(messages, options = {}) {
  const observed = {
    current: null,
    queued: null,
    pump: 0,
    tornDown: false,
  };
  const session = Object.create(M.Session.prototype);
  const withTurn = messages.map((message) =>
    options.addTurn === false || message === null || typeof message !== "object" || Array.isArray(message)
      ? message
      : { ...message, turn: options.replyTurn ?? 0n },
  );
  session.frames = {
    buffer: Buffer.alloc(options.trailingBytes ?? 0),
    push() {
      return withTurn;
    },
  };
  session.dead = false;
  session.inFlight = options.inFlight === false ? null : {
    turn: options.pendingTurn ?? 0n,
    resolve(value) {
      observed.current = { kind: "value", value };
    },
    reject(error) {
      observed.current = { kind: "error", error };
    },
  };
  session.queue = [{
    reject(error) {
      observed.queued = error;
    },
  }];
  session.replyDeadline = null;
  session.exitHook = () => {};
  session.pump = () => {
    observed.pump += 1;
  };
  session.teardown = () => {
    session.dead = true;
    observed.tornDown = true;
  };
  M.Session.prototype.onData.call(
    session,
    options.chunk ?? Buffer.from([0x01]),
  );
  return { observed, session };
}

{
  const observed = { first: null, second: null, pump: 0, tornDown: false };
  let delivered;
  const session = Object.create(M.Session.prototype);
  session.frames = {
    buffer: Buffer.alloc(0),
    push() {
      return [delivered];
    },
  };
  session.dead = false;
  session.inFlight = {
    turn: 0n,
    resolve(value) {
      observed.first = value;
    },
    reject(error) {
      observed.first = error;
    },
  };
  session.queue = [{
    resolve(value) {
      observed.second = { kind: "value", value };
    },
    reject(error) {
      observed.second = { kind: "error", error };
    },
  }];
  session.replyDeadline = null;
  session.exitHook = () => {};
  session.pump = () => {
    observed.pump += 1;
    const next = session.queue.shift();
    if (next !== undefined) session.inFlight = { ...next, turn: 1n };
  };
  session.teardown = () => {
    session.dead = true;
    observed.tornDown = true;
  };

  delivered = { data: 7n, kind: "value", turn: 0n };
  M.Session.prototype.onData.call(session, Buffer.from([0x01]));
  delivered = { data: 999n, kind: "value", turn: 0n };
  M.Session.prototype.onData.call(session, Buffer.from([0x02]));

  ok(
    "reply-delayed-duplicate-cannot-settle-next-call",
    observed.first === 7n &&
      observed.second?.kind === "error" &&
      observed.second.error instanceof M.MarrowLossError &&
      observed.second.error.loss === M.LOSS.OUTCOME_UNKNOWN &&
      observed.second.error.cause instanceof M.WireFormatError &&
      observed.pump === 1 &&
      session.dead &&
      observed.tornDown,
    `first=${String(observed.first)} pump=${observed.pump} dead=${session.dead}`,
  );
}

{
  const observed = { first: null, second: null, writes: [] };
  let delivered;
  const session = Object.create(M.Session.prototype);
  session.frames = {
    buffer: Buffer.alloc(0),
    push() {
      return [delivered];
    },
  };
  session.dead = false;
  session.inFlight = null;
  session.nextTurn = 0xffff_ffffn;
  session.queue = [
    {
      args: [],
      exportId: "11".repeat(32),
      resolve(value) {
        observed.first = value;
      },
      reject(error) {
        observed.first = error;
      },
    },
    {
      args: [],
      exportId: "22".repeat(32),
      resolve(value) {
        observed.second = value;
      },
      reject(error) {
        observed.second = error;
      },
    },
  ];
  session.socket = {
    write(frame) {
      observed.writes.push(frame);
    },
  };
  session.replyDeadline = null;
  session.exitHook = () => {};
  session.teardown = () => {
    session.dead = true;
  };

  M.Session.prototype.pump.call(session);
  const requestReader = new M.__katFrameReader();
  const request = M.__katDecodeOneFrame(requestReader, observed.writes[0]);
  delivered = { data: 1n, kind: "value", turn: 0xffff_ffffn };
  M.Session.prototype.onData.call(session, Buffer.from([0x01]));

  ok(
    "request-turn-maximum-then-exhaustion-never-wraps",
    request.turn === 0xffff_ffffn &&
      session.nextTurn === 0x1_0000_0000n &&
      observed.first === 1n &&
      observed.second instanceof RangeError &&
      observed.writes.length === 1 &&
      session.dead,
  );
}

function malformedReply(label, messages, options = {}) {
  const { observed, session } = driveReply(messages, options);
  ok(
    `reply-${label}`,
    observed.current?.kind === "error" &&
      observed.current.error instanceof M.MarrowLossError &&
      observed.current.error.loss === M.LOSS.OUTCOME_UNKNOWN &&
      observed.current.error.cause instanceof M.WireFormatError &&
      observed.current.error.cause.code === "wire.malformed" &&
      observed.queued instanceof M.MarrowLossError &&
      observed.queued.loss === M.LOSS.INTERRUPTED &&
      observed.pump === 0 &&
      session.dead &&
      observed.tornDown,
    String(observed.current?.error),
  );
}

const span = { column: 2n, line: 1n };
for (const [label, message] of [
  ["null", null],
  ["primitive", 1n],
  ["array", []],
  ["value-missing", { kind: "value" }],
  ["value-extra", { data: null, extra: null, kind: "value" }],
  ["fault-empty-code", { code: "", kind: "fault", span }],
  ["fault-uppercase-code", { code: "Run.commit", kind: "fault", span }],
  ["fault-array-span", { code: "run.commit", kind: "fault", span: [] }],
  ["fault-extra-span", {
    code: "run.commit",
    kind: "fault",
    span: { column: 2n, extra: 0n, line: 1n },
  }],
  ["fault-negative-line", {
    code: "run.commit",
    kind: "fault",
    span: { column: 2n, line: -1n },
  }],
  ["fault-overflow-column", {
    code: "run.commit",
    kind: "fault",
    span: { column: 0x1_0000_0000n, line: 1n },
  }],
  ["fault-number-line", {
    code: "run.commit",
    kind: "fault",
    span: { column: 2n, line: 1 },
  }],
  ["incomplete-durable", {
    code: "run.commit",
    durable: "maybe",
    kind: "incomplete",
    span,
  }],
  ["incomplete-extra", {
    code: "run.commit",
    durable: M.DURABLE_STATE.UNKNOWN,
    extra: null,
    kind: "incomplete",
    span,
  }],
  ["reject-code", { code: "runner-bad", kind: "reject" }],
  ["ready-after-handshake", {
    interface: "11".repeat(32),
    kind: "ready",
    session: "22".repeat(32),
  }],
]) {
  malformedReply(label, [message]);
}

malformedReply(
  "two-frames",
  [{ data: 1n, kind: "value" }, { data: 2n, kind: "value" }],
);
malformedReply(
  "trailing-partial-frame",
  [{ data: 1n, kind: "value" }],
  { trailingBytes: 1 },
);
malformedReply("turn-missing", [{ data: 1n, kind: "value" }], { addTurn: false });
malformedReply("turn-negative", [{ data: 1n, kind: "value" }], { replyTurn: -1n });
malformedReply("turn-number", [{ data: 1n, kind: "value" }], { replyTurn: 0 });
malformedReply("turn-overflow", [{ data: 1n, kind: "value" }], {
  replyTurn: 0x1_0000_0000n,
});
malformedReply("turn-mismatch", [{ data: 1n, kind: "value" }], {
  pendingTurn: 1n,
  replyTurn: 0n,
});

{
  const { observed, session } = driveReply([], { inFlight: false });
  ok(
    "reply-idle-partial-unsolicited",
    observed.current === null &&
      observed.queued instanceof M.MarrowLossError &&
      observed.queued.loss === M.LOSS.INTERRUPTED &&
      observed.pump === 0 &&
      session.dead,
  );
}

{
  const { observed, session } = driveReply([{ data: 7n, kind: "value" }]);
  ok(
    "reply-valid-value",
    observed.current?.kind === "value" &&
      observed.current.value === 7n &&
      observed.pump === 1 &&
      !session.dead,
  );
}
{
  const { observed, session } = driveReply([
    { code: "run.commit", kind: "fault", span },
  ]);
  ok(
    "reply-valid-fault",
    observed.current?.kind === "error" &&
      observed.current.error instanceof M.MarrowFault &&
      observed.current.error.line === 1n &&
      observed.current.error.column === 2n &&
      observed.pump === 1 &&
      !session.dead,
  );
}
{
  const { observed, session } = driveReply([
    { code: "runner.unknown_export", kind: "reject" },
  ]);
  ok(
    "reply-valid-reject",
    observed.current?.kind === "error" &&
      observed.current.error instanceof M.MarrowReject &&
      observed.pump === 1 &&
      !session.dead,
  );
}
for (const durable of Object.values(M.DURABLE_STATE)) {
  const { observed, session } = driveReply([{
    code: "run.commit",
    durable,
    kind: "incomplete",
    span,
  }]);
  ok(
    `reply-valid-incomplete-${durable}-retires`,
    observed.current?.kind === "error" &&
      observed.current.error instanceof M.MarrowIncomplete &&
      observed.current.error.durable === durable &&
      observed.queued instanceof M.MarrowLossError &&
      observed.queued.loss === M.LOSS.INTERRUPTED &&
      observed.pump === 0 &&
      session.dead,
  );
}

const validSession = "ab".repeat(32);
const validInterface = "cd".repeat(32);
const validReady = {
  interface: validInterface,
  kind: "ready",
  session: validSession,
};
for (const [label, ready, accepted] of [
  ["valid", validReady, true],
  ["null", null, false],
  ["primitive", 1n, false],
  ["missing", { interface: validInterface, kind: "ready" }, false],
  ["extra", { ...validReady, extra: null }, false],
  ["wrong-kind", { ...validReady, kind: "value" }, false],
  ["uppercase-session", { ...validReady, session: validSession.toUpperCase() }, false],
  ["nonhex-interface", { ...validReady, interface: `${"cd".repeat(31)}cg` }, false],
  ["short-interface", { ...validReady, interface: "c".repeat(63) }, false],
  ["long-interface", { ...validReady, interface: "c".repeat(65) }, false],
]) {
  ok(`ready-schema-${label}`, M.__katIsReady(ready) === accepted);
}

for (const [label, descriptor, accepted] of [
  ["valid", { interface: validInterface, session: validSession, socket: "/tmp/s" }, true],
  ["null", null, false],
  ["extra", {
    extra: null,
    interface: validInterface,
    session: validSession,
    socket: "/tmp/s",
  }, false],
  ["uppercase", {
    interface: validInterface.toUpperCase(),
    session: validSession,
    socket: "/tmp/s",
  }, false],
]) {
  ok(
    `descriptor-schema-${label}`,
    M.__katIsLaunchDescriptor(descriptor) === accepted,
  );
}

const readyFrame = M.encodeFrame(validReady);
{
  const reader = new M.__katFrameReader();
  const ready = M.__katDecodeOneFrame(reader, readyFrame);
  ok(
    "ready-exactly-one-frame",
    M.__katIsReady(ready) && reader.buffer.length === 0,
  );
}
for (const [label, bytes] of [
  ["two-frames", Buffer.concat([readyFrame, readyFrame])],
  ["trailing-partial", Buffer.concat([readyFrame, Buffer.from([0x00])])],
]) {
  const reader = new M.__katFrameReader();
  try {
    M.__katDecodeOneFrame(reader, bytes);
    ok(`ready-${label}`, false, "accepted more than one complete frame");
  } catch (error) {
    ok(
      `ready-${label}`,
      error instanceof M.WireFormatError && error.code === "wire.malformed",
      String(error),
    );
  }
}
{
  const reader = new M.__katFrameReader();
  const waiting = M.__katDecodeOneFrame(reader, readyFrame.subarray(0, 3));
  ok(
    "ready-partial-current-frame-waits",
    waiting === undefined && reader.buffer.length === 3,
  );
  const ready = M.__katDecodeOneFrame(reader, readyFrame.subarray(3));
  ok(
    "ready-split-frame-completes-exactly-once",
    M.__katIsReady(ready) && reader.buffer.length === 0,
  );
}

for (const boundary of [0n, 0xffff_ffffn]) {
  const { observed, session } = driveReply([{
    code: "run.boundary",
    kind: "fault",
    span: { column: boundary, line: boundary },
  }], { pendingTurn: boundary, replyTurn: boundary });
  ok(
    `reply-valid-u32-${boundary}`,
    observed.current?.kind === "error" &&
      observed.current.error instanceof M.MarrowFault &&
      observed.current.error.line === boundary &&
      observed.current.error.column === boundary &&
      observed.pump === 1 &&
      !session.dead,
  );
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
