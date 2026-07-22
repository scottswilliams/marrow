// The pinned Marrow Node supervision module. Emitted verbatim by
// `marrow client typescript` beside the generated client; the two are a matched
// pair with the runner they were built with.
//
// This module implements the supervised-channel law with Node built-in modules
// only (`node:child_process`, `node:net`, `node:crypto`, `node:process`): it
// spawns the stock runner without a shell, passes a fresh 256-bit launch nonce by
// environment, reads the runner's one-line launch descriptor from stdout, connects
// to the private Unix socket the runner bound before printing that line, proves
// the nonce in `hello`, and verifies the session token and interface identity in
// `ready`. Requests are served by ONE serial worker over a bounded queue; a lost
// reply is classified — never replayed — into the closed loss classes
// `not_started` / `interrupted` / `outcome_unknown` by how far the call had
// progressed when the runner died.
//
// The wire grammar here mirrors the single Rust wire owner (`marrow-local-wire`):
// length-prefixed frames with a version byte, and canonical JSON — object keys in
// ascending byte order, no whitespace, minimal integer spellings (JavaScript
// `bigint`, never a lossy `number`), and the fixed string escapes. This module
// deliberately never invokes the built-in global JSON codec: that grammar is
// neither canonical nor integer-exact, and a second wire grammar is forbidden.
// Client-side validation is a mirror for early failure; the Rust wire owner
// remains authoritative.

import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { randomBytes } from "node:crypto";
import { rmSync } from "node:fs";
import { dirname } from "node:path";
import process from "node:process";

// ---------------------------------------------------------------------------
// Protocol constants (matched with the Rust wire owner).

export const PROTOCOL_VERSION = 1;
export const MAX_FRAME = 1 << 20;
export const MAX_DEPTH = 64;
export const MAX_STRING_BYTES = 64 * 1024;

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

const MAX_QUEUE = 64;
const HANDSHAKE_DEADLINE_MS = 2000;
const REPLY_DEADLINE_MS = 30000;

// ---------------------------------------------------------------------------
// Typed failures.

/** The closed loss classification for a call whose reply never arrived. */
export const LOSS = Object.freeze({
  NOT_STARTED: "not_started",
  INTERRUPTED: "interrupted",
  OUTCOME_UNKNOWN: "outcome_unknown",
});

/** A call was lost to runner death; `loss` is one of the `LOSS` classes. */
export class MarrowLossError extends Error {
  constructor(loss) {
    super(`marrow call lost: ${loss}`);
    this.name = "MarrowLossError";
    this.loss = loss;
  }
}

/** A source-mapped runtime fault from the running export. */
export class MarrowFault extends Error {
  constructor(code, line, column) {
    super(`${code} at ${line}:${column}`);
    this.name = "MarrowFault";
    this.code = code;
    this.line = line;
    this.column = column;
  }
}

/** The runner refused the request (unknown export, argument mismatch, ...). */
export class MarrowReject extends Error {
  constructor(code) {
    super(code);
    this.name = "MarrowReject";
    this.code = code;
  }
}

/** A wire-grammar violation detected on this side; `code` mirrors `wire.*`. */
export class WireFormatError extends Error {
  constructor(code, detail) {
    super(detail ? `${code}: ${detail}` : code);
    this.name = "WireFormatError";
    this.code = code;
  }
}

/** The launch or handshake failed; no call was ever admitted. */
export class LaunchError extends Error {
  constructor(detail) {
    super(detail);
    this.name = "LaunchError";
    this.loss = LOSS.NOT_STARTED;
  }
}

// ---------------------------------------------------------------------------
// Canonical JSON: encode.
//
// A wire value is `null`, a boolean, a `bigint` in the i64 range, a string, an
// array of wire values, or a plain object of wire values. `encodeCanonical`
// emits the one canonical byte spelling; anything else is a `TypeError` (a
// programming error on this side, not a wire rejection).

export function encodeCanonical(value) {
  const out = [];
  encodeInto(value, out);
  return Buffer.from(out.join(""), "utf8");
}

