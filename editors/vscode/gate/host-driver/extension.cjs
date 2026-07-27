"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { performance } = require("node:perf_hooks");

const DRIVER_EXTENSION_ID = "marrow-project.marrow-vsq-host-driver";
const DEFAULT_TARGET_EXTENSION_ID = "marrow-project.marrow";
const VIRTUAL_SCHEME = "marrow-vsq";
const EVIDENCE_LIMIT_BYTES = 4 * 1024 * 1024;
const EVIDENCE_RECORD_LIMIT_BYTES = 64 * 1024;
const EVIDENCE_RECORD_LIMIT = 256;
const INPUT_JSON_LIMIT_BYTES = 1024 * 1024;
const CONTROL_LIMIT_BYTES = 1024 * 1024;
const CONTROL_LINE_LIMIT_BYTES = 64 * 1024;
const CONTROL_RECORD_LIMIT = 256;
const DEFAULT_TIMEOUT_MS = 4_000;
const SHA256_PATTERN = /^[0-9a-f]{64}$/u;
const LOCK_WAIT_TIMEOUT_MS = 4_000;
const LOCK_WAIT_ARRAY = new Int32Array(new SharedArrayBuffer(4));

let activeRuntime;

function plainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function requirePlainObject(value, name) {
  if (!plainObject(value)) {
    throw new TypeError(`${name} must be an object`);
  }
  return value;
}

function requireAbsolutePath(value, name) {
  if (typeof value !== "string" || !path.isAbsolute(value)) {
    throw new TypeError(`${name} must be an absolute path`);
  }
  return value;
}

function boundedInteger(value, name, minimum, maximum) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new RangeError(`${name} must be an integer in ${minimum}..${maximum}`);
  }
  return value;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function readBoundedJsonRecord(file, maxBytes = INPUT_JSON_LIMIT_BYTES) {
  requireAbsolutePath(file, "JSON input path");
  boundedInteger(maxBytes, "JSON byte limit", 1, EVIDENCE_LIMIT_BYTES);
  const pathInfo = fs.lstatSync(file);
  if (!pathInfo.isFile() || pathInfo.isSymbolicLink()) {
    throw new RangeError("JSON input must be a regular non-symlink file");
  }
  let descriptor;
  try {
    descriptor = fs.openSync(
      file,
      fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0),
    );
    const opened = fs.fstatSync(descriptor);
    if (
      !opened.isFile() ||
      opened.size === 0 ||
      opened.size > maxBytes ||
      opened.dev !== pathInfo.dev ||
      opened.ino !== pathInfo.ino
    ) {
      throw new RangeError(`JSON input must be a nonempty regular file of at most ${maxBytes} bytes`);
    }
    const bytes = fs.readFileSync(descriptor);
    if (bytes.length !== opened.size) {
      throw new RangeError("JSON input changed while reading");
    }
    return {
      bytes,
      value: requirePlainObject(JSON.parse(bytes.toString("utf8")), "JSON input"),
    };
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
}

function readBoundedJson(file, maxBytes = INPUT_JSON_LIMIT_BYTES) {
  return readBoundedJsonRecord(file, maxBytes).value;
}

function clockSampleAuthorityRoot() {
  const controlPath = requireAbsolutePath(
    process.env.MARROW_VSQ_CONTROL_PATH,
    "clock sample control path",
  );
  return path.dirname(controlPath);
}

function clockSampleSpec(args, authorityRoot = clockSampleAuthorityRoot()) {
  requirePlainObject(args, "clock sample args");
  assert.deepEqual(
    Object.keys(args).sort(),
    ["delayMs", "endpoint", "expectedSpecSha256", "specPath"],
    "clock sample args have an unknown or missing field",
  );
  if (!["format", "hover", "definition", "updatedDiagnostics"].includes(args.endpoint)) {
    throw new TypeError("clock sample endpoint is invalid");
  }
  const delayMs = boundedInteger(args.delayMs, "clock sample delay", 0, 60_000);
  if (!SHA256_PATTERN.test(args.expectedSpecSha256)) {
    throw new TypeError("clock sample expected digest is invalid");
  }
  const specPath = requireAbsolutePath(args.specPath, "clock sample spec path");
  const root = requireAbsolutePath(authorityRoot, "clock sample authority root");
  const rootReal = fs.realpathSync(root);
  const parentReal = fs.realpathSync(path.dirname(specPath));
  const info = fs.lstatSync(specPath);
  if (
    rootReal !== path.resolve(root) ||
    parentReal !== rootReal ||
    !info.isFile() ||
    info.isSymbolicLink() ||
    (info.mode & 0o777) !== 0o600
  ) {
    throw new Error("clock sample spec escaped its isolated mode-0600 authority");
  }
  const input = readBoundedJsonRecord(specPath);
  const actualSha256 = sha256(input.bytes);
  if (actualSha256 !== args.expectedSpecSha256) {
    throw new Error("clock sample spec digest differs from its invocation authority");
  }
  const spec = input.value;
  if (Object.hasOwn(spec, "clockSpecSha256") || Object.hasOwn(spec, "delayMsByEndpoint")) {
    throw new TypeError("clock sample base spec contains invocation-owned fields");
  }
  return {
    ...spec,
    clockSpecSha256: actualSha256,
    delayMsByEndpoint: delayMs === 0 ? {} : { [args.endpoint]: delayMs },
  };
}

function acquireExclusiveFileLock(lockPath, name) {
  const deadline = Date.now() + LOCK_WAIT_TIMEOUT_MS;
  while (true) {
    try {
      const descriptor = fs.openSync(
        lockPath,
        fs.constants.O_WRONLY |
          fs.constants.O_CREAT |
          fs.constants.O_EXCL |
          (fs.constants.O_NOFOLLOW ?? 0),
        0o600,
      );
      const opened = fs.fstatSync(descriptor);
      if (!opened.isFile() || (opened.mode & 0o777) !== 0o600) {
        fs.closeSync(descriptor);
        fs.rmSync(lockPath, { force: true });
        throw new Error(`${name} lock is not a mode-0600 regular file`);
      }
      return descriptor;
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
      if (Date.now() >= deadline) throw new Error(`${name} lock acquisition timed out`);
      Atomics.wait(LOCK_WAIT_ARRAY, 0, 0, 5);
    }
  }
}

function withExclusiveFileLock(lockPath, name, operation) {
  let lock;
  let primaryError;
  try {
    lock = acquireExclusiveFileLock(lockPath, name);
    return operation();
  } catch (error) {
    primaryError = error;
    throw error;
  } finally {
    let cleanupError;
    for (const cleanup of [
      () => lock !== undefined && fs.closeSync(lock),
      () => lock !== undefined && fs.rmSync(lockPath, { force: true }),
    ]) {
      try {
        cleanup();
      } catch (error) {
        cleanupError ??= error;
      }
    }
    if (primaryError === undefined && cleanupError !== undefined) throw cleanupError;
  }
}

function validateEvidenceRecord(record, expectedSequence) {
  requirePlainObject(record, "evidence record");
  assert.deepEqual(
    Object.keys(record).sort(),
    ["data", "event", "schema", "sequence"],
    "evidence record shape is not closed",
  );
  if (
    record.schema !== 1 ||
    record.sequence !== expectedSequence ||
    typeof record.event !== "string" ||
    !/^[a-z][a-z0-9.-]{0,63}$/u.test(record.event)
  ) {
    throw new TypeError("evidence record identity is invalid");
  }
  requirePlainObject(record.data, "evidence data");
}

