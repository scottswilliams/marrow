"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const Module = require("node:module");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");

const driver = require("./extension.cjs");

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function jsonLines(file) {
  const body = fs.readFileSync(file, "utf8");
  assert.ok(body.length === 0 || body.endsWith("\n"));
  return body.length === 0 ? [] : body.split("\n").slice(0, -1).map(JSON.parse);
}

function waitUntil(name, predicate, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const poll = () => {
      if (predicate()) {
        resolve();
      } else if (Date.now() >= deadline) {
        reject(new Error(`${name} timed out`));
      } else {
        setTimeout(poll, 10);
      }
    };
    poll();
  });
}

function runNodeChild(source) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["-e", source], {
      stdio: ["ignore", "ignore", "pipe"],
    });
    const stderr = [];
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (code === 0 && signal === null) {
        resolve();
      } else {
        reject(new Error(`child failed ${code ?? signal}: ${Buffer.concat(stderr).toString("utf8")}`));
      }
    });
  });
}

function controlLine(id, exactBytes) {
  const record = { id, op: "window.quit", args: { padding: "" } };
  const empty = Buffer.from(`${JSON.stringify(record)}\n`);
  if (exactBytes === undefined) return empty;
  assert.ok(exactBytes >= empty.length);
  record.args.padding = "x".repeat(exactBytes - empty.length);
  const line = Buffer.from(`${JSON.stringify(record)}\n`);
  assert.equal(line.length, exactBytes);
  return line;
}

function evidencePaddingForExactLine(sequence, event, exactBytes) {
  const record = { schema: 1, sequence, event, data: { padding: "" } };
  const empty = Buffer.from(`${JSON.stringify(record)}\n`);
  assert.ok(exactBytes >= empty.length);
  return "x".repeat(exactBytes - empty.length);
}

function controlVscode(executed) {
  class TestEventEmitter {
    constructor() {
      this.event = () => ({ dispose() {} });
    }

    fire() {}

    dispose() {}
  }
  class TestDisposable {
    constructor(dispose) {
      this.dispose = dispose;
    }
  }
  const disposable = () => ({ dispose() {} });
  return {
    EventEmitter: TestEventEmitter,
    Disposable: TestDisposable,
    FileChangeType: { Changed: 1 },
    FileType: { File: 1, Directory: 2 },
    workspace: {
      isTrusted: true,
      workspaceFolders: [],
      textDocuments: [],
      registerFileSystemProvider: disposable,
      onDidGrantWorkspaceTrust: disposable,
      onDidChangeWorkspaceFolders: disposable,
    },
    extensions: {
      getExtension: () => undefined,
    },
    commands: {
      executeCommand: async (command) => {
        executed.push(command);
      },
    },
  };
}

function controlFault(evidencePath, message) {
  return jsonLines(evidencePath).some((record) =>
    record.event === "control.fault" && record.data?.error?.message.includes(message));
}

async function withControlDriver(controlPath, operation) {
  const executed = [];
  const evidencePath = `${controlPath}.evidence`;
  const vscode = controlVscode(executed);
  const context = { extensionPath: __dirname, subscriptions: [] };
  const originalLoad = Module._load;
  const isolatedEnvironment = [
    "MARROW_VSQ_CONTROL_PATH",
    "MARROW_VSQ_EVIDENCE_PATH",
  ];
  const previousEnvironment = Object.fromEntries(
    isolatedEnvironment.map((name) => [name, process.env[name]]),
  );
  Module._load = function load(request, ...args) {
    if (request === "vscode") return vscode;
    return originalLoad.call(this, request, ...args);
  };
  for (const name of isolatedEnvironment) delete process.env[name];
  process.env.MARROW_VSQ_CONTROL_PATH = controlPath;
  process.env.MARROW_VSQ_EVIDENCE_PATH = evidencePath;
  try {
    await driver.activate(context);
    await operation(executed, evidencePath);
  } finally {
    driver.deactivate();
    for (const subscription of context.subscriptions) subscription.dispose?.();
    Module._load = originalLoad;
    for (const [name, value] of Object.entries(previousEnvironment)) {
      if (value === undefined) {
        delete process.env[name];
      } else {
        process.env[name] = value;
      }
    }
  }
}