function encodeInto(value, out) {
  if (value === null) {
    out.push("null");
    return;
  }
  switch (typeof value) {
    case "boolean":
      out.push(value ? "true" : "false");
      return;
    case "bigint":
      if (value < I64_MIN || value > I64_MAX) {
        throw new TypeError("marrow int out of the 64-bit range");
      }
      out.push(value.toString(10));
      return;
    case "string":
      encodeString(value, out);
      return;
    case "number":
      throw new TypeError(
        "a marrow int is a bigint on the wire; JavaScript number is not exact",
      );
    case "object":
      break;
    default:
      throw new TypeError(`not a wire value: ${typeof value}`);
  }
  if (Array.isArray(value)) {
    out.push("[");
    for (let i = 0; i < value.length; i += 1) {
      if (i > 0) out.push(",");
      encodeInto(value[i], out);
    }
    out.push("]");
    return;
  }
  const keys = Object.keys(value).sort(byUtf8);
  out.push("{");
  for (let i = 0; i < keys.length; i += 1) {
    if (i > 0) out.push(",");
    encodeString(keys[i], out);
    out.push(":");
    encodeInto(value[keys[i]], out);
  }
  out.push("}");
}

function byUtf8(a, b) {
  const ba = Buffer.from(a, "utf8");
  const bb = Buffer.from(b, "utf8");
  return Buffer.compare(ba, bb);
}

function encodeString(text, out) {
  out.push('"');
  for (const ch of text) {
    const code = ch.codePointAt(0);
    if (ch === '"') out.push('\\"');
    else if (ch === "\\") out.push("\\\\");
    else if (code === 0x08) out.push("\\b");
    else if (code === 0x09) out.push("\\t");
    else if (code === 0x0a) out.push("\\n");
    else if (code === 0x0c) out.push("\\f");
    else if (code === 0x0d) out.push("\\r");
    else if (code < 0x20) out.push(`\\u00${code.toString(16).padStart(2, "0")}`);
    else out.push(ch);
  }
  out.push('"');
}

// ---------------------------------------------------------------------------
// Canonical JSON: strict parse.
//
// Mirrors the Rust owner: a bounded tolerant parse, then the re-encoding must be
// byte-identical or the input is `wire.noncanonical`. Integers must be exact
// i64 (no fraction, exponent, or overflow); strings and depth are bounded.

export function parseCanonical(buf) {
  const text = buf.toString("utf8");
  if (Buffer.from(text, "utf8").length !== buf.length) {
    throw new WireFormatError("wire.malformed", "invalid utf-8");
  }
  const parser = new Parser(text);
  const value = parser.parseValue(0);
  parser.skipWs();
  if (parser.pos !== text.length) {
    throw new WireFormatError("wire.malformed", "trailing bytes");
  }
  if (!encodeCanonical(value).equals(buf)) {
    throw new WireFormatError("wire.noncanonical");
  }
  return value;
}

class Parser {
  constructor(text) {
    this.text = text;
    this.pos = 0;
  }

  skipWs() {
    for (;;) {
      const ch = this.text[this.pos];
      if (ch === " " || ch === "\t" || ch === "\n" || ch === "\r") {
        this.pos += 1;
      } else {
        return;
      }
    }
  }

  peek() {
    return this.text[this.pos];
  }

  parseValue(depth) {
    this.skipWs();
    const ch = this.peek();
    if (ch === undefined) throw new WireFormatError("wire.malformed", "eof");
    if (ch === "{") return this.parseObject(depth);
    if (ch === "[") return this.parseArray(depth);
    if (ch === '"') return this.parseString();
    if (ch === "t") return this.parseLiteral("true", true);
    if (ch === "f") return this.parseLiteral("false", false);
    if (ch === "n") return this.parseLiteral("null", null);
    if (ch === "-" || (ch >= "0" && ch <= "9")) return this.parseNumber();
    throw new WireFormatError("wire.malformed", `unexpected ${ch}`);
  }

  parseLiteral(word, value) {
    if (this.text.startsWith(word, this.pos)) {
      this.pos += word.length;
      return value;
    }
    throw new WireFormatError("wire.malformed", "bad literal");
  }