function readEvidenceState(file, maxBytes, maxRecordBytes) {
  if (!fs.existsSync(file)) {
    return { bytes: 0, records: 0, device: undefined, inode: undefined };
  }
  const pathInfo = fs.lstatSync(file);
  if (!pathInfo.isFile() || pathInfo.isSymbolicLink()) {
    throw new RangeError("evidence path is not a mode-0600 regular file");
  }
  let descriptor;
  try {
    descriptor = fs.openSync(file, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
    const opened = fs.fstatSync(descriptor);
    if (
      !opened.isFile() ||
      opened.dev !== pathInfo.dev ||
      opened.ino !== pathInfo.ino ||
      (opened.mode & 0o777) !== 0o600 ||
      opened.size > maxBytes
    ) {
      throw new RangeError("evidence path is not a bounded mode-0600 regular file");
    }
    const body = fs.readFileSync(descriptor);
    const afterRead = fs.fstatSync(descriptor);
    const afterPath = fs.lstatSync(file);
    if (
      body.length !== opened.size ||
      afterRead.size !== opened.size ||
      afterPath.dev !== opened.dev ||
      afterPath.ino !== opened.ino ||
      (afterPath.mode & 0o777) !== 0o600 ||
      (body.length > 0 && body.at(-1) !== 0x0a)
    ) {
      throw new RangeError("evidence changed or contains a partial record while reading");
    }
    const text = body.toString("utf8");
    if (!Buffer.from(text, "utf8").equals(body)) {
      throw new RangeError("evidence contains invalid UTF-8");
    }
    const lines = body.length === 0 ? [] : text.split("\n").slice(0, -1);
    if (lines.length > EVIDENCE_RECORD_LIMIT) {
      throw new RangeError("evidence record capacity exceeded");
    }
    for (const [sequence, line] of lines.entries()) {
      if (Buffer.byteLength(line) + 1 > maxRecordBytes) {
        throw new RangeError("evidence record exceeds its byte limit");
      }
      validateEvidenceRecord(JSON.parse(line), sequence);
    }
    return {
      bytes: body.length,
      records: lines.length,
      device: opened.dev,
      inode: opened.ino,
    };
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
}

function createEvidenceWriter(file, options = {}) {
  requireAbsolutePath(file, "evidence path");
  requirePlainObject(options, "evidence options");
  const maxBytes = boundedInteger(
    options.maxBytes ?? EVIDENCE_LIMIT_BYTES,
    "evidence byte limit",
    1,
    EVIDENCE_LIMIT_BYTES,
  );
  const maxRecordBytes = boundedInteger(
    options.maxRecordBytes ?? Math.min(EVIDENCE_RECORD_LIMIT_BYTES, maxBytes),
    "evidence record byte limit",
    1,
    Math.min(EVIDENCE_RECORD_LIMIT_BYTES, maxBytes),
  );
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const parent = fs.lstatSync(path.dirname(file));
  if (!parent.isDirectory() || parent.isSymbolicLink()) {
    throw new Error("evidence parent is not a real directory");
  }
  const lockPath = `${file}.lock`;
  withExclusiveFileLock(lockPath, "evidence", () => {
    readEvidenceState(file, maxBytes, maxRecordBytes);
  });
  return Object.freeze({
    file,
    get bytes() {
      return withExclusiveFileLock(lockPath, "evidence", () =>
        readEvidenceState(file, maxBytes, maxRecordBytes).bytes);
    },
    emit(event, data = {}) {
      if (typeof event !== "string" || !/^[a-z][a-z0-9.-]{0,63}$/u.test(event)) {
        throw new TypeError("evidence event must be a bounded lowercase identifier");
      }
      requirePlainObject(data, "evidence data");
      withExclusiveFileLock(lockPath, "evidence", () => {
        const state = readEvidenceState(file, maxBytes, maxRecordBytes);
        const line = Buffer.from(`${JSON.stringify({
          schema: 1,
          sequence: state.records,
          event,
          data,
        })}\n`, "utf8");
        if (
          line.length > maxRecordBytes ||
          state.records + 1 > EVIDENCE_RECORD_LIMIT ||
          state.bytes + line.length > maxBytes
        ) {
          throw new RangeError("bounded evidence capacity exceeded");
        }
        let descriptor;
        try {
          descriptor = fs.openSync(
            file,
            fs.constants.O_WRONLY |
              fs.constants.O_APPEND |
              fs.constants.O_CREAT |
              (fs.constants.O_NOFOLLOW ?? 0),
            0o600,
          );
          const opened = fs.fstatSync(descriptor);
          if (
            !opened.isFile() ||
            (opened.mode & 0o777) !== 0o600 ||
            opened.size !== state.bytes ||
            (state.device !== undefined &&
              (opened.dev !== state.device || opened.ino !== state.inode))
          ) {
            throw new Error("evidence changed before append");
          }
          if (fs.writeSync(descriptor, line) !== line.length) {
            throw new Error("evidence write was short");
          }
          fs.fsyncSync(descriptor);
          const afterWrite = fs.fstatSync(descriptor);
          const afterPath = fs.lstatSync(file);
          if (
            afterWrite.size !== state.bytes + line.length ||
            afterPath.dev !== afterWrite.dev ||
            afterPath.ino !== afterWrite.ino ||
            (afterPath.mode & 0o777) !== 0o600
          ) {
            throw new Error("evidence size changed during append");
          }
        } finally {
          if (descriptor !== undefined) fs.closeSync(descriptor);
        }
      });
    },
  });
}

function normalizeCompletionItems(value) {
  if (value === undefined || value === null) {
    return [];
  }
  if (Array.isArray(value)) {
    return value;
  }
  if (plainObject(value) && Array.isArray(value.items)) {
    return value.items;
  }
  throw new TypeError("completion result is neither a list nor CompletionList");
}

function completionLabel(item) {
  const label = item?.label;
  if (typeof label === "string") {
    return label;
  }
  if (plainObject(label) && typeof label.label === "string") {
    return label.label;
  }
  throw new TypeError("completion item has no string label");
}

function parseControlLine(line) {
  if (typeof line !== "string" || Buffer.byteLength(line) + 1 > CONTROL_LINE_LIMIT_BYTES) {
    throw new RangeError("control record exceeds its byte limit");
  }
  const parsed = requirePlainObject(JSON.parse(line), "control record");
  const keys = Object.keys(parsed).sort();
  assert.deepEqual(keys, ["args", "id", "op"], "control record has an unknown or missing field");
  if (typeof parsed.id !== "string" || !/^[A-Za-z0-9._-]{1,64}$/.test(parsed.id)) {
    throw new TypeError("control id is invalid");
  }
  if (typeof parsed.op !== "string" || !/^[a-z][A-Za-z0-9.]{0,63}$/.test(parsed.op)) {
    throw new TypeError("control operation is invalid");
  }
  requirePlainObject(parsed.args, "control args");
  return parsed;
}

function delay(ms) {
  boundedInteger(ms, "delay", 0, 60_000);
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(name, predicate, options = {}) {
  const timeoutMs = boundedInteger(
    options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    `${name} timeout`,
    1,
    120_000,
  );
  const intervalMs = boundedInteger(options.intervalMs ?? 10, `${name} interval`, 1, 1_000);
  const deadline = performance.now() + timeoutMs;
  let lastError;
  while (performance.now() <= deadline) {
    try {
      const value = await predicate();
      if (value) {
        return value;
      }
    } catch (error) {
      if (error?.code === "host.signal") throw error;
      lastError = error;
    }
    await delay(intervalMs);
  }
  if (lastError) {
    throw new Error(`${name} timed out after predicate error: ${safeError(lastError).message}`);
  }
  throw new Error(`${name} timed out after ${timeoutMs} ms`);
}

function safeError(error) {
  const name = typeof error?.name === "string" ? error.name.slice(0, 80) : "Error";
  let message = typeof error?.message === "string" ? error.message : String(error);
  message = message
    .replace(/file:\/\/\/?[^\s)]+/gu, "<uri>")
    .replace(/\/(?:[^\s/:]+\/)+[^\s/:]+/gu, "<path>")
    .replace(/[\r\n]+/gu, " ")
    .slice(0, 512);
  const result = { name, message };
  if (typeof error?.code === "string" && /^[a-z][a-z0-9._-]{0,79}$/u.test(error.code)) {
    result.code = error.code;
  }
  return result;
}

function toPosition(vscode, raw, name = "position") {
  requirePlainObject(raw, name);
  return new vscode.Position(
    boundedInteger(raw.line, `${name}.line`, 0, 10_000_000),
    boundedInteger(raw.character, `${name}.character`, 0, 10_000_000),
  );
}

function wholeDocumentRange(vscode, document) {
  return new vscode.Range(document.positionAt(0), document.positionAt(document.getText().length));
}

function uriFromReference(vscode, reference) {
  requirePlainObject(reference, "document reference");
  const keys = [reference.path !== undefined, reference.uri !== undefined].filter(Boolean).length;
  if (keys !== 1) {
    throw new TypeError("document reference must contain exactly one of path or uri");
  }
  if (reference.path !== undefined) {
    return vscode.Uri.file(requireAbsolutePath(reference.path, "document path"));
  }
  if (typeof reference.uri !== "string" || reference.uri.length > 16_384) {
    throw new TypeError("document URI is invalid");
  }
  return vscode.Uri.parse(reference.uri, true);
}

async function openShownDocument(vscode, reference, expectedLanguageId) {
  const document = await vscode.workspace.openTextDocument(uriFromReference(vscode, reference));
  const editor = await vscode.window.showTextDocument(document, { preview: false });
  if (expectedLanguageId !== undefined) {
    assert.equal(document.languageId, expectedLanguageId, "unexpected document language ID");
  }
  return { document, editor };
}

async function replaceDocument(vscode, document, text) {
  if (typeof text !== "string") {
    throw new TypeError("replacement text must be a string");
  }
  const edit = new vscode.WorkspaceEdit();
  edit.replace(document.uri, wholeDocumentRange(vscode, document), text);
  assert.equal(await vscode.workspace.applyEdit(edit), true, "workspace edit was refused");
  await waitFor("document replacement", () => document.getText() === text);
}

async function forceDirtyText(vscode, document, text) {
  if (document.getText() === text && !document.isDirty) {
    const end = document.positionAt(document.getText().length);
    const edit = new vscode.WorkspaceEdit();
    edit.insert(document.uri, end, " ");
    assert.equal(await vscode.workspace.applyEdit(edit), true, "dirtying edit was refused");
  }
  await replaceDocument(vscode, document, text);
  assert.equal(document.isDirty, true, "document must remain dirty after the overlay edit");
}

function timedDelayMs(spec, endpoint) {
  const delays = spec?.delayMsByEndpoint;
  if (!plainObject(delays) || delays[endpoint] === undefined) {
    return 0;
  }
  return boundedInteger(delays[endpoint], `${endpoint} injected delay`, 0, 60_000);
}

async function optionalTimedDelay(spec, endpoint) {
  const delayMs = timedDelayMs(spec, endpoint);
  if (delayMs > 0) await delay(delayMs);
}

function targetExtension(vscode, targetExtensionId = DEFAULT_TARGET_EXTENSION_ID) {
  if (typeof targetExtensionId !== "string" || targetExtensionId.length > 256) {
    throw new TypeError("target extension ID is invalid");
  }
  return vscode.extensions.getExtension(targetExtensionId);
}

function stateSnapshot(vscode, targetExtensionId = DEFAULT_TARGET_EXTENSION_ID) {
  const target = targetExtension(vscode, targetExtensionId);
  const folders = vscode.workspace.workspaceFolders ?? [];
  return {
    trusted: vscode.workspace.isTrusted,
    targetInstalled: target !== undefined,
    targetActive: target?.isActive === true,
    workspaceFolderCount: folders.length,
    workspaceSchemes: [...new Set(folders.map((folder) => folder.uri.scheme))].sort(),
    targetExtensionPathHash: target ? sha256(target.extensionPath) : null,
    targetExtensionRealPathHash: target ? sha256(fs.realpathSync(target.extensionPath)) : null,
    marrowDocumentCount: vscode.workspace.textDocuments.filter(
      (document) => document.languageId === "marrow",
    ).length,
  };
}

async function waitForTargetActivation(vscode, targetExtensionId, timeoutMs) {
  const target = targetExtension(vscode, targetExtensionId);
  assert.ok(target, `target extension ${targetExtensionId} is not installed`);
  await waitFor("target extension activation", () => target.isActive, { timeoutMs });
}

function signatureLabel(signature) {
  return typeof signature?.label === "string" ? signature.label : "";
}

function hoverText(value) {
  const hovers = Array.isArray(value) ? value : value ? [value] : [];
  const pieces = [];
  for (const hover of hovers) {
    const contents = Array.isArray(hover?.contents) ? hover.contents : [hover?.contents];
    for (const content of contents) {
      if (typeof content === "string") {
        pieces.push(content);
      } else if (typeof content?.value === "string") {
        pieces.push(content.value);
      }
    }
  }
  return pieces.join("\n");
}

function definitionLocations(value) {
  if (value === undefined || value === null) {
    return [];
  }
  return Array.isArray(value) ? value : [value];
}

function definitionUri(location) {
  return location?.uri ?? location?.targetUri;
}

function definitionSelectionRange(location) {
  return location?.range ?? location?.targetSelectionRange;
}

function plainRange(range) {
  if (range === undefined) return undefined;
  return {
    start: { line: range.start.line, character: range.start.character },
    end: { line: range.end.line, character: range.end.character },
  };
}

function diagnosticCode(diagnostic) {
  const code = diagnostic?.code;
  if (typeof code === "string" || typeof code === "number") {
    return String(code);
  }
  if (plainObject(code) && (typeof code.value === "string" || typeof code.value === "number")) {
    return String(code.value);
  }
  return "";
}

function flattenSymbolNames(symbols, output = []) {
  for (const symbol of symbols ?? []) {
    if (typeof symbol?.name === "string") {
      output.push(symbol.name);
    }
    if (Array.isArray(symbol?.children)) {
      flattenSymbolNames(symbol.children, output);
    }
  }
  return output;
}

function expectedCompletionItems(vscode, values, context) {
  if (!Array.isArray(values)) return undefined;
  return values.map((item, index) => {
    requirePlainObject(item, `${context} completion item ${index}`);
    if (typeof item.label !== "string" || item.label.length === 0 || item.label.length > 256) {
      throw new TypeError(`${context} completion item ${index} label is invalid`);
    }
    if (typeof item.kind !== "string" || !Number.isInteger(vscode.CompletionItemKind[item.kind])) {
      throw new TypeError(`${context} completion item ${index} kind is invalid`);
    }
    return { label: item.label, kind: vscode.CompletionItemKind[item.kind] };
  });
}

async function requestCompletion(vscode, document, expectation) {
  const started = performance.now();
  const value = await vscode.commands.executeCommand(
    "vscode.executeCompletionItemProvider",
    document.uri,
    toPosition(vscode, expectation.position, "completion position"),
  );
  const elapsedMs = performance.now() - started;
  const items = normalizeCompletionItems(value);
  const expectedItems = expectedCompletionItems(
    vscode,
    expectation.completionItems,
    "positive",
  );
  for (const expected of expectedItems ?? []) {
    assert.ok(
      items.some(
        (item) => completionLabel(item) === expected.label && item.kind === expected.kind,
      ),
      `completion is missing ${expected.label}/${expected.kind}; count=${items.length}`,
    );
  }
  if (expectation.exactCount !== undefined) {
    assert.equal(items.length, expectation.exactCount, "unexpected completion item count");
  }
  const tuples = items
    .map((item) => [completionLabel(item), item.kind ?? null])
    .sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right), "en-US"));
  return { elapsedMs, count: items.length, itemSetHash: sha256(JSON.stringify(tuples)) };
}

