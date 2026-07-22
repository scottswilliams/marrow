// The Club Locker automatable probe suite.
//
// Every clause here runs under plain Node against a REAL composed deployment (a real
// runner binary, the verified image, real Unix-socket sessions) — the same way the
// shipped G03 workshop journey runs. It proves the containment-critical trusted-main
// behavior that does not require a GUI: the frozen security floor and its fail-closed
// predicates, deployment integrity refusal, single-winner first-launch provisioning,
// the renderer's closed call surface, that the staged compiled build launches, and
// that the terminal and TypeScript paths read one store identically.
//
// Clauses that genuinely need a running Chromium/GUI or a truly clean machine cannot
// be honestly automated here; they are printed as PENDING-HUMAN, exactly. The B6
// unaided ceiling-expansion walkthrough is BLOCKED-ON-F03b (its expansion mechanism
// has not landed) and is printed as such.
//
//   node gate/probe.mjs                 # DEPLOY defaults to ./deploy, MARROW from env
//   MARROW=<path> DEPLOY=<dir> node gate/probe.mjs

import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { EXPECTED_RELEASE } from "../app/release.mts";
import {
  CONTENT_SECURITY_POLICY,
  DOMAIN_EXPORTS,
  WEB_PREFERENCES,
  isDomainExport,
  isTrustedSender,
} from "../app/security.mts";
import { DeploymentFault, companionReleaseId, resolveDeployment } from "../app/deployment.mts";
import { CallRefused, ClubLockerApp } from "../app/core.mts";

const APP = dirname(dirname(fileURLToPath(import.meta.url)));
const DEPLOY = process.env.DEPLOY ?? join(APP, "deploy");
const MARROW = process.env.MARROW;

let failures = 0;
let passes = 0;
const scratch = mkdtempSync(join(tmpdir(), "club-probe-"));
process.on("exit", () => rmSync(scratch, { recursive: true, force: true }));

function ok(label, cond, detail) {
  if (cond) {
    passes += 1;
    console.log(`  PASS ${label}`);
  } else {
    failures += 1;
    console.log(`  FAIL ${label}${detail ? ` — ${detail}` : ""}`);
  }
}
function section(name) {
  console.log(`\n== ${name} ==`);
}
async function expectThrows(label, fn, kind) {
  try {
    await fn();
    ok(label, false, "did not throw");
  } catch (error) {
    const gotKind = error instanceof DeploymentFault ? error.kind : error?.name;
    ok(label, kind === undefined || gotKind === kind, `threw ${gotKind}`);
  }
}
function expectResolves(label, fn) {
  try {
    fn();
    ok(label, true);
  } catch (error) {
    ok(label, false, `threw ${error?.name ?? error}`);
  }
}
function tempStore(name) {
  return join(scratch, name);
}
function copyDeploy(name) {
  const dst = join(scratch, name);
  cpSync(DEPLOY, dst, { recursive: true });
  return dst;
}

// ---------------------------------------------------------------------------