  parseNumber() {
    const start = this.pos;
    if (this.peek() === "-") this.pos += 1;
    const digitsStart = this.pos;
    while (this.peek() >= "0" && this.peek() <= "9") this.pos += 1;
    if (this.pos === digitsStart) {
      throw new WireFormatError("wire.malformed", "bad number");
    }
    const next = this.peek();
    if (next === "." || next === "e" || next === "E") {
      throw new WireFormatError("wire.malformed", "not an integer");
    }
    const value = BigInt(this.text.slice(start, this.pos));
    if (value < I64_MIN || value > I64_MAX) {
      throw new WireFormatError("wire.malformed", "int out of range");
    }
    return value;
  }

  parseString() {
    this.pos += 1; // consume '"'
    let out = "";
    for (;;) {
      const ch = this.peek();
      if (ch === undefined) {
        throw new WireFormatError("wire.malformed", "unterminated string");
      }
      if (ch === '"') {
        this.pos += 1;
        return out;
      }
      if (ch === "\\") {
        this.pos += 1;
        out += this.parseEscape();
      } else if (ch.codePointAt(0) < 0x20) {
        throw new WireFormatError("wire.malformed", "raw control character");
      } else {
        const cp = this.text.codePointAt(this.pos);
        const glyph = String.fromCodePoint(cp);
        out += glyph;
        this.pos += glyph.length;
      }
      if (Buffer.byteLength(out, "utf8") > MAX_STRING_BYTES) {
        throw new WireFormatError("wire.string_limit");
      }
    }
  }

  parseEscape() {
    const esc = this.peek();
    this.pos += 1;
    switch (esc) {
      case '"':
        return '"';
      case "\\":
        return "\\";
      case "/":
        return "/";
      case "b":
        return "\b";
      case "f":
        return "\f";
      case "n":
        return "\n";
      case "r":
        return "\r";
      case "t":
        return "\t";
      case "u": {
        const code = this.parseHex4();
        if (code >= 0xd800 && code <= 0xdbff) {
          if (this.peek() !== "\\") {
            throw new WireFormatError("wire.malformed", "lone surrogate");
          }
          this.pos += 1;
          if (this.peek() !== "u") {
            throw new WireFormatError("wire.malformed", "lone surrogate");
          }
          this.pos += 1;
          const low = this.parseHex4();
          if (low < 0xdc00 || low > 0xdfff) {
            throw new WireFormatError("wire.malformed", "bad surrogate pair");
          }
          return String.fromCodePoint(
            0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00),
          );
        }
        if (code >= 0xdc00 && code <= 0xdfff) {
          throw new WireFormatError("wire.malformed", "lone low surrogate");
        }
        return String.fromCodePoint(code);
      }
      default:
        throw new WireFormatError("wire.malformed", "bad escape");
    }
  }

  parseHex4() {
    const hex = this.text.slice(this.pos, this.pos + 4);
    if (hex.length !== 4 || !/^[0-9a-fA-F]{4}$/.test(hex)) {
      throw new WireFormatError("wire.malformed", "bad \\u escape");
    }
    this.pos += 4;
    return Number.parseInt(hex, 16);
  }

  parseArray(depth) {
    if (depth + 1 > MAX_DEPTH) throw new WireFormatError("wire.depth_limit");
    this.pos += 1; // consume '['
    const items = [];
    this.skipWs();
    if (this.peek() === "]") {
      this.pos += 1;
      return items;
    }
    for (;;) {
      items.push(this.parseValue(depth + 1));
      this.skipWs();
      const next = this.peek();
      if (next === ",") this.pos += 1;
      else if (next === "]") {
        this.pos += 1;
        return items;
      } else throw new WireFormatError("wire.malformed", "bad array");
    }
  }

  parseObject(depth) {
    if (depth + 1 > MAX_DEPTH) throw new WireFormatError("wire.depth_limit");
    this.pos += 1; // consume '{'
    const out = Object.create(null);
    this.skipWs();
    if (this.peek() === "}") {
      this.pos += 1;
      return out;
    }
    for (;;) {
      this.skipWs();
      if (this.peek() !== '"') {
        throw new WireFormatError("wire.malformed", "bad object key");
      }
      const key = this.parseString();
      if (Object.prototype.hasOwnProperty.call(out, key)) {
        throw new WireFormatError("wire.noncanonical", "duplicate key");
      }
      this.skipWs();
      if (this.peek() !== ":") {
        throw new WireFormatError("wire.malformed", "missing colon");
      }
      this.pos += 1;
      out[key] = this.parseValue(depth + 1);
      this.skipWs();
      const next = this.peek();
      if (next === ",") this.pos += 1;
      else if (next === "}") {
        this.pos += 1;
        return out;
      } else throw new WireFormatError("wire.malformed", "bad object");
    }
  }
}