async function requestSignature(vscode, document, expectation) {
  const started = performance.now();
  const value = await vscode.commands.executeCommand(
    "vscode.executeSignatureHelpProvider",
    document.uri,
    toPosition(vscode, expectation.position, "signature position"),
  );
  const elapsedMs = performance.now() - started;
  assert.ok(value && Array.isArray(value.signatures), "signature-help provider returned no result");
  const activeSignature = value.signatures[value.activeSignature ?? 0];
  assert.ok(activeSignature, "signature-help result has no active signature");
  if (expectation.exactCount !== undefined) {
    assert.equal(value.signatures.length, expectation.exactCount, "unexpected signature count");
  }
  if (expectation.label !== undefined) {
    assert.equal(signatureLabel(activeSignature), expectation.label, "unexpected signature label");
  }
  if (expectation.activeParameter !== undefined) {
    assert.equal(value.activeParameter, expectation.activeParameter, "unexpected active parameter");
  }
  return {
    elapsedMs,
    count: value.signatures.length,
    signatureHash: sha256(signatureLabel(activeSignature)),
    activeParameter: value.activeParameter ?? null,
  };
}

async function requestHover(vscode, document, expectation, suiteSpec) {
  const started = performance.now();
  await optionalTimedDelay(suiteSpec, "hover");
  const value = await vscode.commands.executeCommand(
    "vscode.executeHoverProvider",
    document.uri,
    toPosition(vscode, expectation.position, "hover position"),
  );
  const elapsedMs = performance.now() - started;
  const text = hoverText(value);
  assert.ok(text.length > 0, "hover provider returned no result");
  if (expectation.includes !== undefined) {
    assert.ok(text.includes(expectation.includes), "hover result lacks the expected fact text");
  }
  return { elapsedMs, resultHash: sha256(text) };
}

async function requestDefinition(vscode, document, expectation, suiteSpec) {
  const started = performance.now();
  await optionalTimedDelay(suiteSpec, "definition");
  const value = await vscode.commands.executeCommand(
    "vscode.executeDefinitionProvider",
    document.uri,
    toPosition(vscode, expectation.position, "definition position"),
  );
  const elapsedMs = performance.now() - started;
  const locations = definitionLocations(value);
  assert.ok(locations.length > 0, "definition provider returned no location");
  if (expectation.exactCount !== undefined) {
    assert.equal(locations.length, expectation.exactCount, "unexpected definition count");
  }
  if (expectation.targetUri !== undefined) {
    assert.equal(
      definitionUri(locations[0])?.toString(),
      expectation.targetUri,
      "definition did not target the exact expected URI",
    );
  }
  if (expectation.selectionRange !== undefined) {
    assert.deepEqual(
      plainRange(definitionSelectionRange(locations[0])),
      expectation.selectionRange,
      "definition did not target the exact declaration selection range",
    );
  }
  const firstTarget = locations[0];
  const firstTargetUri = definitionUri(firstTarget)?.toString() ?? null;
  const firstSelectionRange = plainRange(definitionSelectionRange(firstTarget)) ?? null;
  return {
    elapsedMs,
    count: locations.length,
    targetHash: sha256(JSON.stringify({
      uri: firstTargetUri,
      selectionRange: firstSelectionRange,
    })),
    targetSchemes: [...new Set(locations.map((location) => definitionUri(location)?.scheme ?? ""))]
      .filter(Boolean)
      .sort(),
  };
}

async function requestDocumentSymbols(vscode, document, expectation) {
  const value = await vscode.commands.executeCommand(
    "vscode.executeDocumentSymbolProvider",
    document.uri,
  );
  assert.ok(Array.isArray(value), "document-symbol provider returned no list");
  const names = flattenSymbolNames(value);
  for (const expected of expectation.includeNames ?? []) {
    assert.ok(names.includes(expected), "document symbols are missing an expected name");
  }
  if (expectation.exactCount !== undefined) {
    assert.equal(names.length, expectation.exactCount, "unexpected document-symbol count");
  }
  return { count: names.length, nameSetHash: sha256([...names].sort().join("\n")) };
}

function diagnosticsMatch(diagnostics, expectation) {
  if (expectation.minCount !== undefined && diagnostics.length < expectation.minCount) {
    return false;
  }
  if (expectation.exactCount !== undefined && diagnostics.length !== expectation.exactCount) {
    return false;
  }
  const codes = diagnostics.map(diagnosticCode);
  return (expectation.includeCodes ?? []).every((code) => codes.includes(String(code)));
}

async function waitForDiagnostics(
  vscode,
  document,
  expectation,
  endpoint,
  suiteSpec,
  started = performance.now(),
) {
  await optionalTimedDelay(suiteSpec, endpoint);
  const diagnostics = await waitFor(
    `${endpoint} publication`,
    () => {
      const current = vscode.languages.getDiagnostics(document.uri);
      return diagnosticsMatch(current, expectation) ? current : undefined;
    },
    { timeoutMs: expectation.timeoutMs ?? DEFAULT_TIMEOUT_MS },
  );
  return {
    elapsedMs: performance.now() - started,
    count: diagnostics.length,
    codeSetHash: sha256(diagnostics.map(diagnosticCode).sort().join("\n")),
  };
}

async function probeProviderAbsence(vscode, document, expectation = {}) {
  const position = expectation.position
    ? toPosition(vscode, expectation.position, "absence probe position")
    : new vscode.Position(0, 0);
  const refused = [];
  const bounded = async (name, promise) => {
    let timer;
    try {
      return await Promise.race([
        promise,
        new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error(`${name} absence probe timed out`)), 4_000);
        }),
      ]);
    } catch (error) {
      if (error?.name === "CodeExpectedError" && /^ENOPRO:/u.test(error.message ?? "")) {
        refused.push(name);
        return undefined;
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  };
  const [completion, signature, hover, definition, symbols, formatting] = await Promise.all([
    bounded(
      "completion",
      vscode.commands.executeCommand("vscode.executeCompletionItemProvider", document.uri, position),
    ),
    bounded(
      "signature",
      vscode.commands.executeCommand("vscode.executeSignatureHelpProvider", document.uri, position),
    ),
    bounded("hover", vscode.commands.executeCommand("vscode.executeHoverProvider", document.uri, position)),
    bounded(
      "definition",
      vscode.commands.executeCommand("vscode.executeDefinitionProvider", document.uri, position),
    ),
    bounded(
      "document symbols",
      vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", document.uri),
    ),
    bounded(
      "formatting",
      vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", document.uri, {
        tabSize: 4,
        insertSpaces: true,
      }),
    ),
  ]);
  const completionItems = normalizeCompletionItems(completion);
  const targetCompletionItems = expectedCompletionItems(
    vscode,
    expectation.completionItems,
    "absence",
  );
  const targetCompletionMatches = targetCompletionItems
    ? completionItems.filter((item) =>
      targetCompletionItems.some(
        (expected) => completionLabel(item) === expected.label && item.kind === expected.kind,
      ))
    : completionItems;
  const responding = {
    completion: targetCompletionMatches.length > 0,
    signature: signature !== undefined && signature !== null,
    hover: Array.isArray(hover) ? hover.length > 0 : hover !== undefined && hover !== null,
    definition: definitionLocations(definition).length > 0,
    documentSymbols: Array.isArray(symbols) && symbols.length > 0,
    formatting: Array.isArray(formatting) && formatting.length > 0,
    diagnostics: vscode.languages.getDiagnostics(document.uri).length > 0,
  };
  return {
    providers: responding,
    refused: refused.sort(),
    completion: {
      observedCount: completionItems.length,
      targetMatchCount: targetCompletionMatches.length,
      kindSetHash: sha256(
        [...new Set(completionItems.map((item) => String(item.kind ?? "none")))].sort().join("\n"),
      ),
    },
  };
}

async function openScratchEditor(vscode, content, position) {
  const document = await vscode.workspace.openTextDocument({ language: "marrow", content });
  const editor = await vscode.window.showTextDocument(document, { preview: false });
  const cursor = toPosition(vscode, position, "scratch cursor");
  editor.selection = new vscode.Selection(cursor, cursor);
  await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
  await waitFor(
    "scratch editor focus",
    () => vscode.window.activeTextEditor?.document === document,
    { timeoutMs: 1_000 },
  );
  await delay(25);
  return { document, editor };
}

async function withScratchEditor(vscode, name, content, position, callback) {
  const opened = await openScratchEditor(vscode, content, position);
  try {
    return await callback(opened.editor);
  } catch (error) {
    throw new Error(`${name}: ${safeError(error).message}`);
  } finally {
    await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
    await waitFor(`${name} scratch close`, () => opened.document.isClosed, { timeoutMs: 1_000 });
  }
}

