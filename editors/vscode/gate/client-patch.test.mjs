import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { patchClientCleanup, verifyClientCleanup } from "./client-cleanup.mjs";

const installed = new URL("../node_modules/vscode-languageclient/", import.meta.url);
const patched = readFileSync(new URL("lib/common/client.js", installed), "utf8");
const original = patched.replace("        this._pendingChangeDelayer.cancel();\n", "");
const manifest = readFileSync(new URL("package.json", installed), "utf8");
const license = readFileSync(new URL("License.txt", installed));

test("patch accepts only reviewed bytes and version; verification never repairs", () => {
  verifyClientCleanup(fileURLToPath(new URL("..", import.meta.url)));
  const root = mkdtempSync(join(tmpdir(), "marrow-client-patch-"));
  const dependency = join(root, "node_modules/vscode-languageclient");
  const client = join(dependency, "lib/common/client.js");
  try {
    mkdirSync(dirname(client), { recursive: true });
    writeFileSync(join(dependency, "package.json"), manifest);
    writeFileSync(join(dependency, "License.txt"), license);
    writeFileSync(client, original);
    assert.throws(() => verifyClientCleanup(root));
    assert.equal(readFileSync(client, "utf8"), original);
    assert.equal(patchClientCleanup(root), "patched");
    assert.equal(readFileSync(client, "utf8"), patched);
    assert.equal(patchClientCleanup(root), "already patched");
    verifyClientCleanup(root);
    writeFileSync(client, patched + "\n");
    assert.throws(() => patchClientCleanup(root));
    assert.throws(() => verifyClientCleanup(root));
    assert.equal(readFileSync(client, "utf8"), patched + "\n");
    writeFileSync(client, original);
    writeFileSync(join(dependency, "package.json"), JSON.stringify({ ...JSON.parse(manifest), version: "10.1.1" }));
    assert.throws(() => patchClientCleanup(root));
    assert.equal(readFileSync(client, "utf8"), original);
    writeFileSync(join(dependency, "package.json"), manifest);
    writeFileSync(join(dependency, "License.txt"), "changed");
    assert.throws(() => patchClientCleanup(root));
    assert.equal(readFileSync(client, "utf8"), original);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
