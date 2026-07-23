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

function defineData(target, key, value) {
  Object.defineProperty(target, key, {
    configurable: true,
    enumerable: true,
    value,
    writable: true,
  });
  return target;
}

function ownObject(entries, prototype = Object.prototype) {
  const value = Object.create(prototype);
  for (const [key, field] of entries) defineData(value, key, field);
  return value;
}

function normalOwnData(target, key, expected) {
  const descriptor = Object.getOwnPropertyDescriptor(target, key);
  return (
    descriptor !== undefined &&
    descriptor.configurable === true &&
    descriptor.enumerable === true &&
    descriptor.writable === true &&
    Object.is(descriptor.value, expected) &&
    descriptor.get === undefined &&
    descriptor.set === undefined
  );
}

function normalDenseArray(value, expected) {
  if (
    !Array.isArray(value) ||
    Object.getPrototypeOf(value) !== Array.prototype ||
    value.length !== expected.length
  ) {
    return false;
  }
  for (let i = 0; i < expected.length; i += 1) {
    if (!normalOwnData(value, String(i), expected[i])) return false;
  }
  return true;
}

function inheritedSparse(length, index, inheritedValue) {
  let reads = 0;
  const prototype = Object.create(Array.prototype);
  Object.defineProperty(prototype, String(index), {
    configurable: true,
    enumerable: true,
    get() {
      reads += 1;
      return inheritedValue;
    },
  });
  const value = [];
  value.length = length;
  Object.setPrototypeOf(value, prototype);
  return { value, reads: () => reads };
}

function mutatingDense(firstValue, inheritedValue) {
  let reads = 0;
  const prototype = Object.create(Array.prototype);
  Object.defineProperty(prototype, "1", {
    configurable: true,
    enumerable: true,
    get() {
      reads += 1;
      return inheritedValue;
    },
  });
  const value = [];
  Object.setPrototypeOf(value, prototype);
  defineData(value, "1", inheritedValue);
  Object.defineProperty(value, "0", {
    configurable: true,
    enumerable: true,
    get() {
      delete value[1];
      return firstValue;
    },
  });
  return { value, reads: () => reads };
}

function mutatingOwnObject(firstValue, inheritedValue) {
  let reads = 0;
  const prototype = {};
  Object.defineProperty(prototype, "b", {
    configurable: true,
    enumerable: true,
    get() {
      reads += 1;
      return inheritedValue;
    },
  });
  const value = Object.create(prototype);
  defineData(value, "b", inheritedValue);
  Object.defineProperty(value, "a", {
    configurable: true,
    enumerable: true,
    get() {
      delete value.b;
      return firstValue;
    },
  });
  return { value, reads: () => reads };
}

function rejectSparseWithoutRead(label, sparse, crossing, ErrorType) {
  let error;
  try {
    crossing(sparse.value);
  } catch (caught) {
    error = caught;
  }
  ok(
    label,
    error instanceof ErrorType &&
      (!(error instanceof M.WireFormatError) || error.code === "wire.malformed") &&
      sparse.reads() === 0,
    `error=${String(error)} inheritedReads=${sparse.reads()}`,
  );
}

