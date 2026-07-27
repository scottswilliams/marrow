#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  closeSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { normalizeVsixModes } from "./artifact-identity.mjs";

const HERE = dirname(new URL(import.meta.url).pathname);
const REPO = resolve(HERE, "../../..");
const EDITOR = resolve(HERE, "..");
const DRIVER = join(HERE, "host-driver");
const TARGET = "/Users/scottwilliams/Dev/.build/marrow-targets/vsq01-main";
const CODE = "/Applications/Visual Studio Code.app/Contents/MacOS/Code";
const CLI = "/Applications/Visual Studio Code.app/Contents/Resources/app/out/cli.js";
const PRODUCT = "/Applications/Visual Studio Code.app/Contents/Resources/app/product.json";
const THEME_ROOT =
  "/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/theme-defaults";
const EXPECTED_HEAD = "56e2ae4c3778af6cb22487d9af5a73dc4476cda1";
const EXPECTED_SERVER =
  "39c12b09f699ab1e81cd0375d16f23c5a577110d72ba6c230d952cfd59e2a247";
const TARGET_ID = "marrow-project.marrow";
const DRIVER_ID = "marrow-project.marrow-vsq-host-driver";
const EXTENSION_PACKAGE = join(EDITOR, "package.json");
const LANGUAGE_CONFIGURATION = join(EDITOR, "language-configuration.json");
const GRAMMAR = join(EDITOR, "syntaxes/marrow.tmLanguage.json");
const EXTENSION_SOURCE = join(EDITOR, "src/extension.ts");
const EXTENSION_PACKAGE_SHA256 =
  "84a9f2777844d2429d5dc9f44f17fa43eaa713126375942ff1a12f43c0533540";
const LANGUAGE_CONFIGURATION_SHA256 =
  "e4b263194132d3d6e56e7d37225dd2b9357b534bdf0aac3599e3acb28aeb6aaf";
const GRAMMAR_SHA256 =
  "10b2e40a057fa3c44d1b2a01cc4b53f4e7b9d6dddca7abefba78ce2b5fe2a4ad";
const MAX_OUTPUT = 4 * 1024 * 1024;
const MAX_EVIDENCE = 4 * 1024 * 1024;
const MAX_COMMANDS = 128;
const MAX_COMMAND_RECORD = 32 * 1024;
const MAX_COMMAND_LOG = 512 * 1024;
const COMMAND_TIMEOUT_MS = 120_000;
const OWNED_PATHS = Object.freeze([
  "editors/vscode/gate/installed-probe.mjs",
  "editors/vscode/gate/verify-vsix.mjs",
  "editors/vscode/gate/artifact-identity.mjs",
  "editors/vscode/gate/real-host.mjs",
  "editors/vscode/gate/host-driver/package.json",
  "editors/vscode/gate/host-driver/extension.cjs",
  "editors/vscode/gate/host-driver/tests.cjs",
  "editors/vscode/src/extension.ts",
]);
const HOST_TIMEOUT_MS = 30_000;
const TARGET_VSIX_LIMIT = 32 * 1024 * 1024;
const DRIVER_VSIX_LIMIT = 4 * 1024 * 1024;

const STAGE_INPUTS = Object.freeze([
  ".npmrc",
  ".vscodeignore",
  "LICENSE",
  "README.md",
  "icons",
  "language-configuration.json",
  "package-lock.json",
  "package.json",
  "src",
  "syntaxes",
  "tsconfig.json",
]);

const CODE_FILES = Object.freeze({
  [CODE]: "e1e3268741a2658a22b31e82b58a42fa48be73f64fc2de006be48a2ba136b930",
  [PRODUCT]: "e8ce947ba231a32c15993a6068add7de7268be112c928005017bd3df7727e06d",
  [CLI]: "7930c3f6f8bc2854f6fec7091b220a0506fe7948014963d336bb4c6ad69af636",
});

const THEME_FILES = Object.freeze({
  "package.json": "1188ab40303da2345797c509f81c87c3e168249e50009bdec24a1ea28b515bb4",
  "package.nls.json": "8eba17a4db2c6db687367cec5553292732a1afcac56b472c00b4018a4af69486",
  "themes/2026-dark.json": "10f8dbeb38fb722394096df9f91d5042af31b1f28946a84340dd763fc01f01f7",
  "themes/dark_modern.json": "7e87bcc01b7b4ca057fc1b4463cddcc9b3f494bd6566d22d9aaddadab2d45db4",
  "themes/dark_plus.json": "88f5b662378cbe39473a4a8d916a1b4ec580f85858876eaec440288aee2852df",
  "themes/dark_vs.json": "3c9d8a8056638204ab6419fa43ada5ae3ea1ae5ecf262ba550b7f554b57f230c",
  "themes/2026-light.json": "e30ba1939d7d2535ac723a44386d345396b59a5a442d5f6f58de303633dda2d2",
  "themes/light_modern.json": "cad5a1a2da4d66a2a0354f2e41bd45dfbd206ca58b2ba8c920a02dcd8c16c989",
  "themes/light_plus.json": "bf64f3ec2a80788fad115086bd10fd4136dd1d760ec478f4107756f098d101ea",
  "themes/light_vs.json": "c3273b4a9d00bfe126f138285a6e460bd9f62622ea4861f4590bdee26a889240",
  "themes/hc_black.json": "2d2a8b6d47db029ee826bc0f235f7a7d76142a82bffec485b12e38855d9feaf9",
});

const SCOPE_ROWS = Object.freeze([
  ["status", { line: 2, character: 3 }, "Status",
    ["comment.line.double-slash.marrow", "source.marrow"], "Comment"],
  ["status", { line: 11, character: 4 }, "fn",
    ["keyword.declaration.marrow", "source.marrow"], "Other"],
  ["status", { line: 12, character: 17 }, "active",
    ["string.quoted.double.marrow", "source.marrow"], "String"],
  ["status", { line: 12, character: 13 }, "==",
    ["keyword.operator.marrow", "source.marrow"], "Other"],
  ["changeset", { line: 20, character: 29 }, "1",
    ["constant.numeric.integer.marrow", "source.marrow"], "Other"],
  ["changeset", { line: 216, character: 22 }, "^",
    ["punctuation.definition.variable.marrow", "source.marrow"], "Other"],
  ["scratch", { line: 2, character: 4 }, "(",
    ["punctuation.section.group.marrow", "source.marrow"], "Other"],
  ["scratch", { line: 3, character: 16 }, "\\n",
    ["constant.character.escape.marrow", "string.quoted.double.marrow", "source.marrow"], "String"],
  ["status", { line: 11, character: 7 }, "patientStatusValid",
    ["source.marrow"], "Other"],
]);

const THEMES = Object.freeze([
  ["Dark 2026", 2, "vs-dark"],
  ["Light 2026", 1, "vs"],
  ["Default High Contrast", 3, "hc-black"],
]);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function shaFile(path) {
  return sha256(readFileSync(path));
}

function requireCondition(value, message) {
  if (!value) throw new Error(message);
}

let activeCommandLog;

function appendCommand(log, record) {
  requireCondition(log.length < MAX_COMMANDS, "command log exceeds 128 records");
  const encoded = Buffer.from(JSON.stringify(record));
  requireCondition(encoded.length <= MAX_COMMAND_RECORD, "command record exceeds 32 KiB");
  requireCondition(
    Buffer.byteLength(JSON.stringify([...log, record])) <= MAX_COMMAND_LOG,
    "command log exceeds 512 KiB",
  );
  log.push(record);
}

function recordCommand(record) {
  if (activeCommandLog) appendCommand(activeCommandLog, record);
}

function assertCommandRecorderSource(source) {
  const runStart = source.indexOf("\nfunction run(") + 1;
  const runEnd = source.indexOf("\nfunction preflight(", runStart) + 1;
  const runSource = source.slice(runStart, runEnd);
  requireCondition(
    runStart >= 0 &&
      runEnd > runStart &&
      runSource.indexOf("recordCommand({") >= 0 &&
      runSource.indexOf("recordCommand({") < runSource.indexOf("spawnSync("),
    "sync command recording does not precede spawnSync",
  );
  const launchStart = source.indexOf("\nfunction launchCode(") + 1;
  const launchEnd = source.indexOf("\nasync function closeHost(", launchStart) + 1;
  const launchSource = source.slice(launchStart, launchEnd);
  requireCondition(
    launchStart >= 0 &&
      launchEnd > launchStart &&
      launchSource.indexOf("recordCommand({") >= 0 &&
      launchSource.indexOf("recordCommand({") < launchSource.indexOf("const child = spawn("),
    "Code command recording does not precede spawn",
  );
}