async function executeType(vscode, editor, text, expectedText, expectedPosition) {
  await vscode.commands.executeCommand("type", { text });
  try {
    await waitFor("typed editor text", () => editor.document.getText() === expectedText, {
      timeoutMs: 1_000,
    });
  } catch (error) {
    const actual = editor.document.getText();
    throw new Error(
      `typed editor mismatch expectedBytes=${Buffer.byteLength(expectedText)} actualBytes=${Buffer.byteLength(actual)} actualHash=${sha256(actual)}: ${safeError(error).message}`,
    );
  }
  const expected = toPosition(vscode, expectedPosition, "expected cursor");
  assert.equal(editor.selection.active.line, expected.line, "typed cursor line mismatch");
  assert.equal(editor.selection.active.character, expected.character, "typed cursor column mismatch");
}

async function runTypingSuite(vscode) {
  const pairs = [
    ["{", "}"],
    ["[", "]"],
    ["(", ")"],
    ['"', '"'],
  ];
  for (const [open, close] of pairs) {
    await withScratchEditor(vscode, `pair ${open}`, "", { line: 0, character: 0 }, async (editor) => {
      await executeType(vscode, editor, open, `${open}${close}`, { line: 0, character: 1 });
      await executeType(vscode, editor, close, `${open}${close}`, { line: 0, character: 2 });
    });
  }

  await withScratchEditor(vscode, "comment quote", "// x", { line: 0, character: 3 }, async (editor) => {
    await executeType(vscode, editor, '"', '// "x', { line: 0, character: 4 });
  });
  await withScratchEditor(
    vscode,
    "string quote",
    'const s = "value"',
    { line: 0, character: 11 },
    async (editor) => {
    const source = 'const s = "value"';
    assert.equal(editor.document.getText(), source);
    await executeType(vscode, editor, '"', 'const s = ""value"', {
      line: 0,
      character: 12,
    });
    },
  );
  await withScratchEditor(vscode, "brace Enter", "{}", { line: 0, character: 1 }, async (editor) => {
    await vscode.commands.executeCommand("type", { text: "\n" });
    await waitFor("Enter between braces", () => editor.document.lineCount === 3, {
      timeoutMs: 1_000,
    });
    const middle = editor.document.lineAt(1).text;
    assert.match(middle, /^\s+$/u, "Enter did not indent the new line");
    assert.equal(editor.document.lineAt(2).text, "}", "Enter did not outdent the closer");
    assert.equal(editor.selection.active.line, 1, "Enter cursor is not on the indented line");
    assert.equal(
      editor.selection.active.character,
      middle.length,
      "Enter cursor is not after the indentation",
    );
  });
  return { pairKinds: pairs.length, quoteSuppressionContexts: 2, enterIndentOutdent: true };
}

async function requestFormatEdits(vscode, document, waitForNonempty = false) {
  const started = performance.now();
  const request = () =>
    vscode.commands.executeCommand(
      "vscode.executeFormatDocumentProvider",
      document.uri,
      { tabSize: 4, insertSpaces: true },
    );
  const response = waitForNonempty
    ? await waitFor(
        "format provider readiness",
        async () => {
          const value = await request();
          return Array.isArray(value) && value.length > 0 ? value : undefined;
        },
        { timeoutMs: 8_000, intervalMs: 25 },
      )
    : await request();
  const edits = response ?? [];
  assert.ok(Array.isArray(edits), "format provider returned a non-list result");
  return { edits, elapsedMs: performance.now() - started };
}

function canonicalText(formatSpec) {
  if (typeof formatSpec.canonicalText === "string") {
    return formatSpec.canonicalText;
  }
  if (formatSpec.canonicalPath !== undefined) {
    return fs.readFileSync(requireAbsolutePath(formatSpec.canonicalPath, "canonical format path"), "utf8");
  }
  throw new TypeError("format spec needs canonicalText or canonicalPath");
}

function explicitFormatOnSaveValues(inspect) {
  const names = [
    "globalValue",
    "workspaceValue",
    "workspaceFolderValue",
    "globalLanguageValue",
    "workspaceLanguageValue",
    "workspaceFolderLanguageValue",
  ];
  return names.filter((name) => inspect?.[name] !== undefined);
}

async function runFormattingSuite(vscode, suiteSpec, formatSpec) {
  requirePlainObject(formatSpec, "format spec");
  const expected = canonicalText(formatSpec);
  const { document } = await openShownDocument(vscode, formatSpec.file, "marrow");
  await forceDirtyText(vscode, document, formatSpec.unformattedText);

  const formatStarted = performance.now();
  const projected = await requestFormatEdits(vscode, document, true);
  assert.ok(projected.edits.length > 0, "unformatted source produced no Format Document edits");
  const commandStarted = performance.now();
  await optionalTimedDelay(suiteSpec, "format");
  if (formatSpec.invoke === "provider") {
    const edit = new vscode.WorkspaceEdit();
    edit.set(document.uri, projected.edits);
    assert.equal(await vscode.workspace.applyEdit(edit), true, "format provider edits were refused");
  } else {
    await vscode.commands.executeCommand("editor.action.formatDocument");
  }
  await waitFor("Format Document command", () => document.getText() === expected, {
    timeoutMs: formatSpec.timeoutMs ?? DEFAULT_TIMEOUT_MS,
  });
  const commandElapsedMs = performance.now() - commandStarted;
  const formatElapsedMs = performance.now() - formatStarted;
  assert.equal(document.getText(), expected, "Format Document differs from canonical marrow fmt");
  const firstHash = sha256(document.getText());

  const secondStarted = performance.now();
  if (formatSpec.invoke === "provider") {
    const second = await requestFormatEdits(vscode, document);
    if (second.edits.length > 0) {
      const edit = new vscode.WorkspaceEdit();
      edit.set(document.uri, second.edits);
      assert.equal(await vscode.workspace.applyEdit(edit), true, "idempotence edits were refused");
    }
  } else {
    await vscode.commands.executeCommand("editor.action.formatDocument");
  }
  await waitFor("idempotent Format Document command", () => document.getText() === expected, {
    timeoutMs: formatSpec.timeoutMs ?? DEFAULT_TIMEOUT_MS,
  });
  const secondElapsedMs = performance.now() - secondStarted;
  assert.equal(document.getText(), expected, "Format Document is not idempotent");

  const scope = { uri: document.uri, languageId: document.languageId };
  const config = vscode.workspace.getConfiguration("editor", scope);
  const inspectedDefault = config.inspect("formatOnSave");
  assert.equal(config.get("formatOnSave", false), false, "format-on-save is not off by default");
  assert.deepEqual(
    explicitFormatOnSaveValues(inspectedDefault),
    [],
    "format-on-save has an unexpected explicit default-profile value",
  );

  await forceDirtyText(vscode, document, formatSpec.unformattedText);
  assert.equal(await document.save(), true, "default save failed");
  assert.equal(document.getText(), formatSpec.unformattedText, "default save formatted unexpectedly");
  assert.equal(
    fs.readFileSync(document.uri.fsPath, "utf8"),
    formatSpec.unformattedText,
    "default save wrote formatted bytes unexpectedly",
  );

  await config.update("formatOnSave", true, vscode.ConfigurationTarget.Workspace, true);
  try {
    const opted = vscode.workspace.getConfiguration("editor", scope);
    assert.equal(opted.get("formatOnSave"), true, "language-scoped format-on-save did not opt in");
    await forceDirtyText(vscode, document, formatSpec.unformattedText);
    assert.equal(await document.save(), true, "opt-in save failed");
    await waitFor("format on save", () => document.getText() === expected, {
      timeoutMs: formatSpec.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    });
    assert.equal(
      fs.readFileSync(document.uri.fsPath, "utf8"),
      expected,
      "opt-in save did not write canonical bytes",
    );
  } finally {
    await config.update("formatOnSave", undefined, vscode.ConfigurationTarget.Workspace, true);
  }

  return {
    formatMs: formatElapsedMs,
    formatCommandMs: commandElapsedMs,
    idempotenceMs: secondElapsedMs,
    formatProviderMs: projected.elapsedMs,
    projectedFormatMs: projected.elapsedMs,
    canonicalHash: firstHash,
    projectedEditCount: projected.edits.length,
    defaultOff: true,
    languageOptIn: true,
  };
}

async function runColdStartSuite(vscode, suiteSpec, coldStartSpec) {
  requirePlainObject(coldStartSpec, "cold-start spec");
  assert.deepEqual(
    Object.keys(coldStartSpec).sort(),
    ["expectedTextHash", "file", "firstDiagnostics", "timeoutMs"],
    "cold-start spec has an unknown or missing field",
  );
  if (!SHA256_PATTERN.test(coldStartSpec.expectedTextHash)) {
    throw new TypeError("cold-start expected text hash is invalid");
  }
  const timeoutMs = boundedInteger(
    coldStartSpec.timeoutMs,
    "cold-start timeout",
    1,
    60_000,
  );
  const expectation = requirePlainObject(
    coldStartSpec.firstDiagnostics,
    "cold-start diagnostics expectation",
  );
  const targetId = suiteSpec.targetExtensionId ?? DEFAULT_TARGET_EXTENSION_ID;
  const target = targetExtension(vscode, targetId);
  assert.ok(target, `target extension ${targetId} is not installed`);
  assert.equal(target.isActive, false, "cold-start target was active before its first file open");
  const uri = uriFromReference(vscode, coldStartSpec.file);
  assert.equal(uri.scheme, "file", "cold-start fixture must be a file URI");
  assert.equal(
    vscode.workspace.textDocuments.some((document) => document.uri.toString() === uri.toString()),
    false,
    "cold-start fixture was already open",
  );
  assert.equal(
    vscode.languages.getDiagnostics(uri).length,
    0,
    "cold-start fixture had diagnostics before its first file open",
  );

  let firstPublication;
  let firstPublicationTimer;
  let firstPublicationObserved = false;
  const started = performance.now();
  const firstDiagnosticsDelayMs = timedDelayMs(suiteSpec, "firstDiagnostics");
  const diagnosticsListener = vscode.languages.onDidChangeDiagnostics((event) => {
    if (
      !firstPublicationObserved &&
      event.uris.some((changed) => changed.toString() === uri.toString())
    ) {
      firstPublicationObserved = true;
      const diagnostics = vscode.languages.getDiagnostics(uri);
      const recordPublication = () => {
        firstPublication = {
          elapsedMs: performance.now() - started,
          diagnostics,
        };
      };
      if (firstDiagnosticsDelayMs === 0) {
        recordPublication();
      } else {
        firstPublicationTimer = setTimeout(recordPublication, firstDiagnosticsDelayMs);
      }
    }
  });
  try {
    const { document } = await openShownDocument(vscode, coldStartSpec.file, "marrow");
    assert.equal(document.uri.toString(), uri.toString(), "cold-start opened a different document");
    assert.equal(document.isDirty, false, "cold-start fixture was not on-disk source");
    assert.equal(
      sha256(Buffer.from(document.getText(), "utf8")),
      coldStartSpec.expectedTextHash,
      "cold-start document bytes differ from the invocation authority",
    );
    assert.equal(
      sha256(fs.readFileSync(document.uri.fsPath)),
      coldStartSpec.expectedTextHash,
      "cold-start on-disk bytes differ from the invocation authority",
    );
    await optionalTimedDelay(suiteSpec, "activation");
    await waitForTargetActivation(vscode, targetId, timeoutMs);
    const activationElapsedMs = performance.now() - started;
    const publication = await waitFor(
      "cold-start first diagnostics publication",
      () => firstPublication,
      { timeoutMs },
    );
    assert.ok(
      diagnosticsMatch(publication.diagnostics, expectation),
      "cold-start first diagnostics publication differs from its expectation",
    );
    assert.ok(
      publication.diagnostics.length > 0,
      "cold-start fixture did not produce a diagnostic on its first publication",
    );
    return {
      targetInactiveBefore: true,
      documentUnopenedBefore: true,
      brokenOnDisk: true,
      documentHash: coldStartSpec.expectedTextHash,
      activation: {
        elapsedMs: activationElapsedMs,
        languageId: document.languageId,
        targetActive: true,
      },
      firstDiagnostics: {
        elapsedMs: publication.elapsedMs,
        count: publication.diagnostics.length,
        codeSetHash: sha256(publication.diagnostics.map(diagnosticCode).sort().join("\n")),
        firstPublication: true,
      },
    };
  } finally {
    if (firstPublicationTimer !== undefined) clearTimeout(firstPublicationTimer);
    diagnosticsListener.dispose();
  }
}