function hookedDense(values, placement, hook) {
  let observations = 0;
  const value = [];
  for (let i = 0; i < values.length; i += 1) {
    defineData(value, String(i), values[i]);
  }
  const target = placement === "own" ? value : Object.create(Array.prototype);
  if (hook === "species") {
    const constructor = {};
    Object.defineProperty(constructor, Symbol.species, {
      configurable: true,
      get() {
        observations += 1;
        throw new Error("transferred constructor species was observed");
      },
    });
    defineData(target, "constructor", constructor);
  } else {
    const key = hook === "iterator" ? Symbol.iterator : hook;
    Object.defineProperty(target, key, {
      configurable: true,
      enumerable: hook !== "iterator",
      get() {
        observations += 1;
        throw new Error(`transferred ${hook} hook was observed`);
      },
    });
  }
  if (placement === "inherited") Object.setPrototypeOf(value, target);
  return { value, observations: () => observations };
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
    currentSettles: 0,
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
    decode: options.decode ?? ((value) => value),
    resolve(value) {
      observed.currentSettles += 1;
      observed.current = { kind: "value", value };
    },
    reject(error) {
      observed.currentSettles += 1;
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
    decode: (value) => value,
    resolve(value) {
      observed.first = value;
    },
    reject(error) {
      observed.first = error;
    },
  };
  session.queue = [{
    decode: (value) => value,
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
      decode: (value) => value,
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
      decode: (value) => value,
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

async function malformedReturn(label, data, decode) {
  const { observed, session } = driveReply(
    [{ data, kind: "value" }],
    { decode },
  );
  let later;
  M.Session.prototype.call
    .call(session, "11".repeat(32), [], M.dInt)
    .catch((error) => { later = error; });
  await Promise.resolve();
  ok(
    `return-${label}`,
    observed.current?.kind === "error" &&
      observed.current.error instanceof M.MarrowLossError &&
      observed.current.error.loss === M.LOSS.OUTCOME_UNKNOWN &&
      observed.current.error.cause instanceof M.WireFormatError &&
      observed.current.error.cause.code === "wire.malformed" &&
      observed.queued instanceof M.MarrowLossError &&
      observed.queued.loss === M.LOSS.INTERRUPTED &&
      observed.pump === 0 &&
      session.dead &&
      session.inFlight === null &&
      observed.tornDown &&
      later instanceof M.MarrowLossError &&
      later.loss === M.LOSS.NOT_STARTED,
    `current=${String(observed.current?.error)} pump=${observed.pump} dead=${session.dead}`,
  );
}

for (const [label, data, decode] of [
  ["wrong-primitive", "7", M.dInt],
  ["wrong-list-element", [1n, "2"], M.dList(M.dInt)],
  [
    "record-extra-key",
    { extra: 2n, value: 1n },
    M.dRecord([["value", true, M.dInt]]),
  ],
  [
    "sum-extra-key",
    { extra: null, member: "some", payload: [1n] },
    M.dSum([["some", [M.dInt]]]),
  ],
  ["invalid-date", "2021-02-29", M.dDate],
  ["noncanonical-instant", "2026-07-15T12:00:00.50Z", M.dInstant],
  ["out-of-range-duration", "PT170141183460469231731687303716S", M.dDuration],
  ["overlong-duration", `PT${"9".repeat(31)}S`, M.dDuration],
  ["unexpected-decoder-exception", 1n, () => { throw new TypeError("decoder bug"); }],
]) {
  await malformedReturn(label, data, decode);
}

for (const [label, encode, value] of [
  [
    "record-extra-key",
    M.eRecord([["value", true, M.eInt]]),
    { extra: 2n, value: 1n },
  ],
  [
    "sum-extra-key",
    M.eSum([["some", [M.eInt]]]),
    { extra: null, member: "some", payload: [1n] },
  ],
  ["invalid-date", M.eDate, "2021-02-29"],
  ["noncanonical-instant", M.eInstant, "2026-07-15T12:00:00.50Z"],
  ["out-of-range-duration", M.eDuration, "PT170141183460469231731687303716S"],
  ["overlong-duration", M.eDuration, `PT${"9".repeat(31)}S`],
]) {
  try {
    encode(value);
    ok(`encode-${label}`, false, "accepted an invalid crossing value");
  } catch (error) {
    ok(
      `encode-${label}`,
      error instanceof TypeError,
      String(error),
    );
  }
}

for (const [label, encode, decode, value] of [
  ["date-minimum", M.eDate, M.dDate, "0001-01-01"],
  ["date-leap-day", M.eDate, M.dDate, "2000-02-29"],
  ["date-maximum", M.eDate, M.dDate, "9999-12-31"],
  ["instant-minimum", M.eInstant, M.dInstant, "0001-01-01T00:00:00Z"],
  ["instant-maximum", M.eInstant, M.dInstant, "9999-12-31T23:59:59.999999999Z"],
  [
    "duration-maximum",
    M.eDuration,
    M.dDuration,
    "PT170141183460469231731687303715.884105727S",
  ],
  [
    "duration-minimum",
    M.eDuration,
    M.dDuration,
    "-PT170141183460469231731687303715.884105728S",
  ],
]) {
  try {
    ok(`crossing-${label}`, decode(encode(value)) === value);
  } catch (error) {
    ok(`crossing-${label}`, false, String(error));
  }
}

{
  const encodeNested = M.eRecord([["value", true, M.eInt]]);
  const decodeNested = M.dRecord([["value", true, M.dInt]]);
  const encoders = [
    ["__proto__", true, encodeNested],
    ["constructor", true, M.eInt],
    ["prototype", true, M.eInt],
    ["toString", true, M.eInt],
  ];
  const decoders = [
    ["__proto__", true, decodeNested],
    ["constructor", true, M.dInt],
    ["prototype", true, M.dInt],
    ["toString", true, M.dInt],
  ];
  const input = ownObject([
    ["__proto__", ownObject([["value", 7n]])],
    ["constructor", 8n],
    ["prototype", 9n],
    ["toString", 10n],
  ]);
  let encoded;
  let canonical;
  let decoded;
  let error;
  try {
    encoded = M.eRecord(encoders)(input);
    canonical = M.encodeCanonical(encoded);
    const parsed = M.parseCanonical(canonical);
    decoded = M.dRecord(decoders)(parsed);
  } catch (caught) {
    error = caught;
  }
  const encodedProto = encoded === undefined
    ? undefined
    : Object.getOwnPropertyDescriptor(encoded, "__proto__")?.value;
  const decodedProto = decoded === undefined
    ? undefined
    : Object.getOwnPropertyDescriptor(decoded, "__proto__")?.value;
  ok(
    "record-collision-round-trip",
    error === undefined &&
      canonical.toString("utf8") ===
      '{"__proto__":{"value":7},"constructor":8,"prototype":9,"toString":10}' &&
      Object.getPrototypeOf(encoded) === Object.prototype &&
      Object.getPrototypeOf(decoded) === Object.prototype &&
      normalOwnData(encoded, "__proto__", encodedProto) &&
      normalOwnData(encoded, "constructor", 8n) &&
      normalOwnData(encoded, "prototype", 9n) &&
      normalOwnData(encoded, "toString", 10n) &&
      normalOwnData(encodedProto, "value", 7n) &&
      normalOwnData(decoded, "__proto__", decodedProto) &&
      normalOwnData(decoded, "constructor", 8n) &&
      normalOwnData(decoded, "prototype", 9n) &&
      normalOwnData(decoded, "toString", 10n) &&
      normalOwnData(decodedProto, "value", 7n),
    error === undefined ? canonical.toString("utf8") : String(error),
  );
}

for (const [label, crossing, ErrorType] of [
  ["encode", M.eRecord, TypeError],
  ["decode", M.dRecord, M.WireFormatError],
]) {
  const scalar = label === "encode" ? M.eInt : M.dInt;
  const inherited = ownObject([
    ["__proto__", 1n],
    ["constructor", 2n],
    ["prototype", 3n],
    ["toString", 4n],
  ]);
  for (const name of ["__proto__", "constructor", "prototype", "toString"]) {
    let error;
    try {
      crossing([[name, true, scalar]])(Object.create(inherited));
    } catch (caught) {
      error = caught;
    }
    ok(
      `record-${label}-required-${name}-must-be-own`,
      error instanceof ErrorType,
      String(error),
    );
  }

  const optional = crossing([
    ["__proto__", false, scalar],
    ["constructor", false, scalar],
    ["prototype", false, scalar],
    ["toString", false, scalar],
  ])(Object.create(inherited));
  ok(
    `record-${label}-optional-inherited-fields-absent`,
    Object.getPrototypeOf(optional) === Object.prototype &&
      Object.keys(optional).length === 0,
    Object.keys(optional).join(","),
  );

  const explicitUndefined = Object.create(inherited);
  for (const name of ["__proto__", "constructor", "prototype", "toString"]) {
    defineData(explicitUndefined, name, undefined);
  }
  const omitted = crossing([
    ["__proto__", false, scalar],
    ["constructor", false, scalar],
    ["prototype", false, scalar],
    ["toString", false, scalar],
  ])(explicitUndefined);
  ok(
    `record-${label}-own-undefined-remains-omitted`,
    Object.getPrototypeOf(omitted) === Object.prototype &&
      Object.keys(omitted).length === 0,
  );
}

{
  const data = M.parseCanonical(
    Buffer.from('{"__proto__":{"value":7}}', "utf8"),
  );
  const decode = M.dRecord([
    ["__proto__", true, M.dRecord([["value", true, M.dInt]])],
  ]);
  const { observed, session } = driveReply(
    [{ data, kind: "value" }],
    { decode },
  );
  const value = observed.current?.value;
  const protoField = Object.getOwnPropertyDescriptor(value, "__proto__")?.value;
  ok(
    "reply-valid-record-proto-field-settles-before-pump",
    observed.current?.kind === "value" &&
      observed.currentSettles === 1 &&
      observed.pump === 1 &&
      observed.queued === null &&
      session.queue.length === 1 &&
      !session.dead &&
      !observed.tornDown &&
      Object.getPrototypeOf(value) === Object.prototype &&
      normalOwnData(value, "__proto__", protoField) &&
      normalOwnData(protoField, "value", 7n),
    `settles=${observed.currentSettles} pump=${observed.pump} dead=${session.dead}`,
  );
}

{
  const encoder = M.eId("items", [M.eInt, M.eText]);
  const valid = ownObject([
    ["root", "items"],
    ["key", [7n, "a"]],
    ["extra", "preserved accepted-extra behavior"],
  ]);
  const encoded = encoder(valid);
  ok(
    "identity-own-root-key-and-existing-extra-behavior",
    normalDenseArray(encoded, [7n, "a"]),
  );

  for (const [label, value] of [
    ["wrong-root-brand", ownObject([["root", "other"], ["key", [7n, "a"]]])],
    ["non-array-key", ownObject([["root", "items"], ["key", 7n]])],
    ["short-key-arity", ownObject([["root", "items"], ["key", [7n]]])],
    ["long-key-arity", ownObject([["root", "items"], ["key", [7n, "a", 9n]]])],
  ]) {
    let error;
    try {
      encoder(value);
    } catch (caught) {
      error = caught;
    }
    ok(`identity-preserves-${label}-rejection`, error instanceof TypeError, String(error));
  }

  for (const [missing, value] of [
    [
      "root",
      ownObject(
        [["key", [7n, "a"]]],
        ownObject([["root", "items"]]),
      ),
    ],
    [
      "key",
      ownObject(
        [["root", "items"]],
        ownObject([["key", [7n, "a"]]]),
      ),
    ],
  ]) {
    let error;
    try {
      encoder(value);
    } catch (caught) {
      error = caught;
    }
    ok(
      `identity-${missing}-must-be-own`,
      error instanceof TypeError,
      String(error),
    );
  }

  let inheritedKeyReads = 0;
  const prototype = ownObject([]);
  Object.defineProperty(prototype, "key", {
    configurable: true,
    enumerable: true,
    get() {
      inheritedKeyReads += 1;
      return [7n, "a"];
    },
  });
  const mutating = ownObject([["key", [7n, "a"]]], prototype);
  Object.defineProperty(mutating, "root", {
    configurable: true,
    enumerable: true,
    get() {
      delete mutating.key;
      return "items";
    },
  });
  let mutationError;
  try {
    encoder(mutating);
  } catch (caught) {
    mutationError = caught;
  }
  ok(
    "identity-rechecks-own-key-after-root-getter",
    mutationError instanceof TypeError && inheritedKeyReads === 0,
    `error=${String(mutationError)} inheritedReads=${inheritedKeyReads}`,
  );
}

for (const [label, crossing, ErrorType] of [
  ["encode", M.eSum([["some", [M.eInt]]]), TypeError],
  ["decode", M.dSum([["some", [M.dInt]]]), M.WireFormatError],
]) {
  let inheritedPayloadReads = 0;
  const prototype = ownObject([]);
  Object.defineProperty(prototype, "payload", {
    configurable: true,
    enumerable: true,
    get() {
      inheritedPayloadReads += 1;
      return [1n];
    },
  });
  const value = ownObject([["payload", [1n]]], prototype);
  Object.defineProperty(value, "member", {
    configurable: true,
    enumerable: true,
    get() {
      delete value.payload;
      return "some";
    },
  });
  let error;
  try {
    crossing(value);
  } catch (caught) {
    error = caught;
  }
  ok(
    `sum-${label}-rechecks-own-payload-after-member-getter`,
    error instanceof ErrorType && inheritedPayloadReads === 0,
    `error=${String(error)} inheritedReads=${inheritedPayloadReads}`,
  );
}

rejectSparseWithoutRead(
  "canonical-array-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  (value) => M.encodeCanonical(value),
  TypeError,
);
rejectSparseWithoutRead(
  "encode-list-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  M.eList(M.eInt),
  TypeError,
);
rejectSparseWithoutRead(
  "decode-list-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  M.dList(M.dInt),
  M.WireFormatError,
);
rejectSparseWithoutRead(
  "encode-nested-list-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  (value) => M.eList(M.eList(M.eInt))([value]),
  TypeError,
);
rejectSparseWithoutRead(
  "decode-nested-list-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  (value) => M.dList(M.dList(M.dInt))([value]),
  M.WireFormatError,
);
rejectSparseWithoutRead(
  "encode-map-outer-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, [1n, 2n]),
  M.eMap(M.eInt, M.eInt),
  TypeError,
);
rejectSparseWithoutRead(
  "decode-map-outer-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, [1n, 2n]),
  M.dMap(M.dInt, M.dInt),
  M.WireFormatError,
);
rejectSparseWithoutRead(
  "encode-map-pair-hole-rejects-before-inherited-read",
  inheritedSparse(2, 1, 2n),
  (value) => M.eMap(M.eInt, M.eInt)([defineData(value, "0", 1n)]),
  TypeError,
);
rejectSparseWithoutRead(
  "decode-map-pair-hole-rejects-before-inherited-read",
  inheritedSparse(2, 1, 2n),
  (value) => M.dMap(M.dInt, M.dInt)([defineData(value, "0", 1n)]),
  M.WireFormatError,
);
rejectSparseWithoutRead(
  "encode-identity-key-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  (value) => M.eId("items", [M.eInt])(ownObject([["root", "items"], ["key", value]])),
  TypeError,
);
rejectSparseWithoutRead(
  "decode-identity-key-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  M.dId("items", [M.dInt]),
  M.WireFormatError,
);
rejectSparseWithoutRead(
  "encode-sum-payload-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  (value) => M.eSum([["some", [M.eInt]]])({ member: "some", payload: value }),
  TypeError,
);
rejectSparseWithoutRead(
  "decode-sum-payload-hole-rejects-before-inherited-read",
  inheritedSparse(1, 0, 1n),
  (value) => M.dSum([["some", [M.dInt]]])({ member: "some", payload: value }),
  M.WireFormatError,
);

for (const [label, array, crossing, ErrorType] of [
  ["canonical", mutatingDense(1n, 2n), (value) => M.encodeCanonical(value), TypeError],
  ["encode-list", mutatingDense(1n, 2n), M.eList(M.eInt), TypeError],
  ["decode-list", mutatingDense(1n, 2n), M.dList(M.dInt), M.WireFormatError],
  [
    "encode-map-outer",
    mutatingDense([1n, 2n], [3n, 4n]),
    M.eMap(M.eInt, M.eInt),
    TypeError,
  ],
  [
    "decode-map-outer",
    mutatingDense([1n, 2n], [3n, 4n]),
    M.dMap(M.dInt, M.dInt),
    M.WireFormatError,
  ],
  [
    "encode-map-pair",
    mutatingDense(1n, 2n),
    (value) => M.eMap(M.eInt, M.eInt)([value]),
    TypeError,
  ],
  [
    "decode-map-pair",
    mutatingDense(1n, 2n),
    (value) => M.dMap(M.dInt, M.dInt)([value]),
    M.WireFormatError,
  ],
  [
    "encode-identity-key",
    mutatingDense(1n, 2n),
    (value) => M.eId("items", [M.eInt, M.eInt])(
      ownObject([["root", "items"], ["key", value]]),
    ),
    TypeError,
  ],
  [
    "decode-identity-key",
    mutatingDense(1n, 2n),
    M.dId("items", [M.dInt, M.dInt]),
    M.WireFormatError,
  ],
  [
    "encode-sum-payload",
    mutatingDense(1n, 2n),
    (value) => M.eSum([["some", [M.eInt, M.eInt]]])({
      member: "some",
      payload: value,
    }),
    TypeError,
  ],
  [
    "decode-sum-payload",
    mutatingDense(1n, 2n),
    (value) => M.dSum([["some", [M.dInt, M.dInt]]])({
      member: "some",
      payload: value,
    }),
    M.WireFormatError,
  ],
]) {
  rejectSparseWithoutRead(
    `dense-mutation-${label}-rejects-before-inherited-read`,
    array,
    crossing,
    ErrorType,
  );
}

{
  const mutating = mutatingOwnObject(1n, 2n);
  let error;
  try {
    M.encodeCanonical(mutating.value);
  } catch (caught) {
    error = caught;
  }
  ok(
    "canonical-object-mutation-rejects-before-inherited-read",
    error instanceof TypeError && mutating.reads() === 0,
    `error=${String(error)} inheritedReads=${mutating.reads()}`,
  );
}

{
  const dense = [1n, 2n];
  defineData(dense, "extra", 3n);
  ok(
    "canonical-dense-array-keeps-bytes-and-ignores-extra-property",
    M.encodeCanonical(dense).toString("utf8") === "[1,2]",
  );
  const list = M.eList(M.eInt)(dense);
  ok(
    "list-dense-array-keeps-values-and-ignores-extra-property",
    normalDenseArray(list, [1n, 2n]) && !Object.hasOwn(list, "extra"),
  );
}

const hookCases = [
  ["canonical", [1n], (value) => M.encodeCanonical(value)],
  ["encode-list", [1n], M.eList(M.eInt)],
  ["decode-list", [1n], M.dList(M.dInt)],
  ["encode-map-outer", [[1n, 2n]], M.eMap(M.eInt, M.eInt)],
  ["decode-map-outer", [[1n, 2n]], M.dMap(M.dInt, M.dInt)],
  ["encode-map-pair", [1n, 2n], (value) => M.eMap(M.eInt, M.eInt)([value])],
  ["decode-map-pair", [1n, 2n], (value) => M.dMap(M.dInt, M.dInt)([value])],
  [
    "encode-identity-key",
    [1n],
    (value) => M.eId("items", [M.eInt])(ownObject([["root", "items"], ["key", value]])),
  ],
  ["decode-identity-key", [1n], M.dId("items", [M.dInt])],
  [
    "encode-sum-payload",
    [1n],
    (value) => M.eSum([["some", [M.eInt]]])({ member: "some", payload: value }),
  ],
  [
    "decode-sum-payload",
    [1n],
    (value) => M.dSum([["some", [M.dInt]]])({ member: "some", payload: value }),
  ],
];
for (const [crossingLabel, values, crossing] of hookCases) {
  for (const placement of ["own", "inherited"]) {
    for (const hook of ["map", "iterator", "constructor", "species"]) {
      const instrumented = hookedDense(values, placement, hook);
      let error;
      try {
        crossing(instrumented.value);
      } catch (caught) {
        error = caught;
      }
      ok(
        `dense-hooks-${crossingLabel}-${placement}-${hook}`,
        error === undefined && instrumented.observations() === 0,
        `error=${String(error)} observations=${instrumented.observations()}`,
      );
    }
  }
}

{
  const encodedList = M.eList(M.eInt)([1n, 2n]);
  const decodedList = M.dList(M.dInt)([1n, 2n]);
  const encodedMap = M.eMap(M.eInt, M.eText)([[1n, "a"]]);
  const decodedMap = M.dMap(M.dInt, M.dText)([[1n, "a"]]);
  const encodedId = M.eId("items", [M.eInt])(
    ownObject([["root", "items"], ["key", [1n]]]),
  );
  const decodedId = M.dId("items", [M.dInt])([1n]);
  const encodedSum = M.eSum([["some", [M.eInt]]])({
    member: "some",
    payload: [1n],
  });
  const decodedSum = M.dSum([["some", [M.dInt]]])({
    member: "some",
    payload: [1n],
  });
  ok(
    "dense-crossing-outputs-have-normal-own-index-descriptors",
    normalDenseArray(encodedList, [1n, 2n]) &&
      normalDenseArray(decodedList, [1n, 2n]) &&
      normalDenseArray(encodedMap, [encodedMap[0]]) &&
      normalDenseArray(encodedMap[0], [1n, "a"]) &&
      normalDenseArray(decodedMap, [decodedMap[0]]) &&
      normalDenseArray(decodedMap[0], [1n, "a"]) &&
      normalDenseArray(encodedId, [1n]) &&
      decodedId.root === "items" &&
      normalDenseArray(decodedId.key, [1n]) &&
      encodedSum.member === "some" &&
      normalDenseArray(encodedSum.payload, [1n]) &&
      decodedSum.member === "some" &&
      normalDenseArray(decodedSum.payload, [1n]),
  );
}

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