// ---------------------------------------------------------------------------
// Framing: u32_be(body_len) ‖ u8(version) ‖ canonical json.

export function encodeFrame(value) {
  const json = encodeCanonical(value);
  const bodyLen = 1 + json.length;
  if (bodyLen > MAX_FRAME) throw new WireFormatError("wire.frame_too_large");
  const frame = Buffer.alloc(4 + bodyLen);
  frame.writeUInt32BE(bodyLen, 0);
  frame.writeUInt8(PROTOCOL_VERSION, 4);
  json.copy(frame, 5);
  return frame;
}

/** Incremental frame splitter over a byte stream. `push` returns decoded values. */
class FrameReader {
  constructor() {
    this.buffer = Buffer.alloc(0);
  }

  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    const messages = [];
    for (;;) {
      if (this.buffer.length < 4) return messages;
      const bodyLen = this.buffer.readUInt32BE(0);
      if (bodyLen === 0) throw new WireFormatError("wire.malformed", "empty frame");
      if (bodyLen > MAX_FRAME) throw new WireFormatError("wire.frame_too_large");
      if (this.buffer.length < 4 + bodyLen) return messages;
      const version = this.buffer.readUInt8(4);
      if (version !== PROTOCOL_VERSION) {
        throw new WireFormatError("wire.unsupported_version");
      }
      const json = this.buffer.subarray(5, 4 + bodyLen);
      this.buffer = this.buffer.subarray(4 + bodyLen);
      messages.push(parseCanonical(Buffer.from(json)));
    }
  }
}

// ---------------------------------------------------------------------------
// Transfer-value combinators. The generated client composes these into one
// encoder and one decoder per export signature; each mirrors the runner's typed
// validation so a mismatched argument fails here with a `TypeError` before any
// byte is sent (the runner remains authoritative).

const DATE_TEXT = /^\d{4}-\d{2}-\d{2}$/;
const INSTANT_TEXT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?Z$/;
const DURATION_TEXT = /^-?PT\d+(\.\d{1,9})?S$/;

export function eInt(v) {
  if (typeof v !== "bigint" || v < I64_MIN || v > I64_MAX) {
    throw new TypeError("expected a 64-bit bigint");
  }
  return v;
}

export function eBool(v) {
  if (typeof v !== "boolean") throw new TypeError("expected a boolean");
  return v;
}

export function eText(v) {
  if (typeof v !== "string") throw new TypeError("expected a string");
  return v;
}

export function eBytes(v) {
  if (!(v instanceof Uint8Array)) throw new TypeError("expected a Uint8Array");
  return `0x${Buffer.from(v).toString("hex")}`;
}

export function eDate(v) {
  if (typeof v !== "string" || !DATE_TEXT.test(v)) {
    throw new TypeError("expected a canonical date `YYYY-MM-DD`");
  }
  return v;
}

export function eInstant(v) {
  if (typeof v !== "string" || !INSTANT_TEXT.test(v)) {
    throw new TypeError("expected a canonical UTC instant");
  }
  return v;
}

export function eDuration(v) {
  if (typeof v !== "string" || !DURATION_TEXT.test(v)) {
    throw new TypeError("expected a canonical duration `PT<seconds>S`");
  }
  return v;
}

export function eOpt(inner) {
  return (v) => (v === null ? null : inner(v));
}

