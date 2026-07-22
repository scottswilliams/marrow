// One-command developer setup: client generation, the image's conscious
// ceiling acceptance (interactive), deployment composition, build, and the
// deployment gate — the same pipeline as the README's manual steps, in order.
//
// The authority law is unchanged: the image's demand is printed and the owner
// must accept the ceiling before any image is composed. This script only
// removes the clerical retyping (the id is captured and passed back verbatim
// after an explicit yes). CI/scripted flows keep the manual --accept-ceiling
// form; this wrapper is interactive-only and refuses to run without a TTY.

import { execFileSync, spawnSync } from "node:child_process";
import { closeSync, existsSync, mkdtempSync, openSync, readSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const APP = dirname(dirname(fileURLToPath(import.meta.url)));

function fail(message) {
  console.error(`setup: ${message}`);
  process.exit(1);
}

function which(name) {
  const found = spawnSync("/usr/bin/which", [name], { encoding: "utf8" });
  return found.status === 0 ? found.stdout.trim() : null;
}

// Read one line from the controlling terminal itself. Immune to whatever state
// earlier inherited-stdio children left process.stdin in (the readline-based
// prompt self-answered under `npm run`).
function askTty(prompt) {
  process.stdout.write(prompt);
  const tty = openSync("/dev/tty", "r");
  try {
    const buf = Buffer.alloc(256);
    let line = "";
    for (;;) {
      const n = readSync(tty, buf, 0, buf.length, null);
      if (n <= 0) return line;
      line += buf.toString("utf8", 0, n);
      const nl = line.indexOf("\n");
      if (nl >= 0) return line.slice(0, nl);
    }
  } finally {
    closeSync(tty);
  }
}

if (!process.stdin.isTTY) {
  fail("interactive only (the ceiling acceptance needs a human); use the README's manual steps in scripts");
}

const marrow = process.env.MARROW ?? which("marrow");
if (!marrow) fail("no `marrow` on PATH (set MARROW=/path/to/marrow)");
const runner = process.env.MARROW_RUNNER ?? join(dirname(marrow), "marrow-runner");
if (!existsSync(runner)) fail(`no marrow-runner beside marrow (looked at ${runner}; set MARROW_RUNNER)`);

console.log("1/5 generating the TypeScript client");
execFileSync(marrow, ["client", "typescript", "--out", "gen"], { cwd: APP, stdio: "inherit" });

console.log("\n2/5 reviewing the image's durable authority demand");
const scratch = mkdtempSync(join(tmpdir(), "cl-image-"));
const review = spawnSync(marrow, ["image", "--out", scratch], { cwd: APP, encoding: "utf8" });
const reviewText = `${review.stdout}${review.stderr}`;
const idMatch = reviewText.match(/--accept-ceiling ([0-9a-f]{64})/);
if (!idMatch) {
  rmSync(scratch, { recursive: true, force: true });
  console.error(reviewText);
  fail("could not obtain the ceiling id from `marrow image` (output above)");
}
const ceiling = idMatch[1];
const demand = reviewText
  .split("\n")
  .filter((line) => line.includes(" reads ") || line.includes(" writes "))
  .join("\n");
console.log(demand);
console.log(`\nceiling id: ${ceiling}`);

let answer = "";
try {
  answer = askTty("accept this deployment ceiling? [y/N] ").trim().toLowerCase();
} catch {
  answer = ""; // no controlling terminal = not accepted
}
if (answer !== "y" && answer !== "yes") {
  rmSync(scratch, { recursive: true, force: true });
  fail("ceiling not accepted; nothing composed");
}
rmSync(scratch, { recursive: true, force: true });

console.log("\n3/5 composing the deployment (runner + verified image + manifest)");
execFileSync(
  "node",
  ["tools/compose-deployment.mjs", "--marrow", marrow, "--runner", runner, "--accept-ceiling", ceiling],
  { cwd: APP, stdio: "inherit" },
);

console.log("\n4/5 building the app");
execFileSync("npx", ["tsc", "-p", "."], { cwd: APP, stdio: "inherit" });
execFileSync("node", ["tools/stage.mjs"], { cwd: APP, stdio: "inherit" });

console.log("\n5/5 verifying the deployment");
execFileSync("node", ["gate/verify-deployment.mjs", "deploy"], { cwd: APP, stdio: "inherit" });

if (!existsSync(join(APP, "node_modules", "electron", "dist"))) {
  console.log("\nfetching the Electron binary (first run only)");
  execFileSync("node", [join("node_modules", "electron", "install.js")], { cwd: APP, stdio: "inherit" });
}

console.log("\nready — launch with: npm start");