function coldVscode(file, extensionRoot, diagnostics = [{ code: "cold.test" }]) {
  const uri = {
    scheme: "file",
    fsPath: file,
    toString: () => `file://${file}`,
  };
  const target = { isActive: false, extensionPath: extensionRoot };
  const listeners = new Set();
  const documents = [];
  let currentDiagnostics = [];
  let openCount = 0;
  const document = {
    uri,
    languageId: "marrow",
    isDirty: false,
    getText: () => fs.readFileSync(file, "utf8"),
  };
  const vscode = {
    Uri: {
      file: (candidate) => {
        assert.equal(candidate, file);
        return uri;
      },
    },
    extensions: {
      getExtension: (id) => id === "marrow-project.marrow" ? target : undefined,
    },
    workspace: {
      isTrusted: true,
      workspaceFolders: [{ uri: { scheme: "file" } }],
      textDocuments: documents,
      openTextDocument: async (opened) => {
        assert.equal(opened.toString(), uri.toString());
        openCount += 1;
        documents.push(document);
        target.isActive = true;
        setImmediate(() => {
          currentDiagnostics = diagnostics;
          for (const listener of listeners) listener({ uris: [uri] });
        });
        return document;
      },
    },
    window: {
      showTextDocument: async () => ({ document }),
    },
    languages: {
      getDiagnostics: (requested) => {
        assert.equal(requested.toString(), uri.toString());
        return currentDiagnostics;
      },
      onDidChangeDiagnostics: (listener) => {
        listeners.add(listener);
        return { dispose: () => listeners.delete(listener) };
      },
    },
  };
  return {
    vscode,
    target,
    get openCount() {
      return openCount;
    },
  };
}

function factVscode(file, extensionRoot, providerResults) {
  class TestPosition {
    constructor(line, character) {
      this.line = line;
      this.character = character;
    }
  }
  class TestRange {
    constructor(start, end) {
      this.start = start;
      this.end = end;
    }
  }
  class TestWorkspaceEdit {
    constructor() {
      this.operations = [];
    }

    insert(uri, position, text) {
      this.operations.push({ kind: "insert", uri, position, text });
    }

    replace(uri, range, text) {
      this.operations.push({ kind: "replace", uri, range, text });
    }
  }
  const uri = {
    scheme: "file",
    fsPath: file,
    toString: () => `file://${file}`,
  };
  const offsetAt = (text, position) => {
    const lines = text.split("\n");
    assert.ok(position.line >= 0 && position.line < lines.length);
    assert.ok(position.character >= 0 && position.character <= lines[position.line].length);
    return lines.slice(0, position.line).reduce((bytes, line) => bytes + line.length + 1, 0) +
      position.character;
  };
  const document = {
    uri,
    languageId: "marrow",
    isDirty: false,
    text: fs.readFileSync(file, "utf8"),
    getText() {
      return this.text;
    },
    positionAt(offset) {
      assert.ok(Number.isInteger(offset) && offset >= 0 && offset <= this.text.length);
      const prefix = this.text.slice(0, offset).split("\n");
      return new TestPosition(prefix.length - 1, prefix.at(-1).length);
    },
    async save() {
      fs.writeFileSync(file, this.text);
      this.isDirty = false;
      return true;
    },
  };
  const target = { isActive: true, extensionPath: extensionRoot };
  const documents = [];
  const vscode = {
    Position: TestPosition,
    Range: TestRange,
    WorkspaceEdit: TestWorkspaceEdit,
    CompletionItemKind: { Function: 3, EnumMember: 20 },
    Uri: {
      file: (candidate) => {
        assert.equal(candidate, file);
        return uri;
      },
      parse: (candidate) => ({
        scheme: new URL(candidate).protocol.slice(0, -1),
        toString: () => candidate,
      }),
    },
    extensions: {
      getExtension: (id) => id === "marrow-project.marrow" ? target : undefined,
    },
    workspace: {
      isTrusted: true,
      workspaceFolders: [{ uri: { scheme: "file" } }],
      textDocuments: documents,
      openTextDocument: async (opened) => {
        assert.equal(opened.toString(), uri.toString());
        if (!documents.includes(document)) documents.push(document);
        return document;
      },
      applyEdit: async (edit) => {
        for (const operation of edit.operations) {
          assert.equal(operation.uri.toString(), uri.toString());
          if (operation.kind === "replace") {
            document.text = operation.text;
          } else {
            const offset = offsetAt(document.text, operation.position);
            document.text = `${document.text.slice(0, offset)}${operation.text}${document.text.slice(offset)}`;
          }
          document.isDirty = true;
        }
        return true;
      },
    },
    window: {
      showTextDocument: async () => ({ document }),
    },
    languages: {
      getDiagnostics: () => [],
    },
    commands: {
      executeCommand: async (command) => {
        if (command === "vscode.executeCompletionItemProvider") return providerResults.completion;
        if (command === "vscode.executeSignatureHelpProvider") return providerResults.signature;
        if (command === "vscode.executeDefinitionProvider") return providerResults.definition;
        throw new Error(`unexpected fact command ${command}`);
      },
    },
  };
  return { vscode, document };
}