/** fields: `[name, required, encode]` triples in declaration order. */
export function eRecord(fields) {
  const names = new Set(fields.map(([name]) => name));
  return (v) => {
    if (v === null || typeof v !== "object" || Array.isArray(v)) {
      throw new TypeError("expected a record object");
    }
    for (const key of Object.keys(v)) {
      if (!names.has(key) && v[key] !== undefined) {
        throw new TypeError(`unknown record field \`${key}\``);
      }
    }
    const out = {};
    for (const [name, required, encode] of fields) {
      const field = v[name];
      if (field === undefined) {
        if (required) throw new TypeError(`missing required field \`${name}\``);
      } else {
        out[name] = encode(field);
      }
    }
    return out;
  };
}

/** variants: `[name, [payload encoders]]` pairs in declaration order. */
export function eSum(variants) {
  return (v) => {
    if (v === null || typeof v !== "object" || typeof v.member !== "string") {
      throw new TypeError("expected an enum value `{ member, payload }`");
    }
    const variant = variants.find(([name]) => name === v.member);
    if (variant === undefined) {
      throw new TypeError(`unknown enum member \`${v.member}\``);
    }
    const [, encoders] = variant;
    const payload = v.payload;
    if (!Array.isArray(payload) || payload.length !== encoders.length) {
      throw new TypeError(`wrong payload arity for \`${v.member}\``);
    }
    return { member: v.member, payload: payload.map((leaf, i) => encoders[i](leaf)) };
  };
}

function protocol(detail) {
  return new WireFormatError("wire.malformed", detail);
}

export function dUnit(d) {
  if (d !== null) throw protocol("expected null");
  return undefined;
}

export function dInt(d) {
  if (typeof d !== "bigint") throw protocol("expected an int");
  return d;
}

export function dBool(d) {
  if (typeof d !== "boolean") throw protocol("expected a bool");
  return d;
}

export function dText(d) {
  if (typeof d !== "string") throw protocol("expected a string");
  return d;
}

export function dBytes(d) {
  if (typeof d !== "string" || !/^0x([0-9a-f]{2})*$/.test(d)) {
    throw protocol("expected 0x-hex bytes");
  }
  return new Uint8Array(Buffer.from(d.slice(2), "hex"));
}

export function dDate(d) {
  if (typeof d !== "string" || !DATE_TEXT.test(d)) throw protocol("expected a date");
  return d;
}

export function dInstant(d) {
  if (typeof d !== "string" || !INSTANT_TEXT.test(d)) {
    throw protocol("expected an instant");
  }
  return d;
}

export function dDuration(d) {
  if (typeof d !== "string" || !DURATION_TEXT.test(d)) {
    throw protocol("expected a duration");
  }
  return d;
}

export function dOpt(inner) {
  return (d) => (d === null ? null : inner(d));
}

export function dRecord(fields) {
  return (d) => {
    if (d === null || typeof d !== "object" || Array.isArray(d)) {
      throw protocol("expected a record");
    }
    const out = {};
    for (const [name, required, decode] of fields) {
      const field = d[name];
      if (field === undefined) {
        if (required) throw protocol(`missing required field \`${name}\``);
      } else {
        out[name] = decode(field);
      }
    }
    return out;
  };
}

export function dSum(variants) {
  return (d) => {
    if (d === null || typeof d !== "object" || typeof d.member !== "string") {
      throw protocol("expected an enum value");
    }
    const variant = variants.find(([name]) => name === d.member);
    if (variant === undefined) throw protocol(`unknown member \`${d.member}\``);
    const [, decoders] = variant;
    if (!Array.isArray(d.payload) || d.payload.length !== decoders.length) {
      throw protocol("wrong payload arity");
    }
    return {
      member: d.member,
      payload: d.payload.map((leaf, i) => decoders[i](leaf)),
    };
  };
}

// ---------------------------------------------------------------------------
// Supervision: launch, session, serial worker, bounded queue, loss classes.