function git(args) {
  const result = spawnSync("/usr/bin/git", ["-C", REPO, ...args], {
    encoding: "utf8",
    maxBuffer: MAX_OUTPUT,
    timeout: 10_000,
  });
  requireCondition(result.status === 0, `git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout.trimEnd();
}

function run(command, args, { cwd = REPO, env = {}, timeoutMs = COMMAND_TIMEOUT_MS } = {}) {
  recordCommand({
    kind: "sync",
    command,
    args: [...args],
    cwd,
    env: Object.fromEntries(Object.entries(env).sort(([left], [right]) =>
      left.localeCompare(right))),
    timeoutMs,
  });
  const result = spawnSync(command, args, {
    cwd,
    env: { ...process.env, ...env },
    encoding: "utf8",
    maxBuffer: MAX_OUTPUT,
    timeout: timeoutMs,
    killSignal: "SIGKILL",
  });
  requireCondition(
    result.status === 0,
    `${basename(command)} ${args.join(" ")} failed ${result.status ?? result.signal}: ` +
      `${result.stderr ?? result.error ?? ""}`.slice(0, 4096),
  );
  return {
    command,
    args,
    cwd,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function preflight() {
  requireCondition(git(["diff", "--cached", "--name-only"]) === "", "candidate has staged changes");
  const changed = new Set([
    ...git(["diff", "--name-only"]).split("\n").filter(Boolean),
    ...git(["ls-files", "--others", "--exclude-standard"]).split("\n").filter(Boolean),
  ]);
  requireCondition(
    [...changed].every((path) => OWNED_PATHS.includes(path)),
    `candidate has out-of-scope changes: ${[...changed].filter((path) => !OWNED_PATHS.includes(path)).join(",")}`,
  );
  requireCondition(
    OWNED_PATHS.every((path) => existsSync(join(REPO, path))),
    "one or more owned A1 files are absent",
  );
  requireCondition(git(["rev-parse", "HEAD"]) === EXPECTED_HEAD, "candidate HEAD drifted");
  requireCondition(shaFile(join(HERE, "installed-probe.mjs")) !== undefined, "probe is absent");
  requireCondition(shaFile(join(HERE, "host-authority.mjs")) ===
    "72dfcf03bbfe735adadaae98f023315751ff0226994be6cf750324ade990083c",
  "frozen A0 source drifted");
  for (const [path, expected] of Object.entries(CODE_FILES)) {
    requireCondition(shaFile(path) === expected, `Code authority drifted: ${path}`);
  }
  const product = JSON.parse(readFileSync(PRODUCT, "utf8"));
  requireCondition(
    product.version === "1.130.0" &&
      product.commit === "1b6a188127eeaf9194f945eb6eb89a657e93c54c" &&
      product.date === "2026-07-22T14:55:04Z",
    "Code product identity drifted",
  );
  for (const [path, expected] of Object.entries(THEME_FILES)) {
    requireCondition(shaFile(join(THEME_ROOT, path)) === expected, `theme authority drifted: ${path}`);
  }
  const canonical = join(TARGET, "release", "marrow");
  requireCondition(existsSync(canonical), "canonical release server is absent");
  requireCondition(shaFile(canonical) === EXPECTED_SERVER, "canonical server digest drifted");
  return { canonical, structural: structuralEditorChecks() };
}

function collectGrammarNames(value, names = new Set()) {
  if (Array.isArray(value)) {
    for (const item of value) collectGrammarNames(item, names);
  } else if (value && typeof value === "object") {
    if (typeof value.name === "string") names.add(value.name);
    for (const item of Object.values(value)) collectGrammarNames(item, names);
  }
  return names;
}

function occurrenceCount(source, needle) {
  return source.split(needle).length - 1;
}

function assertExtensionLifecycleSource(source) {
  const imports = [...source.matchAll(/from "([^"]+)";/gu)].map((match) => match[1]);
  assert.deepEqual(imports, ["vscode", "vscode-languageclient/node"]);
  const classifier = `function isFileMarrowDocument(document: vscode.TextDocument): boolean {
  return document.languageId === "marrow" && document.uri.scheme === "file";
}`;
  requireCondition(
    occurrenceCount(source, classifier) === 1,
    "extension lacks the exact file-scheme Marrow document classifier",
  );
  const hook = `context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((document) => {
      if (isFileMarrowDocument(document)) {
        void startClient(context);
      }
    }),
  );`;
  requireCondition(
    occurrenceCount(source, hook) === 1 &&
      occurrenceCount(source, "vscode.workspace.onDidOpenTextDocument") === 1,
    "extension lacks one context-owned file-document recovery hook",
  );
  const guard = `if (!vscode.workspace.textDocuments.some(isFileMarrowDocument)) {
    return;
  }`;
  requireCondition(
    occurrenceCount(source, guard) === 1,
    "extension lacks the open file-document start guard",
  );
  const start = source.indexOf(
    "async function startClient(context: vscode.ExtensionContext): Promise<void>",
  );
  const guardAt = source.indexOf(guard);
  const platformAt = source.indexOf("process.platform !== SUPPORTED_PLATFORM", start);
  const constructAt = source.indexOf("new LanguageClient(", start);
  requireCondition(
    start >= 0 && guardAt > start && guardAt < platformAt && platformAt < constructAt,
    "extension start guard is not before platform and client construction",
  );
  requireCondition(
    occurrenceCount(
      source,
      'documentSelector: [{ language: "marrow", scheme: "file" }],',
    ) === 1,
    "extension file-only document selector drifted",
  );
  requireCondition(
    occurrenceCount(source, "startClient(context)") === 3,
    "extension start entry points differ from activate/restart/open",
  );
  requireCondition(
    !/document\.getText|workspace\.fs|readFile|middleware\s*:/u.test(source),
    "extension reads document/source bytes or adds middleware",
  );
}

function structuralEditorChecks() {
  requireCondition(
    shaFile(EXTENSION_PACKAGE) === EXTENSION_PACKAGE_SHA256,
    "extension package authority drifted",
  );
  requireCondition(
    shaFile(LANGUAGE_CONFIGURATION) === LANGUAGE_CONFIGURATION_SHA256,
    "language configuration authority drifted",
  );
  requireCondition(shaFile(GRAMMAR) === GRAMMAR_SHA256, "grammar authority drifted");
  assertExtensionLifecycleSource(readFileSync(EXTENSION_SOURCE, "utf8"));
  const manifest = JSON.parse(readFileSync(EXTENSION_PACKAGE, "utf8"));
  assert.deepEqual(manifest.activationEvents, ["onLanguage:marrow"]);
  assert.deepEqual(manifest.capabilities, {
    untrustedWorkspaces: { supported: false },
    virtualWorkspaces: { supported: false },
  });
  assert.deepEqual(manifest.contributes.languages, [{
    id: "marrow",
    aliases: ["Marrow"],
    extensions: [".mw"],
    configuration: "./language-configuration.json",
    icon: { light: "./icons/marrow.svg", dark: "./icons/marrow.svg" },
  }]);
  assert.deepEqual(manifest.contributes.grammars, [{
    language: "marrow",
    scopeName: "source.marrow",
    path: "./syntaxes/marrow.tmLanguage.json",
  }]);
  const configuration = JSON.parse(readFileSync(LANGUAGE_CONFIGURATION, "utf8"));
  assert.deepEqual(configuration, {
    comments: { lineComment: "//" },
    brackets: [["{", "}"], ["[", "]"], ["(", ")"]],
    autoClosingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: "(", close: ")" },
      { open: '"', close: '"', notIn: ["string", "comment"] },
    ],
    surroundingPairs: [["{", "}"], ["[", "]"], ["(", ")"], ['"', '"']],
  });
  const grammar = JSON.parse(readFileSync(GRAMMAR, "utf8"));
  assert.equal(grammar.scopeName, "source.marrow");
  assert.deepEqual(grammar.patterns, [{ include: "#expression" }]);
  const grammarNames = collectGrammarNames(grammar);
  const representativeScopes = [...new Set(
    SCOPE_ROWS.flatMap(([, , , scopes]) => scopes).filter((scope) => scope !== "source.marrow"),
  )].sort();
  for (const scope of representativeScopes) {
    requireCondition(grammarNames.has(scope), `grammar lacks representative scope ${scope}`);
  }
  return {
    packageSha256: shaFile(EXTENSION_PACKAGE),
    languageConfigurationSha256: shaFile(LANGUAGE_CONFIGURATION),
    grammarSha256: shaFile(GRAMMAR),
    representativeScopes,
    pairing: {
      pairs: configuration.autoClosingPairs.length,
      quoteSuppression: configuration.autoClosingPairs.at(-1).notIn,
      brackets: configuration.brackets.length,
    },
  };
}

function copyStage(label, root, canonical) {
  const stage = join(root, `stage-${label}`);
  mkdirSync(stage, { mode: 0o700 });
  for (const entry of STAGE_INPUTS) {
    cpSync(join(EDITOR, entry), join(stage, entry), {
      recursive: true,
      dereference: false,
      errorOnExist: true,
      preserveTimestamps: false,
    });
  }
  run("/usr/bin/env", ["npm", "ci", "--offline", "--ignore-scripts"], { cwd: stage });
  run("/usr/bin/env", ["npm", "run", "compile"], { cwd: stage });
  mkdirSync(join(stage, "server"), { mode: 0o700 });
  copyFileSync(canonical, join(stage, "server", "marrow"));
  chmodSync(join(stage, "server", "marrow"), 0o755);
  const vsix = join(root, `marrow-${label}.vsix`);
  run(join(stage, "node_modules/.bin/vsce"), [
    "package",
    "--target",
    "darwin-arm64",
    "--out",
    vsix,
  ], { cwd: stage });
  normalizeVsixModes(vsix);
  requireCondition(statSync(vsix).size <= TARGET_VSIX_LIMIT, `${label} VSIX exceeds 32 MiB`);
  return { stage, vsix };
}

function buildDriver(root, vsce) {
  const stage = join(root, "driver-stage");
  mkdirSync(stage, { mode: 0o700 });
  for (const path of ["package.json", "extension.cjs", "tests.cjs"]) {
    copyFileSync(join(DRIVER, path), join(stage, path));
    chmodSync(join(stage, path), 0o644);
  }
  copyFileSync(join(EDITOR, "LICENSE"), join(stage, "LICENSE"));
  writeFileSync(
    join(stage, "README.md"),
    "# Marrow VSQ Host Driver\n\nTransient acceptance driver; never shipped.\n",
    { mode: 0o644 },
  );
  const vsix = join(root, "marrow-vsq-host-driver.vsix");
  run(vsce, [
    "package",
    "--no-dependencies",
    "--allow-missing-repository",
    "--out",
    vsix,
  ], { cwd: stage });
  normalizeVsixModes(vsix);
  requireCondition(statSync(vsix).size <= DRIVER_VSIX_LIMIT, "driver VSIX exceeds 4 MiB");
  return { stage, vsix };
}

function install(label, root, vsixes) {
  const profile = join(root, `profile-${label}`);
  const extensions = join(root, `extensions-${label}`);
  mkdirSync(profile, { mode: 0o700 });
  mkdirSync(extensions, { mode: 0o700 });
  for (const vsix of vsixes) {
    run(CODE, [
      CLI,
      "--user-data-dir",
      profile,
      "--extensions-dir",
      extensions,
      "--install-extension",
      vsix,
      "--force",
      "--disable-telemetry",
    ], {
      cwd: root,
      env: { ELECTRON_RUN_AS_NODE: "1" },
      timeoutMs: 30_000,
    });
  }
  return { profile, extensions };
}

function prepareWorkspace(root, canonical) {
  const workspace = join(root, "workspace");
  mkdirSync(join(workspace, "src"), { recursive: true, mode: 0o700 });
  writeFileSync(join(workspace, "marrow.toml"), 'edition = "2026"\n');
  const graphSource = readFileSync(
    join(REPO, "fixtures/v01/conformance/graph_report/src/graph_report.mw"),
    "utf8",
  );
  const graphPath = join(workspace, "src/graph_report.mw");
  writeFileSync(graphPath, graphSource);
  const formatPath = join(workspace, "src/format.mw");
  const unformatted = "module format\n\npub fn f():int{\n return 1\n}\n";
  writeFileSync(formatPath, unformatted);
  const canonicalPath = join(workspace, "canonical-format.mw");
  writeFileSync(canonicalPath, unformatted);
  run(canonical, ["fmt", "--write", canonicalPath], { cwd: workspace, timeoutMs: 10_000 });
  const typingPath = join(workspace, "src/typing.mw");
  writeFileSync(typingPath, "module typing\n");
  const broken = `${graphSource}\nfn broken( {\n}\n`;
  const coldPath = join(workspace, "src/cold-diagnostics.mw");
  writeFileSync(coldPath, broken);
  const completionSource = graphSource.replace("Role::isolated", "Role::");
  requireCondition(completionSource !== graphSource, "completion red does not bite");
  const status = join(workspace, "status.mw");
  const changeset = join(workspace, "changeset.mw");
  const scratch = join(workspace, "scope-scratch.mw");
  copyFileSync(join(REPO, "apps/emr/src/status.mw"), status);
  copyFileSync(join(REPO, "apps/emr/src/changeset.mw"), changeset);
  writeFileSync(scratch, 'module scope\n\nfn f() {\n    const s = "a\\nb"\n}\n');
  requireCondition(shaFile(status) ===
    "4544ed99bd7a23793c85e121c8149a828e52b31c9936c37e6a68731c9d40b252",
  "status scope source drifted");
  requireCondition(shaFile(changeset) ===
    "2dcee57afe5cdb4b005fbf0cc77a3e7600bbb5ce6e3fd0b02a4ad8307c80158a",
  "changeset scope source drifted");
  requireCondition(shaFile(scratch) ===
    "69e71d721a006159006ed58781d1d51f26ea4a3b9be41fd00a275e039045fba9",
  "scratch scope source drifted");
  const emptyWorkspace = join(root, "empty-workspace");
  const secondWorkspace = join(root, "second-workspace");
  mkdirSync(emptyWorkspace, { mode: 0o700 });
  mkdirSync(secondWorkspace, { mode: 0o700 });
  const multiWorkspace = join(root, "multi.code-workspace");
  writeFileSync(
    multiWorkspace,
    `${JSON.stringify({
      folders: [{ path: workspace }, { path: secondWorkspace }],
    })}\n`,
    { mode: 0o600, flag: "wx" },
  );
  return {
    workspace,
    graphPath,
    graphSource,
    formatPath,
    canonicalPath,
    typingPath,
    coldPath,
    emptyWorkspace,
    secondWorkspace,
    multiWorkspace,
    unformatted,
    broken,
    completionSource,
    positions: {
      completion: filePosition(graphSource, "Role::isolated", "Role::".length),
      signature: filePosition(
        graphSource,
        "getOr(reached, e.src, false)",
        "getOr(reached, ".length,
      ),
      hover: filePosition(graphSource, "classifyRole(o, i)", 1),
      definition: filePosition(graphSource, "classifyRole(o, i)", 1),
    },
    definitionTarget: {
      uri: pathToFileURL(graphPath).href,
      selectionRange: fileRange(
        graphSource,
        "fn classifyRole",
        "fn ".length,
        "classifyRole".length,
      ),
    },
    scope: { status, changeset, scratch },
  };
}

function filePosition(text, needle, offset = 0) {
  const index = text.indexOf(needle);
  requireCondition(index >= 0, `fixture needle is absent: ${needle}`);
  const before = text.slice(0, index + offset);
  const lines = before.split("\n");
  return { line: lines.length - 1, character: lines.at(-1).length };
}

function fileRange(text, needle, offset, length) {
  const start = filePosition(text, needle, offset);
  return {
    start,
    end: { line: start.line, character: start.character + length },
  };
}

function comprehensiveSpec(fixture) {
  return {
    targetExtensionId: TARGET_ID,
    activation: { file: { path: fixture.graphPath }, timeoutMs: 8_000 },
    format: {
      file: { path: fixture.formatPath },
      unformattedText: fixture.unformatted,
      canonicalPath: fixture.canonicalPath,
      invoke: "provider",
      timeoutMs: 8_000,
    },
    facts: {
      file: { path: fixture.graphPath },
      text: fixture.broken,
      firstDiagnostics: { minCount: 1, timeoutMs: 8_000 },
      queriesAfterUpdate: true,
      completion: {
        position: fixture.positions.completion,
        completionItems: ["internal", "isolated", "sink", "source"].map((label) => ({
          label,
          kind: "EnumMember",
        })),
        exactCount: 4,
        text: fixture.completionSource,
        diagnostics: { minCount: 1, timeoutMs: 8_000 },
        restoreDiagnostics: { exactCount: 0, timeoutMs: 8_000 },
      },
      signature: {
        position: fixture.positions.signature,
        label: "fn getOr<V>(m: Map<string, V>, key: string, fallback: V): V",
        activeParameter: 1,
        exactCount: 1,
      },
      hover: { position: fixture.positions.hover, includes: "classifyRole" },
      definition: {
        position: fixture.positions.definition,
        exactCount: 1,
        targetUri: fixture.definitionTarget.uri,
        selectionRange: fixture.definitionTarget.selectionRange,
      },
      documentSymbols: {
        includeNames: ["Pair", "Edge", "Role", "getOr", "classifyRole", "report"],
      },
      updatedText: fixture.graphSource,
      updatedDiagnostics: { exactCount: 0, timeoutMs: 8_000 },
    },
  };
}

function coldSpec(fixture) {
  return {
    targetExtensionId: TARGET_ID,
    coldStart: {
      file: { path: fixture.coldPath },
      expectedTextHash: shaFile(fixture.coldPath),
      firstDiagnostics: { minCount: 1 },
      timeoutMs: 8_000,
    },
  };
}

function readyTimingSpec(fixture, endpoint) {
  const base = {
    targetExtensionId: TARGET_ID,
  };
  const format = {
    ...base,
    format: {
      file: { path: fixture.formatPath },
      unformattedText: fixture.unformatted,
      canonicalPath: fixture.canonicalPath,
      invoke: "provider",
      timeoutMs: 8_000,
    },
  };
  const completion = {
    ...base,
    facts: {
      file: { path: fixture.graphPath },
      text: fixture.broken,
      completion: {
        position: fixture.positions.completion,
        completionItems: ["internal", "isolated", "sink", "source"].map((label) => ({
          label,
          kind: "EnumMember",
        })),
        exactCount: 4,
        text: fixture.completionSource,
        diagnostics: { minCount: 1, timeoutMs: 8_000 },
        restoreDiagnostics: { minCount: 1, timeoutMs: 8_000 },
      },
    },
  };
  const signature = {
    ...base,
    facts: {
      file: { path: fixture.graphPath },
      text: fixture.graphSource,
      signature: {
        position: fixture.positions.signature,
        label: "fn getOr<V>(m: Map<string, V>, key: string, fallback: V): V",
        activeParameter: 1,
        exactCount: 1,
      },
    },
  };
  const specs = { format, completion, signature };
  requireCondition(Object.hasOwn(specs, endpoint), `unknown ready timing endpoint ${endpoint}`);
  return specs[endpoint];
}

function absenceArgs(file, fixture, expectedTargetActive, expectedLanguageId) {
  return {
    file,
    expectedLanguageId,
    targetExtensionId: TARGET_ID,
    expectedTargetActive,
    position: fixture.positions.completion,
    completionItems: ["internal", "isolated", "sink", "source"].map((label) => ({
      label,
      kind: "EnumMember",
    })),
  };
}

function assertProviderAbsence(result, label) {
  const responders = Object.entries(result.providers)
    .filter(([, responds]) => responds)
    .map(([name]) => name);
  requireCondition(responders.length === 0, `${label} providers responded: ${responders.join(",")}`);
  requireCondition(
    result.completion.targetMatchCount === 0,
    `${label} returned target completion candidates`,
  );
}

function editorExpectation(text, position) {
  return {
    textHash: sha256(Buffer.from(text)),
    bytes: Buffer.byteLength(text),
    ...(position === undefined ? {} : { position }),
    timeoutMs: 4_000,
  };
}

async function prepareEditor(host, file, text, position) {
  await host.channel.send("editor.prepare", {
    file: { path: file },
    text,
    position,
  });
  await host.cdp.focusEditor(file);
  await host.channel.send("editor.position", { position });
  await host.channel.send("editor.assert", editorExpectation(text, position));
  await host.cdp.focusEditor(file);
}

async function runRealTyping(host, fixture) {
  const pairs = [["{", "}"], ["[", "]"], ["(", ")"], ['"', '"']];
  for (const [open, close] of pairs) {
    await prepareEditor(host, fixture.typingPath, "", { line: 0, character: 0 });
    await host.channel.send("editor.type", { text: open });
    await host.channel.send("editor.wait",
      editorExpectation(`${open}${close}`, { line: 0, character: 1 }));
    await host.channel.send("editor.type", { text: close });
    await host.channel.send("editor.wait",
      editorExpectation(`${open}${close}`, { line: 0, character: 2 }));
    await host.channel.send("editor.restore");
  }
  await prepareEditor(host, fixture.typingPath, "// x", { line: 0, character: 3 });
  await host.channel.send("editor.type", { text: '"' });
  await host.channel.send("editor.wait",
    editorExpectation('// "x', { line: 0, character: 4 }));
  await host.channel.send("editor.restore");

  await prepareEditor(host, fixture.typingPath, 'const s = "value"', {
    line: 0,
    character: 11,
  });
  await host.channel.send("editor.type", { text: '"' });
  await host.channel.send("editor.wait",
    editorExpectation('const s = ""value"', { line: 0, character: 12 }));
  await host.channel.send("editor.restore");

  await prepareEditor(host, fixture.typingPath, "{}", { line: 0, character: 1 });
  await host.cdp.pressEnter();
  const indent = await host.channel.send("editor.assertIndent");
  await host.channel.send("editor.restore");
  return {
    pairs: pairs.length,
    stepOvers: pairs.length,
    quoteSuppressionContexts: 2,
    enterIndentOutdent: true,
    indentWidth: indent.indentWidth,
  };
}

function readEvidence(path) {
  if (!existsSync(path)) return [];
  const bytes = readFileSync(path);
  requireCondition(bytes.length <= MAX_EVIDENCE, "driver evidence exceeds 4 MiB");
  requireCondition(bytes.length === 0 || bytes.at(-1) === 0x0a, "driver evidence is partial");
  const lines = bytes.length === 0 ? [] : bytes.toString("utf8").split("\n").slice(0, -1);
  return lines.map((line, sequence) => {
    const record = JSON.parse(line);
    requireCondition(
      record.schema === 1 && record.sequence === sequence &&
        typeof record.event === "string" && record.data && typeof record.data === "object",
      "driver evidence record is invalid",
    );
    return record;
  });
}

class DriverChannel {
  constructor(root) {
    this.control = join(root, "control.jsonl");
    this.evidence = join(root, "driver-evidence.jsonl");
    writeFileSync(this.control, "", { mode: 0o600, flag: "wx" });
    this.id = 0;
  }

  async waitReady(child, timeoutMs = HOST_TIMEOUT_MS) {
    return waitFor("driver ready", () => {
      requireCondition(child.exitCode === null,
        `Code exited before driver ready: code=${child.exitCode} signal=${child.signalCode}`);
      return readEvidence(this.evidence).find((record) => record.event === "driver.ready");
    }, timeoutMs);
  }

  async send(op, args = {}, timeoutMs = HOST_TIMEOUT_MS) {
    const id = `c${String(++this.id).padStart(4, "0")}`;
    const line = Buffer.from(`${JSON.stringify({ id, op, args })}\n`);
    requireCondition(line.length <= 64 * 1024, "driver command is oversized");
    const descriptor = openSync(this.control, "a");
    try {
      writeFileSync(descriptor, line);
    } finally {
      closeSync(descriptor);
    }
    const terminal = await waitFor(`driver ${op}`, () =>
      readEvidence(this.evidence).find((record) =>
        ["control.pass", "control.fail"].includes(record.event) && record.data?.id === id),
    timeoutMs);
    requireCondition(terminal.event === "control.pass",
      `${op} failed: ${terminal.data?.error?.message ?? "unknown"}`);
    return terminal.data.result;
  }
}

function processSnapshot() {
  const result = spawnSync("/bin/ps", ["-axo", "pid=,ppid=,pgid=,command="], {
    encoding: "utf8",
    maxBuffer: MAX_OUTPUT,
    timeout: 5_000,
  });
  requireCondition(result.status === 0, `ps failed: ${result.stderr}`);
  return result.stdout.split("\n").flatMap((line) => {
    const match = /^\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/u.exec(line);
    return match ? [{
      pid: Number(match[1]),
      ppid: Number(match[2]),
      pgid: Number(match[3]),
      command: match[4],
    }] : [];
  });
}

function descendants(rootPid, known = new Set([rootPid])) {
  const rows = processSnapshot();
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) {
      if (known.has(row.ppid) && !known.has(row.pid)) {
        known.add(row.pid);
        changed = true;
      }
    }
  }
  return rows.filter((row) => known.has(row.pid));
}

function serverPids(rows) {
  return rows
    .filter((row) => /\/server\/marrow lsp(?:\s|$)/u.test(row.command))
    .map((row) => row.pid)
    .sort((a, b) => a - b);
}

function observedDescriptors(pids) {
  if (pids.length === 0) return { bytes: 0, sha256: sha256(Buffer.alloc(0)) };
  const result = spawnSync("/usr/sbin/lsof", [
    "-n",
    "-P",
    "-a",
    "-p",
    pids.join(","),
    "-F0pftnP",
  ], { encoding: null, maxBuffer: MAX_OUTPUT, timeout: 5_000 });
  const bytes = result.stdout ?? Buffer.alloc(0);
  return { bytes: bytes.length, sha256: sha256(bytes), status: result.status };
}

async function waitFor(name, predicate, timeoutMs = HOST_TIMEOUT_MS, intervalMs = 25) {
  const deadline = performance.now() + timeoutMs;
  let last;
  while (performance.now() <= deadline) {
    last = await predicate();
    if (last) return last;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  throw new Error(`${name} timed out; last=${JSON.stringify(last)}`);
}

class CdpPipe {
  constructor(child) {
    this.input = child.stdio[3];
    this.output = child.stdio[4];
    requireCondition(this.input && this.output, "Code did not expose CDP pipe descriptors");
    this.buffer = Buffer.alloc(0);
    this.pending = new Map();
    this.nextId = 0;
    this.sessionId = undefined;
    this.output.on("data", (chunk) => this.onData(chunk));
  }

  onData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    requireCondition(this.buffer.length <= MAX_OUTPUT, "CDP pipe buffer exceeded 4 MiB");
    for (;;) {
      const end = this.buffer.indexOf(0);
      if (end < 0) return;
      const frame = this.buffer.subarray(0, end);
      this.buffer = this.buffer.subarray(end + 1);
      if (frame.length === 0) continue;
      const message = JSON.parse(frame.toString("utf8"));
      if (message.id !== undefined) {
        const pending = this.pending.get(message.id);
        if (pending) {
          this.pending.delete(message.id);
          if (message.error) pending.reject(new Error(message.error.message));
          else pending.resolve(message.result);
        }
      }
    }
  }

  call(method, params = {}, sessionId = this.sessionId, timeoutMs = 5_000) {
    const id = ++this.nextId;
    const message = Buffer.from(`${JSON.stringify({
      id,
      method,
      params,
      ...(sessionId ? { sessionId } : {}),
    })}\0`);
    requireCondition(message.length <= MAX_OUTPUT, "CDP frame exceeds 4 MiB");
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP ${method} timed out`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.input.write(message);
    });
  }

  async attachWorkbench() {
    await new Promise((resolve) => setTimeout(resolve, 1_000));
    await this.call("Target.setDiscoverTargets", { discover: true }, undefined, 15_000);
    const result = await waitFor("CDP workbench target", async () => {
      const targets = await this.call("Target.getTargets", {}, undefined);
      return targets.targetInfos.find((target) =>
        target.type === "page" && /workbench|Visual Studio Code/iu.test(`${target.title} ${target.url}`));
    }, 8_000);
    const attached = await this.call(
      "Target.attachToTarget",
      { targetId: result.targetId, flatten: true },
      undefined,
    );
    this.sessionId = attached.sessionId;
    await this.call("Runtime.enable");
    await this.call("Emulation.setFocusEmulationEnabled", { enabled: true });
    await this.call("Page.bringToFront");
  }

  async evaluate(expression) {
    const result = await this.call("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    requireCondition(result.exceptionDetails === undefined,
      `CDP evaluation failed: ${result.exceptionDetails?.text ?? "unknown"}`);
    return result.result?.value;
  }

  async focusEditor(file) {
    await this.call("Page.bringToFront");
    const focus = await this.evaluate(`(() => {
      const targetUri = ${JSON.stringify(pathToFileURL(file).href)};
      const visible = node => {
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        return node.isConnected && rect.width > 0 && rect.height > 0 &&
          style.visibility !== "hidden" && style.display !== "none";
      };
      const editors = [...document.querySelectorAll(".monaco-editor[data-uri]")]
        .filter(node => node.getAttribute("data-uri") === targetUri && visible(node));
      const editor = editors.length === 1 ? editors[0] : undefined;
      const inputs = editor
        ? [...editor.querySelectorAll(".native-edit-context, textarea.inputarea")]
            .filter(node => node.isConnected)
        : [];
      const lines = editor
        ? [...editor.querySelectorAll(".view-lines > .view-line")].filter(visible)
        : [];
      const rect = lines[0]?.getBoundingClientRect();
      return {
        editorCount: editors.length,
        inputCount: inputs.length,
        x: rect ? rect.x + 1 : null,
        y: rect ? rect.y + Math.min(1, rect.height / 2) : null,
      };
    })()`);
    requireCondition(
      focus?.editorCount === 1 && focus.inputCount === 1 &&
        Number.isFinite(focus.x) && Number.isFinite(focus.y),
      `visible editor input is ambiguous: ${JSON.stringify(focus)}`,
    );
    for (const type of ["mousePressed", "mouseReleased"]) {
      await this.call("Input.dispatchMouseEvent", {
        type,
        x: focus.x,
        y: focus.y,
        button: "left",
        clickCount: 1,
      });
    }
    const active = await this.evaluate(`(() => {
      const targetUri = ${JSON.stringify(pathToFileURL(file).href)};
      const editors = [...document.querySelectorAll(".monaco-editor[data-uri]")]
        .filter(node => node.getAttribute("data-uri") === targetUri);
      const nativeInputs = editors.flatMap(editor =>
        [...editor.querySelectorAll(".native-edit-context")]).filter(node => node.isConnected);
      const textareas = editors.flatMap(editor =>
        [...editor.querySelectorAll("textarea.inputarea")]).filter(node => node.isConnected);
      const inputs = nativeInputs.length > 0 ? nativeInputs : textareas;
      if (inputs.length === 1) inputs[0].focus();
      return {
        editorCount: editors.length,
        inputCount: inputs.length,
        active: inputs.length === 1 && document.activeElement === inputs[0],
        documentFocused: document.hasFocus(),
        kind: nativeInputs.length > 0 ? "native-edit-context" : "textarea",
        editorFocused:
          inputs.length === 1 &&
          inputs[0].closest(".monaco-editor")?.classList.contains("focused") === true,
      };
    })()`);
    requireCondition(
      active?.editorCount === 1 && active.inputCount === 1 &&
        active.active === true && active.documentFocused === true &&
        active.kind === "textarea" && active.editorFocused === true,
      `CDP did not focus the exact visible editor input: ${JSON.stringify(active)}`,
    );
  }

  async pressEnter() {
    await this.call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
      text: "\r",
      unmodifiedText: "\r",
    });
    await this.call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
    });
  }

  async acceptScopeCommand() {
    const label = "Developer: Inspect Editor Tokens and Scopes";
    await waitFor("scope inspector command", () => this.evaluate(`(() => {
      const label = ${JSON.stringify(label)};
      const visible = node => {
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        return node.isConnected && rect.width > 0 && rect.height > 0 &&
          style.visibility !== "hidden" && style.display !== "none";
      };
      const widgets = [...document.querySelectorAll(".quick-input-widget")].filter(visible);
      const rows = widgets.flatMap(widget =>
        [...widget.querySelectorAll(".monaco-list-row")].filter(visible));
      const matches = rows.filter(node =>
        (node.textContent || "").replace(/\\s+/g, " ").trim() === label);
      const inputs = widgets.flatMap(widget =>
        [...widget.querySelectorAll("input")].filter(visible));
      if (matches.length !== 1 || inputs.length !== 1) return undefined;
      inputs[0].focus();
      return document.activeElement === inputs[0] ? true : undefined;
    })()`), 4_000);
    await this.call("Input.dispatchKeyEvent", {
      type: "rawKeyDown",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
      nativeVirtualKeyCode: 36,
    });
    await this.call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
      nativeVirtualKeyCode: 36,
    });
  }

  async scope(file) {
    let last;
    try {
      return await waitFor("scope inspector", () => this.evaluate(`(() => {
      const targetUri = ${JSON.stringify(pathToFileURL(file).href)};
      const visible = node => {
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        return node.isConnected && rect.width > 0 && rect.height > 0 &&
          style.visibility !== "hidden" && style.display !== "none";
      };
      const workbench = document.querySelector(".monaco-workbench");
      const editor = [...document.querySelectorAll(".monaco-editor[data-uri]")]
        .find(node => node.getAttribute("data-uri") === targetUri);
      const allWidgets = [...document.querySelectorAll(".token-inspect-widget")];
      const widgets = allWidgets.filter(visible);
      const widget = widgets.length === 1 ? widgets[0] : undefined;
      const rows = widget ? [...widget.querySelectorAll(".tiw-metadata-table tr")] : [];
      const keyed = rows.map(row => ({
        row,
        key: (row.querySelector(".tiw-metadata-key")?.textContent || "")
          .replace(/\\s+/g, " ").trim().toLowerCase(),
        value: (row.querySelector(".tiw-metadata-value")?.textContent || "")
          .replace(/\\s+/g, " ").trim(),
      }));
      const scopeRows = keyed.filter(row => row.key === "textmate scopes");
      const cells = scopeRows.flatMap(({row}) =>
        [...row.querySelectorAll(".tiw-metadata-value.tiw-metadata-scopes")]);
      const scopes = cells.length === 1
        ? [...cells[0].childNodes].filter(node => node.nodeType === Node.TEXT_NODE)
            .map(node => (node.nodeValue || "").trim()).filter(Boolean)
        : [];
      const themeClasses = node => node
        ? [...node.classList].filter(name => ["vs", "vs-dark", "hc-black", "hc-light"].includes(name))
        : [];
      return {
        totalWidgetCount: allWidgets.length,
        widgetCount: widgets.length,
        widgetParents: allWidgets.map(node => ({
          className: node.parentElement?.className || "",
          visible: visible(node),
          parentVisible: node.parentElement ? visible(node.parentElement) : false,
          parentWidgetId: node.parentElement?.getAttribute("widgetid"),
          parentVisibleAttribute:
            node.parentElement?.getAttribute("monaco-visible-content-widget"),
        })),
        editorCount: editor ? 1 : 0,
        scopes,
        standardTokenTypes: keyed.filter(row => row.key === "standard token type")
          .map(row => row.value),
        semanticRows: keyed.filter(row => row.key === "semantic token type").length,
        workbenchThemeClasses: themeClasses(workbench),
        editorThemeClasses: themeClasses(editor),
      };
    })()`).then((value) => {
        last = value;
        return value?.widgetCount === 1 && value.editorCount === 1 && value.scopes.length > 0
          ? value
          : undefined;
      }), 4_000);
    } catch (error) {
      throw new Error(`${error.message}; scopeState=${JSON.stringify(last)}`);
    }
  }
}