async function runFactSuite(vscode, suiteSpec, factsSpec) {
  requirePlainObject(factsSpec, "facts spec");
  const { document } = await openShownDocument(vscode, factsSpec.file, "marrow");
  assert.equal(document.uri.scheme, "file", "fact suite requires an external temporary file");
  const original = fs.readFileSync(document.uri.fsPath, "utf8");
  try {
    const firstDiagnosticsStarted = factsSpec.firstDiagnostics ? performance.now() : undefined;
    await forceDirtyText(vscode, document, factsSpec.text);
    const result = { dirty: document.isDirty, overlayHash: sha256(document.getText()) };
    if (factsSpec.firstDiagnostics) {
      result.firstDiagnostics = await waitForDiagnostics(
        vscode,
        document,
        factsSpec.firstDiagnostics,
        "firstDiagnostics",
        suiteSpec,
        firstDiagnosticsStarted,
      );
    }
    const queryFacts = async () => {
      if (factsSpec.completion) {
        const completionText = factsSpec.completion.text;
        if (completionText !== undefined) {
          if (typeof completionText !== "string") {
            throw new TypeError("completion recovery text must be a string");
          }
          const restoredText = document.getText();
          await replaceDocument(vscode, document, completionText);
          result.completionRecoveryDiagnostics = await waitForDiagnostics(
            vscode,
            document,
            requirePlainObject(factsSpec.completion.diagnostics, "completion diagnostics"),
            "completionDiagnostics",
            suiteSpec,
          );
          result.completion = await requestCompletion(vscode, document, factsSpec.completion);
          await replaceDocument(vscode, document, restoredText);
          result.completionRestoreDiagnostics = await waitForDiagnostics(
            vscode,
            document,
            requirePlainObject(
              factsSpec.completion.restoreDiagnostics,
              "completion restore diagnostics",
            ),
            "completionRestoreDiagnostics",
            suiteSpec,
          );
        } else {
          result.completion = await requestCompletion(vscode, document, factsSpec.completion);
        }
      }
      if (factsSpec.signature) {
        result.signature = await requestSignature(vscode, document, factsSpec.signature);
      }
      if (factsSpec.hover) {
        result.hover = await requestHover(vscode, document, factsSpec.hover, suiteSpec);
      }
      if (factsSpec.definition) {
        result.definition = await requestDefinition(vscode, document, factsSpec.definition, suiteSpec);
      }
      if (factsSpec.documentSymbols) {
        result.documentSymbols = await requestDocumentSymbols(
          vscode,
          document,
          factsSpec.documentSymbols,
        );
      }
    };
    if (factsSpec.queriesAfterUpdate !== true) {
      await queryFacts();
    }
    if (factsSpec.updatedText !== undefined) {
      const updatedDiagnosticsStarted = performance.now();
      await replaceDocument(vscode, document, factsSpec.updatedText);
      assert.equal(document.isDirty, true, "updated overlay is not dirty");
      result.updatedOverlayHash = sha256(document.getText());
      result.updatedDiagnostics = await waitForDiagnostics(
        vscode,
        document,
        requirePlainObject(factsSpec.updatedDiagnostics, "updated diagnostics expectation"),
        "updatedDiagnostics",
        suiteSpec,
        updatedDiagnosticsStarted,
      );
    }
    if (factsSpec.queriesAfterUpdate === true) {
      await queryFacts();
    }
    return result;
  } finally {
    if (document.getText() !== original || document.isDirty) {
      await replaceDocument(vscode, document, original);
      assert.equal(await document.save(), true, "fact fixture restore save failed");
    }
    assert.equal(document.isDirty, false, "fact fixture remained dirty after restore");
  }
}

async function runActivationSuite(vscode, suiteSpec, activationSpec) {
  requirePlainObject(activationSpec, "activation spec");
  const targetId = activationSpec.targetExtensionId ?? DEFAULT_TARGET_EXTENSION_ID;
  const started = performance.now();
  const { document } = await openShownDocument(vscode, activationSpec.file, "marrow");
  await optionalTimedDelay(suiteSpec, "activation");
  await waitForTargetActivation(vscode, targetId, activationSpec.timeoutMs ?? DEFAULT_TIMEOUT_MS);
  return {
    elapsedMs: performance.now() - started,
    languageId: document.languageId,
    targetActive: true,
  };
}

async function runStableHostSuite(vscode, spec, options = {}) {
  requirePlainObject(spec, "host suite spec");
  const writer = options.writer ?? activeRuntime?.writer;
  const targetExtensionId = spec.targetExtensionId ?? DEFAULT_TARGET_EXTENSION_ID;
  const result = {
    schema: 1,
    targetExtensionId,
    ...(spec.clockSpecSha256 === undefined
      ? {}
      : { clockSpecSha256: spec.clockSpecSha256 }),
    before: stateSnapshot(vscode, targetExtensionId),
  };
  try {
    if (spec.coldStart) {
      assert.equal(
        [spec.activation, spec.typing, spec.format, spec.facts].some((value) => value !== undefined),
        false,
        "cold-start suite cannot mix with ready-suite operations",
      );
      result.coldStart = await runColdStartSuite(vscode, spec, spec.coldStart);
    }
    if (spec.activation) {
      result.activation = await runActivationSuite(vscode, spec, spec.activation);
    }
    if (spec.typing === true) {
      result.typing = await runTypingSuite(vscode);
    }
    if (spec.format) {
      result.format = await runFormattingSuite(vscode, spec, spec.format);
    }
    if (spec.facts) {
      result.facts = await runFactSuite(vscode, spec, spec.facts);
    }
    result.after = stateSnapshot(vscode, targetExtensionId);
    writer?.emit("suite.pass", evidenceSuiteSummary(result));
    return result;
  } catch (error) {
    writer?.emit("suite.fail", { error: safeError(error) });
    throw error;
  }
}

function evidenceSuiteSummary(result) {
  return {
    trusted: result.after?.trusted ?? result.before?.trusted ?? false,
    targetActive: result.after?.targetActive ?? false,
    clockSpecSha256: result.clockSpecSha256,
    activationMs: result.coldStart?.activation?.elapsedMs ?? result.activation?.elapsedMs,
    formatMs: result.format?.formatMs,
    formatProviderMs: result.format?.formatProviderMs,
    formatCommandMs: result.format?.formatCommandMs,
    hoverMs: result.facts?.hover?.elapsedMs,
    definitionMs: result.facts?.definition?.elapsedMs,
    firstDiagnosticsMs:
      result.coldStart?.firstDiagnostics?.elapsedMs ?? result.facts?.firstDiagnostics?.elapsedMs,
    updatedDiagnosticsMs: result.facts?.updatedDiagnostics?.elapsedMs,
    coldStartTargetInactive: result.coldStart?.targetInactiveBefore,
    coldStartDocumentUnopened: result.coldStart?.documentUnopenedBefore,
    coldStartBrokenOnDisk: result.coldStart?.brokenOnDisk,
    coldStartDocumentHash: result.coldStart?.documentHash,
    coldStartFirstPublication: result.coldStart?.firstDiagnostics?.firstPublication,
    canonicalHash: result.format?.canonicalHash,
    formatOnSaveDefaultOff: result.format?.defaultOff,
    formatOnSaveLanguageOptIn: result.format?.languageOptIn,
    overlayHash: result.facts?.overlayHash,
    completionCount: result.facts?.completion?.count,
    completionMs: result.facts?.completion?.elapsedMs,
    completionItemSetHash: result.facts?.completion?.itemSetHash,
    signatureCount: result.facts?.signature?.count,
    signatureMs: result.facts?.signature?.elapsedMs,
    signatureHash: result.facts?.signature?.signatureHash,
    signatureActiveParameter: result.facts?.signature?.activeParameter,
    definitionCount: result.facts?.definition?.count,
    definitionTargetHash: result.facts?.definition?.targetHash,
    documentSymbolCount: result.facts?.documentSymbols?.count,
  };
}

class VirtualFileSystem {
  constructor(vscode) {
    this.vscode = vscode;
    this.files = new Map();
    this.emitter = new vscode.EventEmitter();
    this.onDidChangeFile = this.emitter.event;
  }

  put(uri, bytes) {
    if (uri.scheme !== VIRTUAL_SCHEME) {
      throw new TypeError(`virtual URI must use ${VIRTUAL_SCHEME}`);
    }
    const data = Uint8Array.from(bytes);
    this.files.set(uri.path, { data, mtime: Date.now() });
    this.emitter.fire([{ type: this.vscode.FileChangeType.Changed, uri }]);
  }

  watch() {
    return new this.vscode.Disposable(() => {});
  }