/**
 * Launch the stock runner and open one authenticated session.
 *
 * options:
 * - `runner`: path to the `marrow-runner` executable;
 * - `image`: path to the compiled program image the runner serves;
 * - `store` (optional): path to a provisioned persistent store directory. When
 *   present, the runner is spawned as a native attached session over that store
 *   (`attach --image <image> --store <store>`); when absent, a storeless session
 *   (`--image <image>`). The store path is chosen by this trusted-main config,
 *   never by the calling renderer.
 * - `log` (optional): `(chunk: Buffer) => void` receiving drained runner
 *   stderr/extra-stdout bytes (byte-clean; never interleaved with protocol).
 *
 * Resolves to a `Session` after `ready` proves the session token, or rejects
 * with a `LaunchError` (loss class `not_started`: no call was ever admitted).
 */
export function launch(options) {
  return new Promise((resolve, reject) => {
    const nonce = randomBytes(32).toString("hex");
    const log = options.log ?? (() => {});
    // The store, when set, selects the native attached-session launch. Both
    // launches pass the image by `--image`; neither exposes a shell, an
    // environment beyond the nonce, or a data path to the renderer.
    const argv =
      options.store === undefined
        ? ["--image", options.image]
        : ["attach", "--image", options.image, "--store", options.store];
    const child = spawn(options.runner, argv, {
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, MARROW_RUNNER_NONCE: nonce },
    });

    let settled = false;
    let stdoutBuffer = Buffer.alloc(0);
    let descriptor = null;

    // The runner cleans its private socket directory on its own orderly exit,
    // but a fail-closed SIGKILL leaves it no chance — so the supervisor also
    // removes the directory, completing the channel law's cleanup obligation.
    const removeChannelDir = () => {
      if (descriptor !== null && typeof descriptor.socket === "string") {
        try {
          rmSync(dirname(descriptor.socket), { recursive: true, force: true });
        } catch {
          // Best effort: the runner may already have removed it.
        }
      }
    };

    const failLaunch = (detail) => {
      if (settled) return;
      settled = true;
      child.kill("SIGKILL");
      removeChannelDir();
      reject(new LaunchError(detail));
    };

    const deadline = setTimeout(
      () => failLaunch("handshake deadline elapsed"),
      HANDSHAKE_DEADLINE_MS,
    );
    deadline.unref?.();

    child.on("error", (error) => failLaunch(`spawn failed: ${error.message}`));
    child.on("exit", () => {
      if (!settled) failLaunch("runner exited before the handshake completed");
    });
    child.stderr.on("data", log);

    child.stdout.on("data", (chunk) => {
      if (descriptor !== null) {
        log(chunk); // post-descriptor stdout is drained log bytes
        return;
      }
      stdoutBuffer = Buffer.concat([stdoutBuffer, chunk]);
      const newline = stdoutBuffer.indexOf(0x0a);
      if (newline === -1) return;
      const line = stdoutBuffer.subarray(0, newline);
      const rest = stdoutBuffer.subarray(newline + 1);
      try {
        descriptor = parseCanonical(Buffer.from(line));
      } catch (error) {
        failLaunch(`bad launch descriptor: ${error.message}`);
        return;
      }
      if (rest.length > 0) log(rest);
      connect();
    });

    const connect = () => {
      if (
        typeof descriptor.socket !== "string" ||
        typeof descriptor.session !== "string" ||
        typeof descriptor.interface !== "string"
      ) {
        failLaunch("incomplete launch descriptor");
        return;
      }
      const socket = createConnection(descriptor.socket);
      const frames = new FrameReader();
      socket.on("error", (error) => failLaunch(`connect failed: ${error.message}`));
      socket.on("connect", () => {
        socket.write(encodeFrame({ kind: "hello", nonce }));
      });
      socket.on("data", (chunk) => {
        if (settled) return;
        let messages;
        try {
          messages = frames.push(chunk);
        } catch (error) {
          failLaunch(`bad ready frame: ${error.message}`);
          return;
        }
        if (messages.length === 0) return;
        const ready = messages[0];
        if (
          ready.kind !== "ready" ||
          ready.session !== descriptor.session ||
          typeof ready.interface !== "string"
        ) {
          failLaunch("handshake refused");
          return;
        }
        settled = true;
        clearTimeout(deadline);
        socket.removeAllListeners("data");
        socket.removeAllListeners("error");
        resolve(
          new Session(child, socket, frames, ready.interface, descriptor.socket, log),
        );
      });
      socket.on("close", () => {
        if (!settled) failLaunch("socket closed during the handshake");
      });
    };
  });
}