function launchCode({
  label,
  root,
  install,
  workspace,
  driverDevelopment = false,
  trusted = true,
}) {
  const hostRoot = join(root, `host-${label}`);
  const home = join(hostRoot, "home");
  const temp = join(hostRoot, "tmp");
  const userData = join(hostRoot, "user-data");
  mkdirSync(home, { recursive: true, mode: 0o700 });
  mkdirSync(temp, { recursive: true, mode: 0o700 });
  mkdirSync(join(userData, "User"), { recursive: true, mode: 0o700 });
  writeFileSync(
    join(userData, "User", "settings.json"),
    `${JSON.stringify({
      "security.workspace.trust.enabled": !trusted,
      "telemetry.telemetryLevel": "off",
      "update.mode": "none",
      "extensions.autoCheckUpdates": false,
      "extensions.autoUpdate": false,
      "workbench.enableExperiments": false,
      "editor.editContext": false,
    })}\n`,
    { mode: 0o600, flag: "wx" },
  );
  const channel = new DriverChannel(hostRoot);
  const args = [
    "--user-data-dir",
    userData,
    "--extensions-dir",
    install.extensions,
    "--remote-debugging-pipe",
    "--disable-telemetry",
    "--disable-updates",
    "--new-window",
    workspace,
    ...(driverDevelopment
      ? [
          `--extensionDevelopmentPath=${DRIVER}`,
        ]
      : []),
  ];
  recordCommand({
    kind: "code",
    label,
    command: CODE,
    args: [...args],
    cwd: REPO,
    env: {
      HOME: home,
      TMPDIR: temp,
      MARROW_VSQ_CONTROL_PATH: channel.control,
      MARROW_VSQ_EVIDENCE_PATH: channel.evidence,
    },
    timeoutMs: HOST_TIMEOUT_MS,
  });
  const child = spawn(CODE, args, {
    cwd: REPO,
    detached: true,
    env: {
      ...process.env,
      HOME: home,
      TMPDIR: temp,
      MARROW_VSQ_CONTROL_PATH: channel.control,
      MARROW_VSQ_EVIDENCE_PATH: channel.evidence,
    },
    stdio: ["ignore", "pipe", "pipe", "pipe", "pipe"],
  });
  const stdout = [];
  const stderr = [];
  const stdoutPath = join(hostRoot, "code.stdout");
  const stderrPath = join(hostRoot, "code.stderr");
  writeFileSync(stdoutPath, Buffer.alloc(0), { mode: 0o600, flag: "wx" });
  writeFileSync(stderrPath, Buffer.alloc(0), { mode: 0o600, flag: "wx" });
  const capture = (chunks, path, chunk) => {
    const total = chunks.reduce((sum, value) => sum + value.length, 0);
    requireCondition(total + chunk.length <= MAX_OUTPUT, `${basename(path)} exceeded 4 MiB`);
    chunks.push(chunk);
    writeFileSync(path, chunk, { flag: "a" });
  };
  child.stdout.on("data", (chunk) => capture(stdout, stdoutPath, chunk));
  child.stderr.on("data", (chunk) => capture(stderr, stderrPath, chunk));
  return {
    label,
    root: hostRoot,
    child,
    channel,
    cdp: new CdpPipe(child),
    stdout,
    stderr,
    stdoutPath,
    stderrPath,
    known: new Set([child.pid]),
  };
}