  stat(uri) {
    const file = this.files.get(uri.path);
    if (file) {
      return {
        type: this.vscode.FileType.File,
        ctime: file.mtime,
        mtime: file.mtime,
        size: file.data.byteLength,
      };
    }
    if (this.isDirectory(uri.path)) {
      return { type: this.vscode.FileType.Directory, ctime: 0, mtime: 0, size: 0 };
    }
    throw this.vscode.FileSystemError.FileNotFound(uri);
  }

  readDirectory(uri) {
    if (!this.isDirectory(uri.path)) {
      throw this.vscode.FileSystemError.FileNotFound(uri);
    }
    const prefix = uri.path === "/" ? "/" : `${uri.path.replace(/\/$/u, "")}/`;
    const entries = new Map();
    for (const name of this.files.keys()) {
      if (!name.startsWith(prefix)) {
        continue;
      }
      const rest = name.slice(prefix.length);
      if (!rest) {
        continue;
      }
      const slash = rest.indexOf("/");
      entries.set(
        slash === -1 ? rest : rest.slice(0, slash),
        slash === -1 ? this.vscode.FileType.File : this.vscode.FileType.Directory,
      );
    }
    return [...entries.entries()].sort(([left], [right]) => left.localeCompare(right));
  }

  readFile(uri) {
    const file = this.files.get(uri.path);
    if (!file) {
      throw this.vscode.FileSystemError.FileNotFound(uri);
    }
    return file.data;
  }

  writeFile() {
    throw this.vscode.FileSystemError.NoPermissions("read-only gate filesystem");
  }

  rename() {
    throw this.vscode.FileSystemError.NoPermissions("read-only gate filesystem");
  }

  delete() {
    throw this.vscode.FileSystemError.NoPermissions("read-only gate filesystem");
  }

  createDirectory() {
    throw this.vscode.FileSystemError.NoPermissions("read-only gate filesystem");
  }

  isDirectory(candidate) {
    if (candidate === "/") {
      return true;
    }
    const prefix = `${candidate.replace(/\/$/u, "")}/`;
    return [...this.files.keys()].some((name) => name.startsWith(prefix));
  }

  dispose() {
    this.emitter.dispose();
    this.files.clear();
  }
}

async function prepareScopeInspection(runtime, args) {
  const { vscode } = runtime;
  assert.equal(runtime.scopeInspection, undefined, "a scope inspection is already active");
  const theme = args.theme;
  const expectedKinds = new Map([
    ["Dark 2026", vscode.ColorThemeKind.Dark],
    ["Light 2026", vscode.ColorThemeKind.Light],
    ["Default High Contrast", vscode.ColorThemeKind.HighContrast],
  ]);
  const expectedKind = expectedKinds.get(theme);
  assert.ok(expectedKind !== undefined, "theme is outside the closed inspection inventory");
  const { document, editor } = await openShownDocument(vscode, args.file, "marrow");
  assert.equal(sha256(document.getText()), args.documentHash, "scope document hash differs");
  const position = toPosition(vscode, args.position, "scope position");
  const validated = document.validatePosition(position);
  assert.equal(validated.line, position.line, "scope line is outside the document");
  assert.equal(validated.character, position.character, "scope column is outside the document");
  if (args.lexeme !== undefined) {
    if (typeof args.lexeme !== "string" || args.lexeme.length === 0 || args.lexeme.length > 256) {
      throw new TypeError("scope lexeme is invalid");
    }
    const end = position.translate(0, args.lexeme.length);
    assert.ok(
      sha256(document.getText(new vscode.Range(position, end))) === sha256(args.lexeme),
      "scope coordinate does not select the frozen lexeme hash",
    );
  }
  await vscode.workspace
    .getConfiguration("workbench")
    .update("colorTheme", theme, vscode.ConfigurationTarget.Global);
  await waitFor(
    "theme selection",
    () =>
      vscode.workspace.getConfiguration("workbench").get("colorTheme") === theme &&
      vscode.window.activeColorTheme.kind === expectedKind,
    { timeoutMs: 2_000 },
  );
  editor.selection = new vscode.Selection(position, position);
  editor.revealRange(
    new vscode.Range(position, position),
    vscode.TextEditorRevealType.InCenterIfOutsideViewport,
  );
  await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
  await waitFor(
    "scope editor focus",
    () => vscode.window.activeTextEditor?.document.uri.toString() === document.uri.toString(),
    { timeoutMs: 1_000 },
  );
  const [semanticLegend, semanticTokens] = await Promise.all([
    vscode.commands.executeCommand("vscode.provideDocumentSemanticTokensLegend", document.uri),
    vscode.commands.executeCommand("vscode.provideDocumentSemanticTokens", document.uri),
  ]);
  assert.equal(semanticLegend, undefined, "semantic-token legend unexpectedly exists");
  assert.equal(semanticTokens, undefined, "semantic tokens unexpectedly exist");
  const inspectionId = `scope-${String(++runtime.scopeSequence).padStart(4, "0")}`;
  runtime.scopeInspection = { id: inspectionId, uri: document.uri.toString() };
  return {
    inspectionId,
    theme,
    themeKind: vscode.window.activeColorTheme.kind,
    position: { line: position.line, character: position.character },
    documentHash: sha256(document.getText()),
    lexemeHash: args.lexeme === undefined ? undefined : sha256(args.lexeme),
    semanticLegendAbsent: true,
    semanticTokensAbsent: true,
  };
}

async function finishScopeInspection(runtime, args) {
  const active = runtime.scopeInspection;
  assert.ok(active, "no scope inspection is active");
  assert.equal(args.inspectionId, active.id, "scope inspection ID differs");
  runtime.scopeInspection = undefined;
  return { inspectionId: active.id, closed: true };
}

async function replaceWorkspaceFolders(runtime, args) {
  const folders = args.folders;
  if (!Array.isArray(folders) || folders.length > 8) {
    throw new TypeError("workspace folder list is invalid");
  }
  const additions = folders.map((folder, index) => {
    requirePlainObject(folder, `workspace folder ${index}`);
    if (typeof folder.uri !== "string" || folder.uri.length > 16_384) {
      throw new TypeError("workspace folder URI is invalid");
    }
    if (folder.name !== undefined && (typeof folder.name !== "string" || folder.name.length > 256)) {
      throw new TypeError("workspace folder name is invalid");
    }
    return { uri: runtime.vscode.Uri.parse(folder.uri, true), ...(folder.name ? { name: folder.name } : {}) };
  });
  const current = runtime.vscode.workspace.workspaceFolders ?? [];
  let unchangedPrefix = 0;
  while (
    unchangedPrefix < current.length &&
    unchangedPrefix < additions.length &&
    current[unchangedPrefix].uri.toString() === additions[unchangedPrefix].uri.toString()
  ) {
    unchangedPrefix += 1;
  }
  assert.equal(
    runtime.vscode.workspace.updateWorkspaceFolders(
      unchangedPrefix,
      current.length - unchangedPrefix,
      ...additions.slice(unchangedPrefix),
    ),
    true,
    "workspace-folder update was refused",
  );
  await waitFor(
    "workspace-folder update",
    () => {
      const actual = runtime.vscode.workspace.workspaceFolders ?? [];
      return actual.length === additions.length && actual.every(
        (folder, index) => folder.uri.toString() === additions[index].uri.toString(),
      );
    },
  );
  return stateSnapshot(runtime.vscode, args.targetExtensionId ?? DEFAULT_TARGET_EXTENSION_ID);
}

async function prepareControlledEditor(runtime, args) {
  assert.equal(runtime.preparedEditor, undefined, "a controlled editor is already prepared");
  const { vscode } = runtime;
  const { document, editor } = await openShownDocument(vscode, args.file, "marrow");
  assert.equal(document.uri.scheme, "file", "controlled editor requires an external temporary file");
  assert.equal(document.isDirty, false, "controlled editor file is already dirty");
  const original = document.getText();
  await forceDirtyText(vscode, document, args.text);
  const position = toPosition(vscode, args.position, "controlled editor position");
  const validated = document.validatePosition(position);
  assert.equal(validated.line, position.line, "controlled editor line is outside the document");
  assert.equal(validated.character, position.character, "controlled editor column is outside the document");
  editor.selection = new vscode.Selection(position, position);
  await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
  await waitFor(
    "controlled editor focus",
    () => vscode.window.activeTextEditor?.document === document,
    { timeoutMs: 1_000 },
  );
  runtime.preparedEditor = { document, editor, original };
  return {
    languageId: document.languageId,
    bytes: Buffer.byteLength(document.getText()),
    textHash: sha256(document.getText()),
    dirty: document.isDirty,
    position: { line: position.line, character: position.character },
  };
}

function assertControlledEditor(runtime, args) {
  const prepared = runtime.preparedEditor;
  assert.ok(prepared, "no controlled editor is prepared");
  const text = prepared.document.getText();
  assert.equal(sha256(text), args.textHash, "controlled editor text hash mismatch");
  assert.equal(Buffer.byteLength(text), args.bytes, "controlled editor byte count mismatch");
  if (args.position !== undefined) {
    const expected = toPosition(runtime.vscode, args.position, "controlled editor expected position");
    assert.equal(prepared.editor.selection.active.line, expected.line, "controlled editor cursor line mismatch");
    assert.equal(
      prepared.editor.selection.active.character,
      expected.character,
      "controlled editor cursor column mismatch",
    );
  }
  return {
    bytes: Buffer.byteLength(text),
    textHash: sha256(text),
    dirty: prepared.document.isDirty,
    position: {
      line: prepared.editor.selection.active.line,
      character: prepared.editor.selection.active.character,
    },
  };
}

function positionControlledEditor(runtime, args) {
  const prepared = runtime.preparedEditor;
  assert.ok(prepared, "no controlled editor is prepared");
  const position = toPosition(runtime.vscode, args.position, "controlled editor position");
  const validated = prepared.document.validatePosition(position);
  assert.equal(validated.line, position.line, "controlled editor line is outside the document");
  assert.equal(validated.character, position.character, "controlled editor column is outside the document");
  prepared.editor.selection = new runtime.vscode.Selection(position, position);
  return { line: position.line, character: position.character };
}

async function typeControlledEditor(runtime, args) {
  const prepared = runtime.preparedEditor;
  assert.ok(prepared, "no controlled editor is prepared");
  if (typeof args.text !== "string" || !["{", "}", "[", "]", "(", ")", '"', "\n"].includes(args.text)) {
    throw new TypeError("controlled editor type text is outside the closed interaction inventory");
  }
  await runtime.vscode.commands.executeCommand("type", { text: args.text });
  const text = prepared.document.getText();
  return {
    bytes: Buffer.byteLength(text),
    textHash: sha256(text),
    dirty: prepared.document.isDirty,
    position: {
      line: prepared.editor.selection.active.line,
      character: prepared.editor.selection.active.character,
    },
  };
}