function probeSecurityFloor() {
  section("frozen renderer security floor (config probes fail closed)");
  ok("contextIsolation on", WEB_PREFERENCES.contextIsolation === true);
  ok("nodeIntegration off", WEB_PREFERENCES.nodeIntegration === false);
  ok("sandbox on", WEB_PREFERENCES.sandbox === true);
  ok("webSecurity on", WEB_PREFERENCES.webSecurity === true);
  ok("insecure content off", WEB_PREFERENCES.allowRunningInsecureContent === false);
  ok("webview tag off", WEB_PREFERENCES.webviewTag === false);

  ok("CSP denies by default", CONTENT_SECURITY_POLICY.includes("default-src 'none'"));
  ok("CSP script self only", CONTENT_SECURITY_POLICY.includes("script-src 'self'"));
  ok("CSP no unsafe-inline", !CONTENT_SECURITY_POLICY.includes("unsafe-inline"));
  ok("CSP no remote connect", CONTENT_SECURITY_POLICY.includes("connect-src 'none'"));
  ok("CSP no reframing", CONTENT_SECURITY_POLICY.includes("frame-ancestors 'none'"));

  const html = readFileSync(join(APP, "renderer", "index.html"), "utf8");
  const meta = html.match(/http-equiv="Content-Security-Policy"\s+content="([^"]+)"/);
  ok("renderer document carries the exact CSP", meta !== null && meta[1] === CONTENT_SECURITY_POLICY);
  // An inline handler is an `on<event>=` attribute (preceded by whitespace), which the
  // CSP would block anyway; the renderer wires every listener from renderer.mjs.
  ok("renderer has no inline handler", !/\son[a-z]+=/i.test(html));
  ok("renderer loads only an external module script", !/<script(?![^>]*\bsrc=)/i.test(html));

  section("renderer selects a domain call, nothing else");
  ok("a domain read is allowed", isDomainExport("registerMember"));
  for (const forbidden of ["close", "launch", "terminate", "provision", "__proto__", "constructor", ""]) {
    ok(`export \`${forbidden}\` is refused`, !isDomainExport(forbidden));
  }
  const selectors = ["close", "launch", "terminate", "provision"];
  ok(
    "allowlist names no session/runner selector",
    selectors.every((s) => !DOMAIN_EXPORTS.includes(s)),
  );

  section("sender-frame checks fail closed");
  const expected = "file:///Users/anyone/app/out/renderer/index.html";
  ok("no sender frame is refused", !isTrustedSender(undefined, true, expected));
  ok("a subframe is refused", !isTrustedSender(expected, false, expected));
  ok("an http origin is refused", !isTrustedSender("http://evil.example/x", true, expected));
  ok("a different file is refused", !isTrustedSender("file:///tmp/evil.html", true, expected));
  ok("the packaged main frame is trusted", isTrustedSender(expected, true, expected));
  ok(
    "a query/fragment on the trusted url still trusts",
    isTrustedSender(`${expected}?x=1#y`, true, expected),
  );
}

function probeDeploymentIntegrity() {
  section("deployment integrity refuses tampering and ambient discovery");
  expectResolves("a clean deployment resolves", () => resolveDeployment(DEPLOY, EXPECTED_RELEASE));

  // A runner whose bytes changed after the manifest recorded its identity.
  const rt = copyDeploy("tamper-runner");
  const runnerPath = join(rt, "marrow-runner");
  writeFileSync(runnerPath, Buffer.concat([readFileSync(runnerPath), Buffer.from([0])]));
  expectThrowsSync("altered runner is refused", () => resolveDeployment(rt, EXPECTED_RELEASE), "runner_mismatch");

  // An image whose tail changed: its recomputed id no longer matches the embedded digest.
  const it = copyDeploy("tamper-image");
  const imagePath = join(it, "program.image");
  const image = readFileSync(imagePath);
  image[image.length - 1] ^= 0xff;
  writeFileSync(imagePath, image);
  expectThrowsSync("altered image is refused", () => resolveDeployment(it, EXPECTED_RELEASE), "image_mismatch");

  // A manifest naming a traversing runner path (ambient-discovery attempt).
  const trav = copyDeploy("traverse");
  patchManifest(trav, (m) => m.replace("runner marrow-runner", "runner ../evil"));
  expectThrowsSync("a traversing runner name is refused", () => resolveDeployment(trav, EXPECTED_RELEASE), "runner_missing");

  // A manifest naming an absolute runner path.
  const absName = copyDeploy("absolute-runner");
  patchManifest(absName, (m) => m.replace("runner marrow-runner", "runner /bin/sh"));
  expectThrowsSync("an absolute runner name is refused", () => resolveDeployment(absName, EXPECTED_RELEASE), "runner_missing");

  // A runner that is a SYMLINK out of the deployment, even under a plain component
  // name: no-follow resolution refuses it rather than spawning the link target.
  const sym = copyDeploy("symlink-runner");
  const symRunner = join(sym, "marrow-runner");
  const elsewhere = join(scratch, "elsewhere-runner");
  cpSync(join(DEPLOY, "marrow-runner"), elsewhere);
  rmSync(symRunner);
  symlinkSync(elsewhere, symRunner);
  expectThrowsSync("a symlinked runner is refused", () => resolveDeployment(sym, EXPECTED_RELEASE), "runner_missing");

  // A missing manifest.
  const nm = copyDeploy("no-manifest");
  rmSync(join(nm, "marrow-deployment"));
  expectThrowsSync("a missing manifest is refused", () => resolveDeployment(nm, EXPECTED_RELEASE), "manifest_missing");

  // A manifest that is a symlink (no-follow refuses it as not a regular file).
  const sm = copyDeploy("symlink-manifest");
  const smPath = join(sm, "marrow-deployment");
  const smElsewhere = join(scratch, "elsewhere-manifest");
  cpSync(join(DEPLOY, "marrow-deployment"), smElsewhere);
  rmSync(smPath);
  symlinkSync(smElsewhere, smPath);
  expectThrowsSync("a symlinked manifest is refused", () => resolveDeployment(sm, EXPECTED_RELEASE), "manifest_malformed");

  // A release that is not the app's.
  const rel = copyDeploy("release-skew");
  patchManifest(rel, (m) => m.replace(`release ${EXPECTED_RELEASE}`, "release 9.9.9"));
  expectThrowsSync("a release mismatch is refused", () => resolveDeployment(rel, EXPECTED_RELEASE), "release_mismatch");
}