async function closeHost(host) {
  const knownSurvivors = () => processSnapshot().filter((row) => host.known.has(row.pid));
  const signalGroup = (signal) => {
    try {
      process.kill(-host.child.pid, signal);
    } catch (error) {
      if (error?.code !== "ESRCH") throw error;
    }
  };
  try {
    await host.channel.send("window.quit", {}, 5_000).catch(() => undefined);
    await waitFor("Code graceful close", () => knownSurvivors().length === 0, 5_000)
      .catch(() => undefined);
    if (knownSurvivors().length !== 0) {
      signalGroup("SIGTERM");
      await waitFor("Code TERM close", () => knownSurvivors().length === 0, 2_000)
        .catch(() => undefined);
    }
    if (knownSurvivors().length !== 0) {
      signalGroup("SIGKILL");
      await waitFor("Code KILL close", () => knownSurvivors().length === 0, 2_000);
    }
  } finally {
    const survivors = knownSurvivors();
    requireCondition(survivors.length === 0,
      `known child survivor after ${host.label}: ${JSON.stringify(survivors)}`);
  }
}

function assertSamples(name, values, count, medianLimit, maxLimit) {
  requireCondition(values.length === count, `${name} sample count differs`);
  requireCondition(values.every((value) => Number.isFinite(value) && value >= 0),
    `${name} samples are invalid`);
  const ordered = [...values].sort((a, b) => a - b);
  const median = ordered[Math.floor(count / 2)];
  const maximum = ordered.at(-1);
  requireCondition(median < medianLimit, `${name} median ${median} >= ${medianLimit}`);
  requireCondition(maximum < maxLimit, `${name} maximum ${maximum} >= ${maxLimit}`);
  return { name, count, median, maximum, ordered };
}