async function waitControlledEditor(runtime, args) {
  try {
    await waitFor(
      "controlled editor state",
      () => {
        try {
          return assertControlledEditor(runtime, args);
        } catch {
          return undefined;
        }
      },
      { timeoutMs: args.timeoutMs ?? DEFAULT_TIMEOUT_MS },
    );
  } catch (error) {
    const prepared = runtime.preparedEditor;
    const text = prepared?.document.getText() ?? "";
    const position = prepared?.editor.selection.active;
    throw new Error(
      `${safeError(error).message}; actual=${JSON.stringify({
        bytes: Buffer.byteLength(text),
        textHash: sha256(text),
        dirty: prepared?.document.isDirty ?? false,
        position: position ? { line: position.line, character: position.character } : null,
      })}`,
    );
  }
  return assertControlledEditor(runtime, args);
}

async function formatControlledEditor(runtime, args) {
  assert.ok(runtime.preparedEditor, "no controlled editor is prepared");
  await runtime.vscode.commands.executeCommand("editor.action.formatDocument");
  return {
    commandCompleted: true,
    ...assertControlledEditor(runtime, args),
  };
}

function assertControlledIndent(runtime) {
  const prepared = runtime.preparedEditor;
  assert.ok(prepared, "no controlled editor is prepared");
  const document = prepared.document;
  assert.equal(document.lineCount, 3, "Enter between braces did not create three lines");
  assert.equal(document.lineAt(0).text, "{", "Enter changed the opener line");
  assert.match(document.lineAt(1).text, /^\s+$/u, "Enter did not indent the middle line");
  assert.equal(document.lineAt(2).text, "}", "Enter did not outdent the closer");
  assert.equal(prepared.editor.selection.active.line, 1, "Enter cursor is not on the middle line");
  assert.equal(
    prepared.editor.selection.active.character,
    document.lineAt(1).text.length,
    "Enter cursor is not after indentation",
  );
  return {
    lineCount: 3,
    indentWidth: document.lineAt(1).text.length,
    textHash: sha256(document.getText()),
  };
}

async function restoreControlledEditor(runtime) {
  const prepared = runtime.preparedEditor;
  assert.ok(prepared, "no controlled editor is prepared");
  try {
    await replaceDocument(runtime.vscode, prepared.document, prepared.original);
    assert.equal(await prepared.document.save(), true, "controlled editor restore save failed");
    assert.equal(prepared.document.isDirty, false, "controlled editor remained dirty");
    return { restoredHash: sha256(prepared.original), dirty: false };
  } finally {
    runtime.preparedEditor = undefined;
  }
}

function disposeTombstone(runtime) {
  runtime.tombstone?.closeSubscription?.dispose();
  runtime.tombstone = undefined;
}

function tombstoneState(runtime) {
  const tombstone = runtime.tombstone;
  assert.ok(tombstone, "no tombstone is prepared");
  const uri = tombstone.uriString;
  const cachedDocument = runtime.vscode.workspace.textDocuments.find(
    (document) => document.uri.toString() === uri,
  );
  const activeUri = runtime.vscode.window.activeTextEditor?.document.uri.toString();
  return {
    closeObserved: tombstone.closeObserved,
    activeUriMatches: activeUri === uri,
    activeUriHash: activeUri ? sha256(activeUri) : null,
    targetUriHash: sha256(uri),
    visible: runtime.vscode.window.visibleTextEditors.some(
      (editor) => editor.document.uri.toString() === uri,
    ),
    retainedByWorkspace: cachedDocument !== undefined,
    cachedDocumentClosed: cachedDocument === undefined,
    cachedDocumentDirty: cachedDocument?.isDirty ?? false,
    diagnosticCount: runtime.vscode.languages.getDiagnostics(tombstone.uri).length,
  };
}

async function prepareTombstone(runtime, args) {
  const { vscode } = runtime;
  assert.equal(runtime.tombstone, undefined, "a tombstone is already prepared");
  const uri = uriFromReference(vscode, args.file);
  await vscode.commands.executeCommand("vscode.open", uri, {
    preview: false,
    preserveFocus: false,
  });
  const editor = await waitFor(
    "tombstone editor open",
    () => {
      const active = vscode.window.activeTextEditor;
      return active?.document.uri.toString() === uri.toString() ? active : undefined;
    },
    { timeoutMs: args.timeoutMs ?? DEFAULT_TIMEOUT_MS },
  );
  const { document } = editor;
  assert.equal(document.uri.scheme, "file", "tombstone requires an external temporary file");
  assert.equal(document.languageId, "marrow", "unexpected tombstone language ID");
  assert.equal(document.isDirty, false, "tombstone file must begin clean");
  const before = await waitForDiagnostics(
    vscode,
    document,
    { minCount: 1, timeoutMs: args.timeoutMs ?? DEFAULT_TIMEOUT_MS },
    "tombstoneDiagnostics",
    {},
  );
  await vscode.commands.executeCommand("workbench.action.focusActiveEditorGroup");
  await waitFor(
    "tombstone editor focus",
    () => vscode.window.activeTextEditor?.document === document,
    { timeoutMs: args.timeoutMs ?? DEFAULT_TIMEOUT_MS },
  );
  assert.equal(
    vscode.window.visibleTextEditors.every((editor) => editor.document.isDirty === false),
    true,
    "tombstone close requires every transient editor to be clean",
  );
  const tombstone = {
    uri: document.uri,
    uriString: document.uri.toString(),
    initialDiagnosticCount: before.count,
    closeObserved: false,
    closeSubscription: undefined,
    started: performance.now(),
  };
  tombstone.closeSubscription = vscode.workspace.onDidCloseTextDocument((candidate) => {
    if (candidate.uri.toString() === tombstone.uriString) tombstone.closeObserved = true;
  });
  runtime.tombstone = tombstone;
  try {
    await vscode.workspace.fs.delete(document.uri, { recursive: false, useTrash: false });
    let absent = false;
    try {
      await vscode.workspace.fs.stat(document.uri);
    } catch (error) {
      absent = error instanceof vscode.FileSystemError && error.code === "FileNotFound";
    }
    assert.equal(absent, true, "tombstoned file still exists");
    return {
      initialDiagnosticCount: before.count,
      fileAbsent: true,
      ...tombstoneState(runtime),
    };
  } catch (error) {
    disposeTombstone(runtime);
    throw error;
  }
}

async function finishTombstone(runtime, args) {
  const { vscode } = runtime;
  const tombstone = runtime.tombstone;
  assert.ok(tombstone, "no tombstone is prepared");
  try {
    await waitFor(
      "tombstone didClose",
      () => {
        const state = tombstoneState(runtime);
        return state.closeObserved &&
          !state.visible &&
          !state.retainedByWorkspace &&
          state.cachedDocumentClosed;
      },
      {
        timeoutMs: args.timeoutMs ?? DEFAULT_TIMEOUT_MS,
      },
    );
    await waitFor(
      "tombstone diagnostics retirement",
      () => vscode.languages.getDiagnostics(tombstone.uri).length === 0,
      { timeoutMs: args.timeoutMs ?? DEFAULT_TIMEOUT_MS },
    );
    return {
      initialDiagnosticCount: tombstone.initialDiagnosticCount,
      retiredDiagnosticCount: 0,
      visibleEditorRetired: true,
      didCloseObserved: true,
      cachedDocumentClosed: true,
      retirementMs: performance.now() - tombstone.started,
    };
  } catch (error) {
    throw new Error(`${safeError(error).message}; actual=${JSON.stringify(tombstoneState(runtime))}`);
  } finally {
    disposeTombstone(runtime);
  }
}

async function executeControl(runtime, record) {
  switch (record.op) {
    case "state":
      return stateSnapshot(
        runtime.vscode,
        record.args.targetExtensionId ?? DEFAULT_TARGET_EXTENSION_ID,
      );
    case "suite.run":
      return evidenceSuiteSummary(
        await runStableHostSuite(runtime.vscode, record.args.spec, { writer: runtime.writer }),
      );
    case "clock.sample":
      return evidenceSuiteSummary(
        await runStableHostSuite(runtime.vscode, clockSampleSpec(record.args), {
          writer: runtime.writer,
        }),
      );
    case "providers.absent": {
      runtime.writer?.emit("providers.absent.phase", { phase: "open.start" });
      const { document } = await openShownDocument(
        runtime.vscode,
        record.args.file,
        record.args.expectedLanguageId ?? "marrow",
      );
      runtime.writer?.emit("providers.absent.phase", { phase: "open.complete" });
      const targetId = record.args.targetExtensionId ?? DEFAULT_TARGET_EXTENSION_ID;
      if (record.args.activateTarget === true) {
        const target = targetExtension(runtime.vscode, targetId);
        assert.ok(target, "target extension is absent");
        runtime.writer?.emit("providers.absent.phase", { phase: "target.activate.start" });
        await target.activate();
        runtime.writer?.emit("providers.absent.phase", { phase: "target.activate.complete" });
      }
      assert.equal(
        targetExtension(runtime.vscode, targetId)?.isActive ?? false,
        record.args.expectedTargetActive ?? false,
        "target activation state differs from the refusal contract",
      );
      runtime.writer?.emit("providers.absent.phase", { phase: "queries.start" });
      const absence = await probeProviderAbsence(runtime.vscode, document, record.args);
      runtime.writer?.emit("providers.absent.phase", { phase: "queries.complete" });
      return {
        languageId: document.languageId,
        ...absence,
      };
    }
    case "scope.prepare":
      return prepareScopeInspection(runtime, record.args);
    case "scope.finish":
      return finishScopeInspection(runtime, record.args);
    case "editor.prepare":
      return prepareControlledEditor(runtime, record.args);
    case "editor.assert":
      return assertControlledEditor(runtime, record.args);
    case "editor.position":
      return positionControlledEditor(runtime, record.args);
    case "editor.type":
      return typeControlledEditor(runtime, record.args);
    case "editor.wait":
      return waitControlledEditor(runtime, record.args);
    case "editor.format":
      return formatControlledEditor(runtime, record.args);
    case "editor.assertIndent":
      return assertControlledIndent(runtime);
    case "editor.restore":
      return restoreControlledEditor(runtime);
    case "tombstone.prepare":
      return prepareTombstone(runtime, record.args);
    case "tombstone.finish":
      return finishTombstone(runtime, record.args);
    case "workspace.replace":
      return replaceWorkspaceFolders(runtime, record.args);
    case "virtual.put": {
      if (typeof record.args.uri !== "string") {
        throw new TypeError("virtual URI is required");
      }
      const uri = runtime.vscode.Uri.parse(record.args.uri, true);
      let bytes;
      if (typeof record.args.text === "string") {
        bytes = Buffer.from(record.args.text, "utf8");
      } else if (typeof record.args.base64 === "string") {
        bytes = Buffer.from(record.args.base64, "base64");
      } else {
        throw new TypeError("virtual.put needs text or base64 bytes");
      }
      if (bytes.byteLength > INPUT_JSON_LIMIT_BYTES) {
        throw new RangeError("virtual fixture exceeds its byte limit");
      }
      runtime.virtualFs.put(uri, bytes);
      return { scheme: uri.scheme, bytes: bytes.byteLength, hash: sha256(bytes) };
    }
    case "virtual.open": {
      const { document } = await openShownDocument(
        runtime.vscode,
        { uri: record.args.uri },
        record.args.languageId ?? "marrow",
      );
      return { scheme: document.uri.scheme, languageId: document.languageId, hash: sha256(document.getText()) };
    }
    case "server.restart":
      await runtime.vscode.commands.executeCommand("marrow.restartServer");
      return { commandCompleted: true };
    case "window.reload":
      await runtime.vscode.commands.executeCommand("workbench.action.reloadWindow");
      return { commandIssued: true };
    case "window.quit":
      await runtime.vscode.commands.executeCommand("workbench.action.quit");
      return { commandIssued: true };
    default:
      throw new Error(`unsupported control operation ${record.op}`);
  }
}