/**
 * Provision a fresh persistent store for an image, one-shot.
 *
 * options:
 * - `runner`: path to the `marrow-runner` executable;
 * - `image`: path to the compiled program image to provision the store for;
 * - `store`: the destination store directory (must not already exist);
 * - `log` (optional): receives the runner's provision report (its stderr bytes).
 *
 * Spawns `provision --image <image> --store <store> --yes` without a shell, with
 * the child's stdin closed, accepting the exact provision report and publishing
 * the store. Resolves the parsed one-line JSON receipt (`{ instance, store }`)
 * the runner prints on a clean exit, or rejects with a `LaunchError` on a
 * non-zero exit or a spawn failure. This is a one-shot lifecycle action, not a
 * `Session`: no channel is bound and no call is served. The destination is
 * chosen by this trusted-main config, never by a calling renderer.
 */
export function provision(options) {
  return new Promise((resolve, reject) => {
    const log = options.log ?? (() => {});
    const child = spawn(
      options.runner,
      ["provision", "--image", options.image, "--store", options.store, "--yes"],
      { shell: false, stdio: ["ignore", "pipe", "pipe"], env: { ...process.env } },
    );
    let stdout = Buffer.alloc(0);
    let settled = false;
    const fail = (detail) => {
      if (settled) return;
      settled = true;
      reject(new LaunchError(detail));
    };
    child.on("error", (error) => fail(`provision spawn failed: ${error.message}`));
    child.stderr.on("data", log);
    child.stdout.on("data", (chunk) => {
      stdout = Buffer.concat([stdout, chunk]);
    });
    child.on("exit", (code) => {
      if (settled) return;
      if (code !== 0) {
        fail(`provision exited with code ${code}`);
        return;
      }
      // The receipt is the one canonical JSON line the runner prints on stdout;
      // the human-readable report went to stderr (the `log`).
      const line = stdout.toString("utf8").split("\n").filter((l) => l.length > 0).pop();
      if (line === undefined) {
        fail("provision printed no receipt");
        return;
      }
      let receipt;
      try {
        receipt = parseCanonical(Buffer.from(line, "utf8"));
      } catch (error) {
        fail(`bad provision receipt: ${error.message}`);
        return;
      }
      settled = true;
      resolve(receipt);
    });
  });
}

/** One authenticated attached session over the private socket. */
export class Session {
  constructor(child, socket, frames, interfaceId, socketPath, log) {
    this.child = child;
    this.socket = socket;
    this.frames = frames;
    this.interfaceId = interfaceId;
    this.socketPath = socketPath;
    this.log = log;
    this.dead = false;
    /** The dispatched call awaiting its reply, or null. */
    this.inFlight = null;
    /** Admitted calls not yet handed to the serial worker. */
    this.queue = [];

    // Explicit fail-closed teardown on process exit (a g02p carry-forward: no
    // reliance on implicit cleanup). SIGKILL is safe: the runner holds no state.
    this.exitHook = () => this.terminate();
    process.on("exit", this.exitHook);

    const die = () => this.fail();
    this.socket.on("data", (chunk) => this.onData(chunk));
    this.socket.on("close", die);
    this.socket.on("error", die);
    this.child.on("exit", die);

    this.replyDeadline = null;
  }

  /**
   * Invoke `exportId` (64 lowercase hex) with already-encoded wire arguments.
   * Resolves with the reply's `data`, or rejects with `MarrowFault`,
   * `MarrowReject`, `WireFormatError`, or `MarrowLossError`.
   */
  call(exportId, args) {
    return new Promise((resolve, reject) => {
      if (this.dead) {
        reject(new MarrowLossError(LOSS.NOT_STARTED));
        return;
      }
      if (this.queue.length >= MAX_QUEUE) {
        reject(new RangeError("marrow call queue is full"));
        return;
      }
      this.queue.push({ exportId, args, resolve, reject });
      this.pump();
    });
  }