function hostProcessEvidence(host) {
  const rows = descendants(host.child.pid, host.known);
  return {
    rows,
    pids: rows.map(({ pid, ppid, pgid, command }) => ({
      pid,
      ppid,
      pgid,
      commandHash: sha256(Buffer.from(command)),
    })),
    descriptors: observedDescriptors(rows.map(({ pid }) => pid)),
  };
}

function serverLogEvidence(host) {
  const logsRoot = join(host.root, "user-data", "logs");
  if (!existsSync(logsRoot)) return { entries: 0, nonempty: [] };
  const pending = [logsRoot];
  const nonempty = [];
  let entries = 0;
  let retainedBytes = 0;
  while (pending.length > 0) {
    const directory = pending.pop();
    const names = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name, "en-US"));
    for (const entry of names) {
      entries += 1;
      requireCondition(entries <= 4_096, "Code log inventory exceeds 4,096 entries");
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        pending.push(path);
      } else if (entry.isFile() && entry.name.endsWith("Marrow Language Server.log")) {
        const size = statSync(path).size;
        retainedBytes += size;
        requireCondition(retainedBytes <= MAX_EVIDENCE, "Marrow server logs exceed 4 MiB");
        if (size > 0) {
          nonempty.push({
            pathHash: sha256(Buffer.from(path.slice(host.root.length))),
            bytes: size,
            sha256: shaFile(path),
          });
        }
      }
    }
  }
  return { entries, nonempty };
}