function expectThrowsSync(label, fn, kind) {
  try {
    fn();
    ok(label, false, "did not throw");
  } catch (error) {
    const gotKind = error instanceof DeploymentFault ? error.kind : error?.name;
    ok(label, gotKind === kind, `threw ${gotKind}`);
  }
}
function patchManifest(dir, edit) {
  const path = join(dir, "marrow-deployment");
  writeFileSync(path, edit(readFileSync(path, "utf8")));
}

async function probeConcurrentFirstLaunch() {
  section("concurrent first launches prove one destination owner");
  const dataDir = tempStore("concurrent-data");

  // Two launches race against an absent destination. The single-winner provision
  // lock means exactly one store is created (never two, never a corrupted one); the
  // store is single-writer, so exactly one launch attaches and the other is refused —
  // one destination, one owner, no second store.
  const settled = await Promise.allSettled([
    ClubLockerApp.open(DEPLOY, dataDir, EXPECTED_RELEASE),
    ClubLockerApp.open(DEPLOY, dataDir, EXPECTED_RELEASE),
  ]);
  const fulfilled = settled.filter((s) => s.status === "fulfilled");
  ok("exactly one launch owns the destination", fulfilled.length === 1, `${fulfilled.length} succeeded`);
  ok(
    "exactly one store was provisioned",
    existsSync(join(dataDir, "store")) &&
      readdirSync(dataDir).filter((e) => e === "store").length === 1,
  );
  ok("the provisioning lock was released", !existsSync(join(dataDir, ".provisioning")));

  const winner = fulfilled[0]?.value;
  if (winner) {
    const member = await winner.call("registerMember", ["Grace Hopper", "2024-09-28"]);
    ok("the sole owner reads and writes its store", member === 1n, String(member));
    await winner.close();
  }
}

async function probeRendererCannotSelect() {
  section("the renderer cannot choose identity/executable/store/path/maintenance");
  const app = await ClubLockerApp.open(DEPLOY, tempStore("select-data"), EXPECTED_RELEASE);
  for (const name of ["close", "launch", "terminate", "provision", "__proto__", "notAnExport"]) {
    await expectThrows(`call \`${name}\` is refused`, () => app.call(name, []), "CallRefused");
  }
  const id = await app.call("registerMember", ["Ada Lovelace", "2024-09-27"]);
  ok("a domain call still works", id === 1n, String(id));
  await app.close();
}

async function probeStagedBuildLaunches() {
  section("the staged compiled build launches through the normal path");
  const outCore = join(APP, "out", "app", "core.mjs");
  const outDeploy = join(APP, "out", "app", "deployment.mjs");
  if (!existsSync(outCore) || !existsSync(outDeploy)) {
    ok("staged build present", false, "run `npm run build` first");
    return;
  }
  const core = await import(outCore);
  const dep = await import(outDeploy);
  // The compiled deployment resolver verifies, and the compiled core opens over it.
  dep.resolveDeployment(DEPLOY, EXPECTED_RELEASE);
  const app = await core.ClubLockerApp.open(DEPLOY, tempStore("staged-data"), EXPECTED_RELEASE);
  const id = await app.call("registerMember", ["Katherine Johnson", "2024-09-29"]);
  ok("the compiled core provisions and calls", id === 1n, String(id));
  await app.close();
}

