import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const originalHash = "9d49ea9912f652bbdf749ddcd7e15526981c81355b3ff3c7bfb4277c405308d2";
const patchedHash = "bd9f017b75003beb5979411da329219eaf9b3204473a9ed9a1860908f5d0a7dd";
const licenseHash = "ec9ee83580841e8eb687aca9867f221503809ba6426c7f876ede17d91b9fcfd0";
const hash = bytes => createHash("sha256").update(bytes).digest("hex");

function inspect(root) {
  const dependency = join(root, "node_modules/vscode-languageclient");
  const manifest = JSON.parse(readFileSync(join(dependency, "package.json"), "utf8"));
  if (manifest.version !== "9.0.1" || manifest.license !== "MIT") {
    throw new Error("Unreviewed vscode-languageclient version or license");
  }
  if (hash(readFileSync(join(dependency, "License.txt"))) !== licenseHash) {
    throw new Error("vscode-languageclient MIT notice changed or missing");
  }
  const file = join(dependency, "lib/common/client.js");
  const source = readFileSync(file, "utf8");
  const digest = hash(source);
  if (digest !== originalHash && digest !== patchedHash) {
    throw new Error("Unreviewed vscode-languageclient cleanup content");
  }
  return { file, source, digest };
}

export function verifyClientCleanup(root) {
  if (inspect(root).digest !== patchedHash) {
    throw new Error("vscode-languageclient cleanup patch is missing");
  }
}

export function patchClientCleanup(root) {
  const { file, source, digest } = inspect(root);
  if (digest === patchedHash) return "already patched";
  // Cleanup must cancel queued edits before clearing document features.
  // Remove this patch when a reviewed upstream version passes the same checks.
  const patched = source.replace("    cleanUp(mode) {\n",
    "    cleanUp(mode) {\n        this._pendingChangeDelayer.cancel();\n");
  if (hash(patched) !== patchedHash) throw new Error("Unexpected cleanup patch result");
  writeFileSync(file, patched);
  return "patched";
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 2) throw new Error("Usage: node gate/client-cleanup.mjs");
  console.log(`vscode-languageclient cleanup: ${patchClientCleanup(join(dirname(fileURLToPath(import.meta.url)), ".."))}`);
}