function controlResultSummary(op, result) {
  switch (op) {
    case "state":
    case "workspace.replace":
      return result;
    case "suite.run":
    case "clock.sample":
      return result;
    case "providers.absent":
      return result;
    case "scope.prepare":
    case "scope.finish":
    case "editor.prepare":
    case "editor.assert":
    case "editor.position":
    case "editor.type":
    case "editor.wait":
    case "editor.format":
    case "editor.assertIndent":
    case "editor.restore":
    case "tombstone.prepare":
    case "tombstone.finish":
      return result;
    case "virtual.put":
    case "virtual.open":
    case "server.restart":
    case "window.reload":
    case "window.quit":
      return result;
    default:
      return { completed: true };
  }
}

function readControlSnapshot(controlPath, expectedIdentity) {
  for (let attempt = 0; attempt < 4; attempt += 1) {
    const pathInfo = fs.lstatSync(controlPath);
    if (
      !pathInfo.isFile() ||
      pathInfo.isSymbolicLink() ||
      (pathInfo.mode & 0o777) !== 0o600
    ) {
      throw new RangeError("control path is not a mode-0600 regular file");
    }
    let descriptor;
    try {
      descriptor = fs.openSync(
        controlPath,
        fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0),
      );
      const opened = fs.fstatSync(descriptor);
      if (
        !opened.isFile() ||
        opened.dev !== pathInfo.dev ||
        opened.ino !== pathInfo.ino ||
        (opened.mode & 0o777) !== 0o600 ||
        opened.size > CONTROL_LIMIT_BYTES ||
        (expectedIdentity !== undefined &&
          (opened.dev !== expectedIdentity.device || opened.ino !== expectedIdentity.inode))
      ) {
        throw new RangeError("control path identity or byte bound changed");
      }
      const body = Buffer.alloc(opened.size);
      let returned = 0;
      while (returned < body.length) {
        const count = fs.readSync(
          descriptor,
          body,
          returned,
          body.length - returned,
          returned,
        );
        if (count === 0) throw new RangeError("control read returned fewer bytes than its size");
        returned += count;
      }
      const afterRead = fs.fstatSync(descriptor);
      const afterPath = fs.lstatSync(controlPath);
      if (
        afterPath.dev !== opened.dev ||
        afterPath.ino !== opened.ino ||
        afterRead.dev !== opened.dev ||
        afterRead.ino !== opened.ino ||
        (afterPath.mode & 0o777) !== 0o600 ||
        (afterRead.mode & 0o777) !== 0o600
      ) {
        throw new RangeError("control path changed during read");
      }
      if (returned !== opened.size) {
        throw new RangeError("control read length differs from its size");
      }
      if (afterRead.size !== opened.size) {
        continue;
      }
      return {
        bytes: body,
        device: opened.dev,
        inode: opened.ino,
      };
    } finally {
      if (descriptor !== undefined) fs.closeSync(descriptor);
    }
  }
  throw new Error("control path did not stabilize for one bounded read");
}

function completeControlRecords(bytes, requireComplete) {
  if (bytes.length === 0) return { bytes: 0, lines: [] };
  const finalNewline = bytes.lastIndexOf(0x0a);
  if (finalNewline === -1) {
    if (requireComplete) throw new RangeError("control file contains a partial record");
    return { bytes: 0, lines: [] };
  }
  if (requireComplete && finalNewline !== bytes.length - 1) {
    throw new RangeError("control file contains a partial record");
  }
  const completeBytes = finalNewline + 1;
  const complete = bytes.subarray(0, completeBytes);
  const text = complete.toString("utf8");
  if (!Buffer.from(text, "utf8").equals(complete)) {
    throw new RangeError("control file contains invalid UTF-8");
  }
  const lines = text.split("\n");
  lines.pop();
  for (const line of lines) {
    if (line.length === 0 || Buffer.byteLength(line) + 1 > CONTROL_LINE_LIMIT_BYTES) {
      throw new RangeError("control file contains an empty or oversized record");
    }
  }
  return { bytes: completeBytes, lines };
}

function startControlLoop(runtime, controlPath, context) {
  requireAbsolutePath(controlPath, "control path");
  const initial = readControlSnapshot(controlPath);
  const existing = completeControlRecords(initial.bytes, true);
  if (existing.lines.length > CONTROL_RECORD_LIMIT) {
    throw new RangeError("control record count exceeded");
  }
  for (const line of existing.lines) parseControlLine(line);
  const state = {
    offset: initial.bytes.length,
    acceptedPrefix: Buffer.from(initial.bytes),
    records: existing.lines.length,
    device: initial.device,
    inode: initial.inode,
    busy: false,
    stopped: false,
  };
  const poll = async () => {
    if (state.busy || state.stopped) {
      return;
    }
    state.busy = true;
    try {
      const snapshot = readControlSnapshot(controlPath, {
        device: state.device,
        inode: state.inode,
      });
      if (snapshot.bytes.length < state.offset) {
        throw new RangeError("control file shrank below its consumed offset");
      }
      if (!snapshot.bytes.subarray(0, state.acceptedPrefix.length).equals(state.acceptedPrefix)) {
        throw new RangeError("control file did not retain its consumed prefix");
      }
      if (snapshot.bytes.length === state.offset) {
        return;
      }
      const pending = completeControlRecords(snapshot.bytes.subarray(state.offset), false);
      if (pending.lines.length === 0) {
        return;
      }
      for (const line of pending.lines) {
        if (state.records >= CONTROL_RECORD_LIMIT) {
          throw new RangeError("control record count exceeded");
        }
        state.records += 1;
        state.offset += Buffer.byteLength(line) + 1;
        const requestSha256 = sha256(Buffer.from(`${line}\n`, "utf8"));
        let record;
        try {
          record = parseControlLine(line);
          runtime.writer?.emit("control.accepted", {
            id: record.id,
            op: record.op,
            requestSha256,
          });
          const result = await executeControl(runtime, record);
          runtime.writer?.emit("control.pass", {
            id: record.id,
            op: record.op,
            requestSha256,
            result: controlResultSummary(record.op, result),
          });
        } catch (error) {
          runtime.writer?.emit("control.fail", {
            id: record?.id ?? "unparsed",
            op: record?.op ?? "unparsed",
            requestSha256,
            error: safeError(error),
          });
        }
      }
      state.acceptedPrefix = Buffer.from(snapshot.bytes.subarray(0, state.offset));
    } catch (error) {
      runtime.writer?.emit("control.fault", { error: safeError(error) });
      state.stopped = true;
    } finally {
      state.busy = false;
    }
  };
  const timer = setInterval(() => void poll(), 25);
  const disposable = {
    dispose() {
      state.stopped = true;
      clearInterval(timer);
    },
  };
  context.subscriptions.push(disposable);
  void poll();
  return disposable;
}

async function activate(context) {
  const vscode = require("vscode");
  const evidencePath = process.env.MARROW_VSQ_EVIDENCE_PATH;
  const writer = evidencePath ? createEvidenceWriter(evidencePath) : undefined;
  const virtualFs = new VirtualFileSystem(vscode);
  context.subscriptions.push(virtualFs);
  context.subscriptions.push(
    vscode.workspace.registerFileSystemProvider(VIRTUAL_SCHEME, virtualFs, {
      isCaseSensitive: true,
      isReadonly: true,
    }),
  );

  const runtime = {
    vscode,
    writer,
    virtualFs,
    control: undefined,
    preparedEditor: undefined,
    tombstone: undefined,
    scopeInspection: undefined,
    scopeSequence: 0,
  };
  activeRuntime = runtime;
  context.subscriptions.push(
    vscode.workspace.onDidGrantWorkspaceTrust(() => {
      writer?.emit("workspace.trusted", stateSnapshot(vscode));
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      writer?.emit("workspace.changed", stateSnapshot(vscode));
    }),
  );
  const controlPath = process.env.MARROW_VSQ_CONTROL_PATH;
  if (controlPath) {
    runtime.control = startControlLoop(runtime, controlPath, context);
  }
  writer?.emit("driver.ready", {
    driverExtensionId: DRIVER_EXTENSION_ID,
    driverExtensionPathHash: sha256(context.extensionPath),
    driverExtensionRealPathHash: sha256(fs.realpathSync(context.extensionPath)),
    processId: process.pid,
    ...stateSnapshot(vscode),
  });
  return Object.freeze({
    state: (targetId) => stateSnapshot(vscode, targetId),
    runStableHostSuite: (spec) => runStableHostSuite(vscode, spec, { writer }),
    prepareScopeInspection: (args) => prepareScopeInspection(runtime, args),
  });
}

function deactivate() {
  activeRuntime?.control?.dispose();
  if (activeRuntime) disposeTombstone(activeRuntime);
  activeRuntime = undefined;
}

module.exports = {
  activate,
  deactivate,
  createEvidenceWriter,
  normalizeCompletionItems,
  parseControlLine,
  readBoundedJson,
  runStableHostSuite,
  stateSnapshot,
  _test: {
    completionLabel,
    diagnosticCode,
    evidenceSuiteSummary,
    flattenSymbolNames,
    formatControlledEditor,
    runColdStartSuite,
    safeError,
    sha256,
    clockSampleSpec,
  },
};