async function runSelfTests() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "marrow-vsq-driver-red-"));
  try {
    const manifest = JSON.parse(fs.readFileSync(path.join(__dirname, "package.json"), "utf8"));
    assert.equal(manifest.publisher + "." + manifest.name, "marrow-project.marrow-vsq-host-driver");
    assert.equal("dependencies" in manifest, false);
    assert.equal("devDependencies" in manifest, false);
    assert.deepEqual(manifest.files, ["extension.cjs", "tests.cjs"]);

    assert.equal(typeof driver.createEvidenceWriter, "function");
    const byteEvidence = path.join(root, "evidence-byte.jsonl");
    const byteEvidenceWriter = driver.createEvidenceWriter(byteEvidence, { maxBytes: 512 });
    const byteRecordBase = Buffer.byteLength(JSON.stringify({
      schema: 1,
      sequence: 0,
      event: "capacity.fill",
      data: { value: "" },
    })) + 1;
    byteEvidenceWriter.emit("capacity.fill", { value: "x".repeat(512 - byteRecordBase) });
    assert.equal(byteEvidenceWriter.bytes, 512);
    const admittedByteEvidence = fs.readFileSync(byteEvidence);
    assert.throws(() => byteEvidenceWriter.emit("capacity.flip", {}), /capacity exceeded/u);
    assert.deepEqual(fs.readFileSync(byteEvidence), admittedByteEvidence);
    assert.throws(
      () => driver.createEvidenceWriter(byteEvidence, { maxBytes: 512 }).emit("capacity.flip", {}),
      /capacity exceeded/u,
    );
    assert.deepEqual(fs.readFileSync(byteEvidence), admittedByteEvidence);

    const recordEvidence = path.join(root, "evidence-record.jsonl");
    const recordWriter = driver.createEvidenceWriter(recordEvidence);
    for (let sequence = 0; sequence < 256; sequence += 1) {
      recordWriter.emit("record.fill", { sequence });
    }
    const admittedRecordEvidence = fs.readFileSync(recordEvidence);
    const admittedRecords = jsonLines(recordEvidence);
    assert.deepEqual(admittedRecords.map(({ sequence }) => sequence),
      Array.from({ length: 256 }, (_, sequence) => sequence));
    assert.throws(() => recordWriter.emit("record.flip", {}), /capacity exceeded/u);
    assert.deepEqual(fs.readFileSync(recordEvidence), admittedRecordEvidence);

    const concurrentEvidence = path.join(root, "evidence-concurrent.jsonl");
    const concurrentObserver = driver.createEvidenceWriter(concurrentEvidence);
    const childSource = (owner) => `
const driver = require(${JSON.stringify(path.join(__dirname, "extension.cjs"))});
const writer = driver.createEvidenceWriter(${JSON.stringify(concurrentEvidence)});
for (let index = 0; index < 32; index += 1) writer.emit("concurrent.write", { owner: ${JSON.stringify(owner)}, index });
`;
    await Promise.all([runNodeChild(childSource("left")), runNodeChild(childSource("right"))]);
    const concurrentRecords = jsonLines(concurrentEvidence);
    assert.equal(concurrentRecords.length, 64);
    assert.equal(concurrentObserver.bytes, fs.statSync(concurrentEvidence).size);
    assert.deepEqual(
      concurrentRecords.map(({ sequence }) => sequence),
      Array.from({ length: 64 }, (_, sequence) => sequence),
    );
    assert.deepEqual(
      [...new Set(concurrentRecords.map(({ data }) => data.owner))].sort(),
      ["left", "right"],
    );
    driver.createEvidenceWriter(concurrentEvidence).emit("reload.write", {});
    assert.equal(jsonLines(concurrentEvidence).at(-1).sequence, 64);

    const concurrentBoundEvidence = path.join(root, "evidence-concurrent-bound.jsonl");
    const boundedChildSource = (owner) => `
const driver = require(${JSON.stringify(path.join(__dirname, "extension.cjs"))});
const writer = driver.createEvidenceWriter(${JSON.stringify(concurrentBoundEvidence)}, { maxBytes: 1024 });
(async () => {
  for (let index = 0; index < 32; index += 1) {
    try {
      writer.emit("bounded.write", { owner: ${JSON.stringify(owner)}, index });
    } catch (error) {
      if (!(error instanceof RangeError) || !error.message.includes("capacity exceeded")) throw error;
      break;
    }
    await new Promise(resolve => setTimeout(resolve, 1));
  }
})().catch(error => { process.stderr.write(String(error.stack || error)); process.exitCode = 1; });
`;
    await Promise.all([
      runNodeChild(boundedChildSource("left")),
      runNodeChild(boundedChildSource("right")),
    ]);
    const concurrentBoundWriter = driver.createEvidenceWriter(concurrentBoundEvidence, {
      maxBytes: 1024,
    });
    while (true) {
      try {
        concurrentBoundWriter.emit("bounded.fill", {});
      } catch (error) {
        assert.match(error.message, /capacity exceeded/u);
        break;
      }
    }
    const concurrentBoundBytes = fs.readFileSync(concurrentBoundEvidence);
    const concurrentBoundRecords = jsonLines(concurrentBoundEvidence);
    assert.ok(concurrentBoundBytes.length <= 1024);
    assert.deepEqual(
      concurrentBoundRecords.map(({ sequence }) => sequence),
      Array.from({ length: concurrentBoundRecords.length }, (_, sequence) => sequence),
    );
    assert.throws(
      () => driver.createEvidenceWriter(concurrentBoundEvidence, { maxBytes: 1024 })
        .emit("bounded.flip", {}),
      /capacity exceeded/u,
    );
    assert.deepEqual(fs.readFileSync(concurrentBoundEvidence), concurrentBoundBytes);

    const identityEvidence = path.join(root, "evidence-identity.jsonl");
    const identityWriter = driver.createEvidenceWriter(identityEvidence);
    identityWriter.emit("identity.before", {});
    const identityBytes = fs.readFileSync(identityEvidence);
    const movedEvidence = `${identityEvidence}.moved`;
    fs.renameSync(identityEvidence, movedEvidence);
    fs.symlinkSync(movedEvidence, identityEvidence);
    assert.throws(() => identityWriter.emit("identity.after", {}), /regular file/u);
    assert.deepEqual(fs.readFileSync(movedEvidence), identityBytes);

    assert.deepEqual(
      driver.normalizeCompletionItems({ items: [{ label: "a" }, { label: "b" }] }),
      [{ label: "a" }, { label: "b" }],
    );
    assert.deepEqual(driver.normalizeCompletionItems(undefined), []);
    const factPath = path.join(root, "fact-contract.mw");
    const factOriginal = "fn source() {}\n";
    const factOverlay = "fn source() {\n    getOr(values, key, fallback)\n}\n";
    fs.writeFileSync(factPath, factOriginal, { mode: 0o600, flag: "wx" });
    const expectedDefinitionUri = "file:///workspace/lib.mw";
    const expectedDefinitionRange = {
      start: { line: 7, character: 3 },
      end: { line: 7, character: 15 },
    };
    const completionExpectation = {
      position: { line: 1, character: 8 },
      completionItems: [{ label: "Ready", kind: "EnumMember" }],
      exactCount: 1,
    };
    const signatureExpectation = {
      position: { line: 1, character: 25 },
      exactCount: 1,
      label: "fn getOr<V>(m: Map<string, V>, key: string, fallback: V): V",
      activeParameter: 1,
    };
    const definitionExpectation = {
      position: { line: 0, character: 4 },
      exactCount: 1,
      targetUri: expectedDefinitionUri,
      selectionRange: expectedDefinitionRange,
    };
    const exactProviders = {
      completion: { items: [{ label: "Ready", kind: 20 }] },
      signature: {
        activeSignature: 0,
        activeParameter: 1,
        signatures: [{ label: signatureExpectation.label }],
      },
      definition: [{
        uri: { scheme: "file", toString: () => expectedDefinitionUri },
        range: expectedDefinitionRange,
      }],
    };
    const runFacts = (providers, facts) => driver.runStableHostSuite(
      factVscode(factPath, root, providers).vscode,
      {
        facts: {
          file: { path: factPath },
          text: factOverlay,
          ...facts,
        },
      },
    );
    const exactFacts = await runFacts(exactProviders, {
      completion: completionExpectation,
      signature: signatureExpectation,
      definition: definitionExpectation,
    });
    const exactFactSummary = driver._test.evidenceSuiteSummary(exactFacts);
    assert.equal(exactFacts.facts.completion.count, 1);
    assert.match(exactFactSummary.completionItemSetHash, /^[0-9a-f]{64}$/u);
    assert.equal(exactFactSummary.signatureCount, 1);
    assert.match(exactFactSummary.signatureHash, /^[0-9a-f]{64}$/u);
    assert.equal(exactFactSummary.signatureActiveParameter, 1);
    assert.equal(exactFactSummary.definitionCount, 1);
    assert.match(exactFactSummary.definitionTargetHash, /^[0-9a-f]{64}$/u);
    await assert.rejects(
      () => runFacts({
        ...exactProviders,
        completion: { items: [{ label: "Ready", kind: 3 }] },
      }, { completion: completionExpectation }),
      /completion is missing Ready\/20/u,
    );
    await assert.rejects(
      () => runFacts({
        ...exactProviders,
        completion: { items: [{ label: "Wrong", kind: 20 }] },
      }, { completion: completionExpectation }),
      /completion is missing Ready\/20/u,
    );
    await assert.rejects(
      () => runFacts({
        ...exactProviders,
        signature: {
          ...exactProviders.signature,
          signatures: [
            { label: signatureExpectation.label },
            { label: "fn other(): int" },
          ],
        },
      }, { signature: signatureExpectation }),
      /unexpected signature count/u,
    );
    await assert.rejects(
      () => runFacts({
        ...exactProviders,
        signature: { ...exactProviders.signature, signatures: [{ label: "fn wrong(): int" }] },
      }, { signature: signatureExpectation }),
      /unexpected signature label/u,
    );
    await assert.rejects(
      () => runFacts({
        ...exactProviders,
        definition: [{
          ...exactProviders.definition[0],
          uri: { scheme: "file", toString: () => "file:///workspace/wrong.mw" },
        }],
      }, { definition: definitionExpectation }),
      /exact expected URI/u,
    );
    await assert.rejects(
      () => runFacts({
        ...exactProviders,
        definition: [{
          ...exactProviders.definition[0],
          range: {
            start: { line: 8, character: 3 },
            end: { line: 8, character: 15 },
          },
        }],
      }, { definition: definitionExpectation }),
      /exact declaration selection range/u,
    );
    const controlledText = "fn source() {}\n";
    let resolveFormat;
    let formatSettled = false;
    const formatCommand = new Promise((resolve) => {
      resolveFormat = resolve;
    });
    const controlledRuntime = {
      vscode: {
        Position: class TestPosition {
          constructor(line, character) {
            this.line = line;
            this.character = character;
          }
        },
        commands: {
          executeCommand: async (command) => {
            assert.equal(command, "editor.action.formatDocument");
            return formatCommand;
          },
        },
      },
      preparedEditor: {
        document: {
          isDirty: true,
          getText: () => controlledText,
        },
        editor: {
          selection: { active: { line: 0, character: 0 } },
        },
      },
    };
    const controlledFormat = driver._test.formatControlledEditor(controlledRuntime, {
      textHash: sha256(controlledText),
      bytes: Buffer.byteLength(controlledText),
      position: { line: 0, character: 0 },
    }).then((result) => {
      formatSettled = true;
      return result;
    });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(formatSettled, false, "controlled Format Document escaped its command promise");
    resolveFormat();
    const controlledFormatResult = await controlledFormat;
    assert.equal(controlledFormatResult.commandCompleted, true);
    assert.equal(formatSettled, true);
    const signalError = new Error("stop");
    signalError.code = "host.signal";
    assert.equal(driver._test.safeError(signalError).code, "host.signal");
    assert.equal(
      driver._test.evidenceSuiteSummary({
        before: { trusted: true },
        after: { trusted: true, targetActive: true },
        format: {
          formatMs: 19,
          formatProviderMs: 17,
          formatCommandMs: 2,
        },
      }).formatMs,
      19,
    );
    assert.deepEqual(
      driver._test.evidenceSuiteSummary({
        before: { trusted: true },
        after: { trusted: true, targetActive: true },
        clockSpecSha256: "a".repeat(64),
        coldStart: {
          targetInactiveBefore: true,
          documentUnopenedBefore: true,
          brokenOnDisk: true,
          documentHash: "b".repeat(64),
          activation: { elapsedMs: 11 },
          firstDiagnostics: { elapsedMs: 22, firstPublication: true },
        },
      }),
      {
        trusted: true,
        targetActive: true,
        clockSpecSha256: "a".repeat(64),
        activationMs: 11,
        formatMs: undefined,
        formatProviderMs: undefined,
        formatCommandMs: undefined,
        hoverMs: undefined,
        definitionMs: undefined,
        firstDiagnosticsMs: 22,
        updatedDiagnosticsMs: undefined,
        coldStartTargetInactive: true,
        coldStartDocumentUnopened: true,
        coldStartBrokenOnDisk: true,
        coldStartDocumentHash: "b".repeat(64),
        coldStartFirstPublication: true,
        canonicalHash: undefined,
        formatOnSaveDefaultOff: undefined,
        formatOnSaveLanguageOptIn: undefined,
        overlayHash: undefined,
        completionCount: undefined,
        completionMs: undefined,
        completionItemSetHash: undefined,
        signatureCount: undefined,
        signatureMs: undefined,
        signatureHash: undefined,
        signatureActiveParameter: undefined,
        definitionCount: undefined,
        definitionTargetHash: undefined,
        documentSymbolCount: undefined,
      },
    );

    assert.deepEqual(driver.parseControlLine('{"id":"1","op":"state","args":{}}'), {
      id: "1",
      op: "state",
      args: {},
    });
    assert.throws(() => driver.parseControlLine('{"id":"1","op":"state","extra":1}'));

    const controlRecordPath = path.join(root, "control-record.jsonl");
    fs.writeFileSync(
      controlRecordPath,
      Buffer.concat(Array.from({ length: 255 }, (_, index) =>
        controlLine(`record${String(index).padStart(3, "0")}`))),
      { mode: 0o600, flag: "wx" },
    );
    await withControlDriver(controlRecordPath, async (executed, evidencePath) => {
      fs.appendFileSync(controlRecordPath, controlLine("record255"));
      await waitUntil("control record N execution", () => executed.length === 1);
      fs.appendFileSync(controlRecordPath, controlLine("record256"));
      await waitUntil(
        "control record N+1 fault",
        () => controlFault(evidencePath, "control record count exceeded"),
      );
      assert.deepEqual(executed, ["workbench.action.quit"]);
    });

    const controlBytePath = path.join(root, "control-byte.jsonl");
    const fullControlLineBytes = 64 * 1024;
    fs.writeFileSync(
      controlBytePath,
      Buffer.concat(Array.from({ length: 15 }, (_, index) =>
        controlLine(`byte${String(index).padStart(3, "0")}`, fullControlLineBytes))),
      { mode: 0o600, flag: "wx" },
    );
    await withControlDriver(controlBytePath, async (executed, evidencePath) => {
      fs.appendFileSync(controlBytePath, controlLine("byte015", fullControlLineBytes));
      assert.equal(fs.statSync(controlBytePath).size, 1024 * 1024);
      await waitUntil("control byte N execution", () => executed.length === 1);
      fs.appendFileSync(controlBytePath, controlLine("byte016"));
      await waitUntil(
        "control byte N+1 fault",
        () => controlFault(evidencePath, "control path identity or byte bound changed"),
      );
      assert.deepEqual(executed, ["workbench.action.quit"]);
    });

    const controlLineBoundaryPath = path.join(root, "control-line-boundary.jsonl");
    fs.writeFileSync(controlLineBoundaryPath, "", { mode: 0o600, flag: "wx" });
    await withControlDriver(controlLineBoundaryPath, async (executed, evidencePath) => {
      const admitted = controlLine("line-boundary-n", 64 * 1024);
      fs.appendFileSync(controlLineBoundaryPath, admitted);
      await waitUntil("control line N execution", () => executed.length === 1);
      await waitUntil(
        "control line N receipts",
        () => jsonLines(evidencePath).some((record) => record.event === "control.pass"),
      );
      const requestSha256 = sha256(admitted);
      const receipts = jsonLines(evidencePath).filter((record) =>
        record.data?.id === "line-boundary-n" && record.event.startsWith("control."));
      assert.deepEqual(
        receipts.map((record) => [record.event, Object.keys(record.data).sort(), record.data.requestSha256]),
        [
          ["control.accepted", ["id", "op", "requestSha256"], requestSha256],
          ["control.pass", ["id", "op", "requestSha256", "result"], requestSha256],
        ],
      );
      fs.appendFileSync(controlLineBoundaryPath, controlLine("line-boundary-n-plus-one", 64 * 1024 + 1));
      await waitUntil(
        "control line N+1 fault",
        () => controlFault(evidencePath, "oversized record"),
      );
      assert.deepEqual(executed, ["workbench.action.quit"]);
    });

    const evidenceLineBoundary = path.join(root, "evidence-line-boundary.jsonl");
    const evidenceLineWriter = driver.createEvidenceWriter(evidenceLineBoundary);
    evidenceLineWriter.emit("line.boundary", {
      padding: evidencePaddingForExactLine(0, "line.boundary", 64 * 1024),
    });
    assert.equal(fs.statSync(evidenceLineBoundary).size, 64 * 1024);
    const evidenceLineOverflow = path.join(root, "evidence-line-overflow.jsonl");
    const evidenceOverflowWriter = driver.createEvidenceWriter(evidenceLineOverflow);
    assert.throws(
      () => evidenceOverflowWriter.emit("line.overflow", {
        padding: evidencePaddingForExactLine(0, "line.overflow", 64 * 1024 + 1),
      }),
      /capacity exceeded/u,
    );
    assert.equal(fs.existsSync(evidenceLineOverflow), false);
    assert.equal(fs.existsSync(`${evidenceLineOverflow}.lock`), false);

    const controlIdentityPath = path.join(root, "control-identity.jsonl");
    fs.writeFileSync(controlIdentityPath, "", { mode: 0o600, flag: "wx" });
    await withControlDriver(controlIdentityPath, async (executed, evidencePath) => {
      const partialLine = controlLine("partial001");
      const split = Math.floor(partialLine.length / 2);
      fs.appendFileSync(controlIdentityPath, partialLine.subarray(0, split));
      await new Promise((resolve) => setTimeout(resolve, 75));
      assert.equal(executed.length, 0);
      fs.appendFileSync(controlIdentityPath, partialLine.subarray(split));
      await waitUntil("complete control record execution", () => executed.length === 1);
      fs.renameSync(controlIdentityPath, `${controlIdentityPath}.moved`);
      fs.writeFileSync(controlIdentityPath, controlLine("replacement001"), {
        mode: 0o600,
        flag: "wx",
      });
      await waitUntil(
        "control replacement fault",
        () => controlFault(evidencePath, "control path identity or byte bound changed"),
      );
      assert.deepEqual(executed, ["workbench.action.quit"]);
    });

    const controlPrefixPath = path.join(root, "control-prefix.jsonl");
    fs.writeFileSync(controlPrefixPath, "", { mode: 0o600, flag: "wx" });
    await withControlDriver(controlPrefixPath, async (executed, evidencePath) => {
      const original = controlLine("prefix-original");
      fs.appendFileSync(controlPrefixPath, original);
      await waitUntil("control prefix initial execution", () => executed.length === 1);
      const replacement = Buffer.from(original);
      const marker = replacement.indexOf(Buffer.from("prefix-original"));
      assert.notEqual(marker, -1);
      replacement[marker] = "q".charCodeAt(0);
      const descriptor = fs.openSync(controlPrefixPath, fs.constants.O_WRONLY);
      try {
        assert.equal(fs.writeSync(descriptor, replacement, 0, replacement.length, 0), replacement.length);
        fs.fsyncSync(descriptor);
      } finally {
        fs.closeSync(descriptor);
      }
      await waitUntil(
        "control same-inode rewrite fault",
        () => controlFault(evidencePath, "consumed prefix"),
      );
      assert.deepEqual(executed, ["workbench.action.quit"]);
    });

    const ipcRootPath = path.join(root, "ipc");
    fs.mkdirSync(ipcRootPath, { mode: 0o700 });
    const ipcRoot = fs.realpathSync(ipcRootPath);
    const clockSpecPath = path.join(ipcRoot, "clock.json");
    const clockSpecBytes = Buffer.from('{"targetExtensionId":"marrow-project.marrow"}\n');
    const clockSpecSha256 = sha256(clockSpecBytes);
    fs.writeFileSync(clockSpecPath, clockSpecBytes, {
      mode: 0o600,
    });
    assert.deepEqual(
      driver._test.clockSampleSpec(
        {
          specPath: clockSpecPath,
          endpoint: "hover",
          delayMs: 26,
          expectedSpecSha256: clockSpecSha256,
        },
        ipcRoot,
      ),
      {
        targetExtensionId: "marrow-project.marrow",
        clockSpecSha256,
        delayMsByEndpoint: { hover: 26 },
      },
    );
    assert.throws(() =>
      driver._test.clockSampleSpec(
        {
          specPath: clockSpecPath,
          endpoint: "hover",
          delayMs: 26,
          expectedSpecSha256: clockSpecSha256,
          extra: true,
        },
        ipcRoot,
      ));
    assert.throws(() =>
      driver._test.clockSampleSpec(
        {
          specPath: clockSpecPath,
          endpoint: "hover",
          delayMs: 26,
          expectedSpecSha256: "0".repeat(64),
        },
        ipcRoot,
      ));
    fs.writeFileSync(
      clockSpecPath,
      Buffer.from('{"targetExtensionId":"marrow-project.changed"}\n'),
    );
    assert.throws(
      () => driver._test.clockSampleSpec(
        {
          specPath: clockSpecPath,
          endpoint: "hover",
          delayMs: 26,
          expectedSpecSha256: clockSpecSha256,
        },
        ipcRoot,
      ),
      /digest differs/u,
    );
    fs.writeFileSync(clockSpecPath, clockSpecBytes);
    const outsideClockSpec = path.join(root, "outside.json");
    fs.writeFileSync(outsideClockSpec, '{}\n', { mode: 0o600 });
    assert.throws(() =>
      driver._test.clockSampleSpec(
        {
          specPath: outsideClockSpec,
          endpoint: "hover",
          delayMs: 26,
          expectedSpecSha256: sha256(Buffer.from('{}\n')),
        },
        ipcRoot,
      ));

    const brokenPath = path.join(root, "cold-broken.mw");
    const brokenBytes = Buffer.from("fn broken( {\n", "utf8");
    fs.writeFileSync(brokenPath, brokenBytes, { mode: 0o600, flag: "wx" });
    const coldSpec = {
      file: { path: brokenPath },
      expectedTextHash: sha256(brokenBytes),
      firstDiagnostics: { minCount: 1, includeCodes: ["cold.test"] },
      timeoutMs: 1_000,
    };
    for (const endpoint of ["activation", "firstDiagnostics"]) {
      const cold = coldVscode(brokenPath, root);
      const result = await driver._test.runColdStartSuite(
        cold.vscode,
        {
          targetExtensionId: "marrow-project.marrow",
          delayMsByEndpoint: { [endpoint]: 8 },
        },
        coldSpec,
      );
      assert.equal(result.targetInactiveBefore, true);
      assert.equal(result.documentUnopenedBefore, true);
      assert.equal(result.brokenOnDisk, true);
      assert.equal(result.firstDiagnostics.firstPublication, true);
      assert.equal(result.firstDiagnostics.count, 1);
      assert.ok(result[endpoint].elapsedMs >= 7, `${endpoint} delay escaped its timed interval`);
      const otherEndpoint = endpoint === "activation" ? "firstDiagnostics" : "activation";
      assert.ok(
        result[endpoint].elapsedMs > result[otherEndpoint].elapsedMs,
        `${endpoint} delay contaminated ${otherEndpoint}`,
      );
      assert.equal(cold.openCount, 1);
    }
    const activeCold = coldVscode(brokenPath, root);
    activeCold.target.isActive = true;
    await assert.rejects(() =>
      driver._test.runColdStartSuite(
        activeCold.vscode,
        { targetExtensionId: "marrow-project.marrow" },
        coldSpec,
      ));
    assert.equal(activeCold.openCount, 0);
    const vacuousCold = coldVscode(brokenPath, root, []);
    await assert.rejects(
      () => driver._test.runColdStartSuite(
        vacuousCold.vscode,
        { targetExtensionId: "marrow-project.marrow" },
        { ...coldSpec, firstDiagnostics: { minCount: 0 } },
      ),
      /did not produce a diagnostic/u,
    );

  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

async function run() {
  const vscode = require("vscode");
  const specPath = process.env.MARROW_VSQ_SPEC_PATH;
  assert.ok(specPath, "MARROW_VSQ_SPEC_PATH is required");
  const spec = driver.readBoundedJson(specPath);
  const extension = vscode.extensions.getExtension("marrow-project.marrow-vsq-host-driver");
  assert.ok(extension, "host-driver development extension is not installed");
  const api = await extension.activate();
  assert.equal(typeof api.runStableHostSuite, "function");
  return api.runStableHostSuite(spec);
}

module.exports = { run };

if (require.main === module) {
  runSelfTests().then(
    () => process.stdout.write("host-driver self-tests: PASS\n"),
    (error) => {
      process.stderr.write(`${error.stack || error}\n`);
      process.exitCode = 1;
    },
  );
}