function driverEpoch(host) {
  const ready = readEvidence(host.channel.evidence)
    .filter((record) => record.event === "driver.ready");
  requireCondition(ready.length === 1, `${host.label} changed driver epoch`);
  requireCondition(
    Number.isSafeInteger(ready[0].data.processId) && ready[0].data.processId > 1,
    `${host.label} driver epoch lacks a process id`,
  );
  return ready[0].data.processId;
}

async function observeVirtualActivationWindow(host, afterSequence, durationMs = 1_000) {
  const observedServers = new Set();
  const observationDeadline = performance.now() + HOST_TIMEOUT_MS;
  let openedAt;
  while (true) {
    const rows = descendants(host.child.pid, host.known);
    for (const pid of serverPids(rows)) observedServers.add(pid);
    const records = readEvidence(host.channel.evidence).slice(afterSequence);
    if (openedAt === undefined &&
        records.some((record) =>
          record.event === "providers.absent.phase" &&
          record.data.phase === "open.complete")) {
      openedAt = performance.now();
    }
    const now = performance.now();
    if (openedAt !== undefined && now - openedAt >= durationMs) break;
    requireCondition(now < observationDeadline, "virtual activation window did not open");
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  return {
    durationMs,
    observedServers: [...observedServers].sort((left, right) => left - right),
    logs: serverLogEvidence(host),
  };
}

async function runRecoveryHost(label, root, install, fixture, evidence) {
  const host = launchCode({
    label,
    root,
    install,
    workspace: fixture.workspace,
  });
  try {
    await host.channel.waitReady(host.child);
    await host.cdp.attachWorkbench();
    const suite = await host.channel.send("suite.run", { spec: comprehensiveSpec(fixture) });
    const processEvidence = hostProcessEvidence(host);
    requireCondition(
      serverPids(processEvidence.rows).length === 1,
      `${label} lacks exactly one recovery server`,
    );
    evidence.hosts.push({
      label,
      recovery: true,
      suite,
      pids: processEvidence.pids,
      descriptors: processEvidence.descriptors,
    });
  } finally {
    await closeHost(host);
  }
}

async function runVirtualFileLifecycle(root, install, fixture, evidence) {
  const virtualFile = "marrow-vsq:/graph_report.mw";
  const host = launchCode({
    label: "negative-virtual-file-recovery",
    root,
    install,
    workspace: fixture.workspace,
  });
  try {
    const ready = await host.channel.waitReady(host.child);
    await host.cdp.attachWorkbench();
    const epoch = ready.data.processId;
    requireCondition(driverEpoch(host) === epoch, "virtual journey changed its initial driver epoch");
    await host.channel.send("virtual.put", {
      uri: virtualFile,
      text: fixture.graphSource,
    });
    const afterSequence = readEvidence(host.channel.evidence).length;
    const absencePromise = host.channel.send(
      "providers.absent",
      {
        ...absenceArgs(
        { uri: virtualFile },
        fixture,
        true,
        "marrow",
        ),
        activateTarget: true,
      },
    );
    const windowPromise = observeVirtualActivationWindow(host, afterSequence);
    const [absence, activationWindow] = await Promise.all([absencePromise, windowPromise]);
    assertProviderAbsence(absence, "virtual");
    requireCondition(driverEpoch(host) === epoch, "virtual open changed the driver epoch");
    requireCondition(
      activationWindow.observedServers.length === 0,
      `virtual document started Marrow server(s): ${activationWindow.observedServers.join(",")}`,
    );
    requireCondition(
      activationWindow.logs.nonempty.length === 0,
      `virtual document emitted Marrow server log(s): ${JSON.stringify(activationWindow.logs.nonempty)}`,
    );

    const suite = await host.channel.send("suite.run", { spec: comprehensiveSpec(fixture) });
    requireCondition(driverEpoch(host) === epoch, "file recovery changed the driver epoch");
    const processEvidence = hostProcessEvidence(host);
    requireCondition(
      serverPids(processEvidence.rows).length === 1,
      "file open did not recover exactly one Marrow server",
    );
    evidence.hosts.push({
      label: "negative-virtual-file-recovery",
      sameDriverEpoch: epoch,
      virtual: { absence, activationWindow },
      fileRecovery: suite,
      pids: processEvidence.pids,
      descriptors: processEvidence.descriptors,
    });
  } finally {
    await closeHost(host);
  }
}

async function runNegativeRecoveryMatrix(root, install, fixture, evidence) {
  const journeys = [
    {
      label: "untrusted",
      workspace: fixture.workspace,
      trusted: false,
      expectedTargetActive: false,
      expectedLanguageId: "plaintext",
      file: { path: fixture.graphPath },
    },
    {
      label: "multi-root",
      workspace: fixture.multiWorkspace,
      trusted: true,
      expectedTargetActive: false,
      expectedLanguageId: "marrow",
      file: { path: fixture.graphPath },
    },
  ];
  for (const journey of journeys) {
    const host = launchCode({
      label: `negative-${journey.label}`,
      root,
      install,
      workspace: journey.workspace,
      trusted: journey.trusted,
    });
    try {
      await host.channel.waitReady(host.child);
      await host.cdp.attachWorkbench();
      const state = await host.channel.send("state", { targetExtensionId: TARGET_ID });
      if (journey.label === "untrusted") {
        requireCondition(state.trusted === false, "untrusted host unexpectedly trusted its workspace");
      }
      const absence = await host.channel.send(
        "providers.absent",
        absenceArgs(
          journey.file,
          fixture,
          journey.expectedTargetActive,
          journey.expectedLanguageId,
        ),
      );
      assertProviderAbsence(absence, journey.label);
      const processEvidence = hostProcessEvidence(host);
      requireCondition(
        serverPids(processEvidence.rows).length === 0,
        `${journey.label} started a Marrow server`,
      );
      evidence.hosts.push({
        label: `negative-${journey.label}`,
        negative: true,
        state,
        absence,
        pids: processEvidence.pids,
        descriptors: processEvidence.descriptors,
      });
    } finally {
      await closeHost(host);
    }
    await runRecoveryHost(
      `recovery-after-${journey.label}`,
      root,
      install,
      fixture,
      evidence,
    );
  }
  await runVirtualFileLifecycle(root, install, fixture, evidence);
}

async function runColdSamples(root, install, fixture, evidence) {
  const activation = [];
  const firstDiagnostics = [];
  for (let index = 0; index < 5; index += 1) {
    const label = `cold-${String(index + 1).padStart(2, "0")}`;
    const host = launchCode({
      label,
      root,
      install,
      workspace: fixture.workspace,
    });
    try {
      await host.channel.waitReady(host.child);
      await host.cdp.attachWorkbench();
      const sample = await host.channel.send("suite.run", { spec: coldSpec(fixture) });
      activation.push(sample.activationMs);
      firstDiagnostics.push(sample.firstDiagnosticsMs);
      const processEvidence = hostProcessEvidence(host);
      requireCondition(
        serverPids(processEvidence.rows).length === 1,
        `${label} lacks exactly one server`,
      );
      evidence.hosts.push({
        label,
        coldSample: true,
        suite: sample,
        pids: processEvidence.pids,
        descriptors: processEvidence.descriptors,
      });
    } finally {
      await closeHost(host);
    }
  }
  evidence.samples.activation = assertSamples("activation", activation, 5, 400, 1_000);
  evidence.samples.firstDiagnostics = assertSamples(
    "first diagnostics",
    firstDiagnostics,
    5,
    800,
    2_000,
  );
}

function runSelfTests() {
  const commandLog = [];
  appendCommand(commandLog, {
    kind: "sync",
    command: "/usr/bin/true",
    args: [],
    cwd: "/private/tmp",
    env: {},
    timeoutMs: 1,
  });
  assert.deepEqual(commandLog, [{
    kind: "sync",
    command: "/usr/bin/true",
    args: [],
    cwd: "/private/tmp",
    env: {},
    timeoutMs: 1,
  }]);
  assert.throws(() => appendCommand(
    Array.from({ length: MAX_COMMANDS }, () => ({})),
    {},
  ));
  assert.throws(() => appendCommand([], {
    command: "x".repeat(MAX_COMMAND_RECORD + 1),
  }));
  const recordBase = Buffer.byteLength(JSON.stringify({ command: "" }));
  const recordOfSize = (bytes) => {
    const record = { command: "x".repeat(bytes - recordBase) };
    assert.equal(Buffer.byteLength(JSON.stringify(record)), bytes);
    return record;
  };
  const nearFullCommandLog = [];
  for (let index = 0; index < 15; index += 1) {
    appendCommand(nearFullCommandLog, recordOfSize(MAX_COMMAND_RECORD));
  }
  appendCommand(nearFullCommandLog, recordOfSize(MAX_COMMAND_RECORD - 19));
  assert.equal(
    Buffer.byteLength(JSON.stringify(nearFullCommandLog)),
    MAX_COMMAND_LOG - 2,
  );
  const nearFullCommandLogBeforeRefusal = JSON.stringify(nearFullCommandLog);
  const atLimitCommandLog = JSON.parse(nearFullCommandLogBeforeRefusal);
  appendCommand(atLimitCommandLog, 0);
  assert.equal(
    Buffer.byteLength(JSON.stringify(atLimitCommandLog)),
    MAX_COMMAND_LOG,
  );
  assert.equal(
    Buffer.byteLength(JSON.stringify([...nearFullCommandLog, {}])),
    MAX_COMMAND_LOG + 1,
  );
  assert.throws(() => appendCommand(nearFullCommandLog, {}));
  assert.equal(JSON.stringify(nearFullCommandLog), nearFullCommandLogBeforeRefusal);
  const gateSource = readFileSync(new URL(import.meta.url), "utf8");
  assertCommandRecorderSource(gateSource);
  const runStart = gateSource.indexOf("\nfunction run(") + 1;
  const launchStart = gateSource.indexOf("\nfunction launchCode(") + 1;
  assert.throws(() => assertCommandRecorderSource(
    `${gateSource.slice(0, runStart)}${
      gateSource.slice(runStart, launchStart).replace(
        "recordCommand({",
        "recordCommandMissing({",
      )
    }${gateSource.slice(launchStart)}`,
  ));
  assert.throws(() => assertCommandRecorderSource(
    `${gateSource.slice(0, launchStart)}${
      gateSource.slice(launchStart).replace(
        "recordCommand({",
        "recordCommandMissing({",
      )
    }`,
  ));
  assert.throws(() => assertSamples("x", [], 5, 1, 2));
  assert.throws(() => assertSamples("x", [0, 0, 0, 0, 2], 5, 1, 2));
  assert.throws(() => assertSamples("x", [0, 0, 1, 1, 1], 5, 1, 2));
  const cold = assertSamples("cold", [399, 1, 2, 3, 4], 5, 400, 1_000);
  assert.equal(cold.median, 3);
  const ready = assertSamples("ready", [11, 1, 10, 2, 9, 3, 8, 4, 7, 5, 6], 11, 50, 200);
  assert.equal(ready.median, 6);
  for (const [name, expected] of Object.entries(THEME_FILES)) {
    assert.equal(shaFile(join(THEME_ROOT, name)), expected);
  }
  assert.equal(SCOPE_ROWS.length, 9);
  assert.equal(THEMES.length, 3);
  const extensionSource = readFileSync(EXTENSION_SOURCE, "utf8");
  assertExtensionLifecycleSource(extensionSource);
  const sourceMutations = [
    extensionSource.replace(
      `if (!vscode.workspace.textDocuments.some(isFileMarrowDocument)) {
    return;
  }`,
      "",
    ),
    extensionSource.replace('document.uri.scheme === "file"', 'document.uri.scheme === "untitled"'),
    extensionSource.replace("vscode.workspace.onDidOpenTextDocument", "vscode.workspace.onDidCloseTextDocument"),
    extensionSource.replace(
      "vscode.workspace.onDidOpenTextDocument",
      "vscode.workspace.onDidOpenTextDocument",
    ) + "\nvscode.workspace.onDidOpenTextDocument;\n",
    extensionSource.replace(
      'documentSelector: [{ language: "marrow", scheme: "file" }],',
      'documentSelector: [{ language: "marrow" }],',
    ),
    extensionSource.replace(
      "context.subscriptions.push(\n    vscode.workspace.onDidOpenTextDocument",
      "vscode.workspace.onDidOpenTextDocument",
    ),
    `${extensionSource}\ndocument.getText();\n`,
  ];
  for (const mutation of sourceMutations) {
    assert.throws(() => assertExtensionLifecycleSource(mutation));
  }
  console.log("real-host self-tests: PASS");
}

async function runHostGate() {
  const { canonical, structural } = preflight();
  const root = realpathSync(mkdtempSync("/private/tmp/marrow-vsq-a1-"));
  const evidenceRoot = realpathSync(mkdtempSync("/private/tmp/marrow-vsq-a1-evidence-"));
  let retain = true;
  const evidence = {
    schema: 1,
    root,
    evidenceRoot,
    candidate: EXPECTED_HEAD,
    canonicalServerSha256: EXPECTED_SERVER,
    commands: [],
    hosts: [],
    scopes: [],
    samples: {},
    structural,
    interactivePending: {
      physicalTyping:
        "Code 1.130 background CDP/command input did not mutate the focused textarea; retained red evidence",
      tokenInspector:
        "Code 1.130 built-in inspector command produced no observable widget; retained red evidence",
    },
  };
  activeCommandLog = evidence.commands;
  try {
    const primary = copyStage("primary", root, canonical);
    const reproduction = copyStage("reproduction", root, canonical);
    const driver = buildDriver(root, join(primary.stage, "node_modules/.bin/vsce"));
    const primaryInstall = install("primary", root, [primary.vsix, driver.vsix]);
    const reproductionInstall = install("reproduction", root, [reproduction.vsix, driver.vsix]);
    const identityEvidence = join(evidenceRoot, "artifact-identity.json");
    run(process.execPath, [
      join(HERE, "verify-vsix.mjs"),
      "--repo", REPO,
      "--expected-head", EXPECTED_HEAD,
      "--target-dir", TARGET,
      "--primary-stage", primary.stage,
      "--primary-vsix", primary.vsix,
      "--primary-extensions-dir", primaryInstall.extensions,
      "--reproduction-stage", reproduction.stage,
      "--reproduction-vsix", reproduction.vsix,
      "--reproduction-extensions-dir", reproductionInstall.extensions,
      "--evidence", identityEvidence,
    ]);
    const fixture = prepareWorkspace(root, canonical);
    const spec = comprehensiveSpec(fixture);
    const developmentSpec = {
      targetExtensionId: spec.targetExtensionId,
      activation: spec.activation,
    };

    const edh = launchCode({
      label: "development",
      root,
      install: primaryInstall,
      workspace: fixture.workspace,
      driverDevelopment: true,
    });
    try {
      await edh.channel.waitReady(edh.child);
      const result = await edh.channel.send("suite.run", { spec: developmentSpec });
      const observedRows = descendants(edh.child.pid, edh.known);
      requireCondition(
        serverPids(observedRows).length === 1,
        "development host lacks exactly one server after suite",
      );
      evidence.hosts.push({
        label: "development",
        suite: result,
        pids: observedRows.map(({ pid, ppid, pgid, command }) => ({
          pid, ppid, pgid, commandHash: sha256(Buffer.from(command)),
        })),
        descriptors: observedDescriptors(observedRows.map(({ pid }) => pid)),
      });
    } finally {
      await closeHost(edh);
    }

    const ordinary = launchCode({
      label: "ordinary",
      root,
      install: reproductionInstall,
      workspace: fixture.workspace,
    });
    try {
      await ordinary.channel.waitReady(ordinary.child);
      await ordinary.cdp.attachWorkbench();
      const suite = await ordinary.channel.send("suite.run", { spec });
      let rows = descendants(ordinary.child.pid, ordinary.known);
      let servers = serverPids(rows);
      requireCondition(servers.length === 1, "ordinary host lacks exactly one server");

      const retainedReady = { format: [], completion: [], signature: [] };
      for (let index = 0; index < 14; index += 1) {
        const formatSample = await ordinary.channel.send("suite.run", {
          spec: readyTimingSpec(fixture, "format"),
        });
        const completionSample = await ordinary.channel.send("suite.run", {
          spec: readyTimingSpec(fixture, "completion"),
        });
        const signatureSample = await ordinary.channel.send("suite.run", {
          spec: readyTimingSpec(fixture, "signature"),
        });
        if (index >= 3) {
          retainedReady.format.push(formatSample.formatProviderMs);
          retainedReady.completion.push(completionSample.completionMs);
          retainedReady.signature.push(signatureSample.signatureMs);
        }
      }
      evidence.samples.format = assertSamples(
        "format",
        retainedReady.format,
        11,
        50,
        200,
      );
      evidence.samples.completion = assertSamples(
        "completion",
        retainedReady.completion,
        11,
        50,
        200,
      );
      evidence.samples.signature = assertSamples(
        "signature",
        retainedReady.signature,
        11,
        50,
        200,
      );
      for (const [theme, kind, cssClass] of THEMES) {
        for (const [source, position, lexeme, scopes, token] of SCOPE_ROWS) {
          const file = fixture.scope[source];
          const prepared = await ordinary.channel.send("scope.prepare", {
            theme,
            file: { path: file },
            documentHash: shaFile(file),
            position,
            lexeme,
          });
          assert.equal(prepared.themeKind, kind);
          assert.equal(prepared.semanticLegendAbsent, true);
          assert.equal(prepared.semanticTokensAbsent, true);
          await ordinary.channel.send("scope.finish", {
            inspectionId: prepared.inspectionId,
          });
          evidence.scopes.push({
            theme,
            themeKind: kind,
            expectedCssClass: cssClass,
            source,
            position,
            lexeme,
            structurallyPresentScopes: scopes,
            expectedStandardTokenType: token,
            semanticOverlayAbsent: true,
            inspectorAutomated: false,
          });
        }
      }
      const oldServer = servers[0];
      await ordinary.channel.send("server.restart");
      const newServer = await waitFor("old server retirement", () => {
        rows = descendants(ordinary.child.pid, ordinary.known);
        servers = serverPids(rows);
        return !servers.includes(oldServer) && servers.length === 1 ? servers[0] : undefined;
      }, 8_000);
      rows = descendants(ordinary.child.pid, ordinary.known);
      evidence.hosts.push({
        label: "ordinary",
        suite,
        restart: { oldServer, newServer },
        pids: rows.map(({ pid, ppid, pgid, command }) => ({
          pid, ppid, pgid, commandHash: sha256(Buffer.from(command)),
        })),
        descriptors: observedDescriptors(rows.map(({ pid }) => pid)),
      });
    } finally {
      await closeHost(ordinary);
    }
    await runNegativeRecoveryMatrix(root, reproductionInstall, fixture, evidence);
    await runColdSamples(root, reproductionInstall, fixture, evidence);

    const finalPath = join(evidenceRoot, "result.json");
    const body = Buffer.from(`${JSON.stringify(evidence, null, 2)}\n`);
    requireCondition(body.length <= MAX_EVIDENCE, "host result exceeds 4 MiB");
    writeFileSync(finalPath, body, { mode: 0o600, flag: "wx" });
    console.log(JSON.stringify({
      status: "PROVISIONAL_CLEAN",
      root,
      evidenceRoot,
      evidence: finalPath,
      evidenceSha256: sha256(body),
      primaryVsix: primary.vsix,
      primaryVsixSha256: shaFile(primary.vsix),
      reproductionVsix: reproduction.vsix,
      reproductionVsixSha256: shaFile(reproduction.vsix),
      driverVsix: driver.vsix,
      driverVsixSha256: shaFile(driver.vsix),
    }));
    retain = true;
  } catch (error) {
    const failure = Buffer.from(`${JSON.stringify({
      status: "RED",
      root,
      error: { name: error.name, message: error.message, stack: error.stack },
    }, null, 2)}\n`);
    writeFileSync(join(evidenceRoot, "failure.json"), failure, { mode: 0o600, flag: "wx" });
    console.error(error.stack ?? error);
    console.error(`retained red root: ${root}`);
    process.exitCode = 1;
  } finally {
    activeCommandLog = undefined;
    if (!retain) {
      rmSync(root, { recursive: true, force: true });
      rmSync(evidenceRoot, { recursive: true, force: true });
    }
  }
}

if (process.argv.length === 3 && process.argv[2] === "--self-test") {
  runSelfTests();
} else if (process.argv.length === 3 && process.argv[2] === "--run") {
  await runHostGate();
} else {
  console.error("usage: node gate/real-host.mjs --self-test|--run");
  process.exitCode = 2;
}