  /** Hand the next queued call to the serial worker when it is idle. */
  pump() {
    if (this.dead || this.inFlight !== null) return;
    const next = this.queue.shift();
    if (next === undefined) return;
    let frame;
    try {
      frame = encodeFrame({
        args: next.args,
        export: next.exportId,
        kind: "request",
      });
    } catch (error) {
      next.reject(error);
      this.pump();
      return;
    }
    // The call is dispatched the moment its bytes are handed to the socket.
    this.inFlight = next;
    this.replyDeadline = setTimeout(() => this.terminate(), REPLY_DEADLINE_MS);
    this.replyDeadline.unref?.();
    this.socket.write(frame);
  }

  onData(chunk) {
    let messages;
    try {
      messages = this.frames.push(chunk);
    } catch (error) {
      // The reply stream is no longer trustworthy: fail the in-flight call with
      // the wire rejection and close fail-closed.
      const pending = this.inFlight;
      this.inFlight = null;
      if (pending !== null) pending.reject(error);
      this.terminate();
      return;
    }
    for (const message of messages) {
      const pending = this.inFlight;
      if (pending === null) continue; // an unsolicited frame; nothing to settle
      this.inFlight = null;
      clearTimeout(this.replyDeadline);
      if (message.kind === "value") {
        pending.resolve(message.data);
      } else if (
        message.kind === "fault" &&
        typeof message.code === "string" &&
        message.span !== null &&
        typeof message.span === "object"
      ) {
        pending.reject(
          new MarrowFault(message.code, message.span.line, message.span.column),
        );
      } else if (message.kind === "reject" && typeof message.code === "string") {
        pending.reject(new MarrowReject(message.code));
      } else {
        pending.reject(protocol("unknown reply kind"));
      }
      this.pump();
    }
  }

  /** Classify and fail every outstanding call after runner/socket death. */
  fail() {
    if (this.dead) return;
    this.dead = true;
    clearTimeout(this.replyDeadline);
    const inFlight = this.inFlight;
    const queued = this.queue;
    this.inFlight = null;
    this.queue = [];
    if (inFlight !== null) {
      inFlight.reject(new MarrowLossError(LOSS.OUTCOME_UNKNOWN));
    }
    for (const call of queued) {
      call.reject(new MarrowLossError(LOSS.INTERRUPTED));
    }
    this.teardown();
  }

  /** Graceful close: hang up, then wait for the runner to exit (it exits on a
   * clean client hangup), escalating to SIGKILL on a deadline. */
  close() {
    return new Promise((resolve) => {
      if (this.dead) {
        resolve(undefined);
        return;
      }
      this.dead = true;
      clearTimeout(this.replyDeadline);
      const failEverything = () => {
        if (this.inFlight !== null) {
          this.inFlight.reject(new MarrowLossError(LOSS.OUTCOME_UNKNOWN));
          this.inFlight = null;
        }
        for (const call of this.queue) {
          call.reject(new MarrowLossError(LOSS.INTERRUPTED));
        }
        this.queue = [];
      };
      failEverything();
      const forceKill = setTimeout(() => this.child.kill("SIGKILL"), 2000);
      forceKill.unref?.();
      this.child.on("exit", () => {
        clearTimeout(forceKill);
        this.teardown();
        resolve(undefined);
      });
      this.socket.end();
    });
  }

  /** Immediate fail-closed shutdown: SIGKILL the runner, destroy the socket,
   * and classify every outstanding call by its handoff stage. */
  terminate() {
    if (!this.dead) {
      this.fail();
      return;
    }
    this.teardown();
  }

  teardown() {
    this.dead = true;
    process.removeListener("exit", this.exitHook);
    this.child.kill("SIGKILL");
    this.socket.destroy();
    // The runner cannot remove its private directory after a SIGKILL, so the
    // supervisor completes the channel law's cleanup obligation.
    try {
      rmSync(dirname(this.socketPath), { recursive: true, force: true });
    } catch {
      // Best effort: the runner may already have removed it on an orderly exit.
    }
  }
}