async function probeTerminalMatchesTypescript() {
  section("terminal and TypeScript paths read one store identically");
  if (!MARROW) {
    ok("terminal path checked", false, "set MARROW=<marrow binary> to run this clause");
    return;
  }
  const deployment = resolveDeployment(DEPLOY, EXPECTED_RELEASE);
  // Install the companion manifest beside the terminal so `marrow run --store` will
  // spawn the same release-verified runner the deployment pins.
  const marrowDir = dirname(MARROW);
  const runnerId = companionReleaseId(readFileSync(deployment.runner));
  writeFileSync(
    join(marrowDir, "marrow-companions"),
    `marrow companions v0\nrelease ${EXPECTED_RELEASE}\nrunner marrow-runner ${runnerId}\nend\n`,
  );

  const dataDir = tempStore("identity-data");
  const app = await ClubLockerApp.open(DEPLOY, dataDir, EXPECTED_RELEASE);
  const member = await app.call("registerMember", ["Ada Lovelace", "2024-09-27"]);
  await app.call("registerAsset", ["R-100", "racquets", "Racquet"]);
  await app.call("checkout", [member, 1n, "2024-10-01"]);
  const tsName = await app.call("memberName", [member]);
  const tsLoan = await app.call("assetOnLoanTo", [1n]);
  await app.close();

  const store = join(dataDir, "store");
  const runTerminal = (exp, arg) =>
    execFileSync(MARROW, ["run", exp, "--store", store, "--", String(arg)], { encoding: "utf8" }).trim();
  const termName = runTerminal("clublocker.memberName", 1);
  const termLoan = runTerminal("clublocker.assetOnLoanTo", 1);
  ok("member name matches across paths", termName === tsName, `terminal ${termName} vs ts ${tsName}`);
  ok("loan holder matches across paths", termLoan === String(tsLoan), `terminal ${termLoan} vs ts ${tsLoan}`);
}

function pendingAndBlocked() {
  section("PENDING-HUMAN (need a real GUI/Chromium or a truly clean machine)");
  for (const item of [
    "Real BrowserWindow creation with the frozen webPreferences, and the preload contextBridge exposing `window.club` in a live renderer.",
    "Chromium's own enforcement of the document CSP and the response-header CSP (this suite proves the policy strings and that the document carries them, not Chromium's enforcement).",
    "The `will-navigate` / `setWindowOpenHandler` denials and single-instance focus, observed in a running Chromium.",
    "The sender-frame check firing on a real IpcMainInvokeEvent (this suite proves the pure predicate over hostile inputs).",
    "Clean-machine equivalence: provision + first domain call on a machine with no Rust/DB toolchain. Locally approximated by a pristine data directory and a from-source staged build; a truly clean host is human-run.",
    "Launching the packaged app itself (`npx electron .`): the Electron binary is not fetched under `ignore-scripts`, so a human with Electron installed performs the interactive launch.",
  ]) {
    console.log(`  PENDING-HUMAN: ${item}`);
  }

  section("BLOCKED-ON-F03b");
  console.log(
    "  BLOCKED-ON-F03b: the B6 unaided ceiling-expansion walkthrough (<=10 min / <=3 artifacts) " +
      "cannot exist yet — the deliberate ceiling-EXPANSION mechanism is owned by F03b and has not " +
      "landed. This deployment binds the store to exactly the image's demand-union ceiling and the " +
      "runner refuses a broadened image (the contraction half, shipped in G03); the expansion half " +
      "is owed downstream and is not faked here.",
  );
}

// ---------------------------------------------------------------------------

async function main() {
  console.log(`Club Locker probe — deployment ${DEPLOY}`);
  probeSecurityFloor();
  probeDeploymentIntegrity();
  await probeConcurrentFirstLaunch();
  await probeRendererCannotSelect();
  await probeStagedBuildLaunches();
  await probeTerminalMatchesTypescript();
  pendingAndBlocked();

  console.log(`\n${passes} passed, ${failures} failed`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
