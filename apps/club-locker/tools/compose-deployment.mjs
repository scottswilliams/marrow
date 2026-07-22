// Compose a MarrowDeployment: the pinned runner, the verified image, and the
// manifest that binds their identities and the accepted ceiling.
//
//   node tools/compose-deployment.mjs \
//     --marrow <marrow> --runner <marrow-runner> \
//     --accept-ceiling <id> [--out deploy]
//
// The step emits the verified image through the stock `marrow image` command (which
// itself refuses to write unless the owner accepts the image's own deployment
// ceiling), copies the release runner beside it, recomputes both identities with the
// same constructions the runtime uses, and writes the `marrow-deployment` manifest.
// It refuses to compose against a toolchain whose release is not the one the app
// expects, so a deployment can never pin a mismatched runner. Run explicitly; never
// an npm lifecycle script.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { companionReleaseId, imageIdOf } from "../app/deployment.mts";
import { EXPECTED_RELEASE } from "../app/release.mts";

const APP = dirname(dirname(fileURLToPath(import.meta.url)));

function arg(name) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const marrow = arg("--marrow");
const runner = arg("--runner");
const acceptCeiling = arg("--accept-ceiling");
const out = arg("--out") ?? "deploy";

if (!marrow || !runner || !acceptCeiling) {
  console.error(
    "usage: node tools/compose-deployment.mjs --marrow <marrow> --runner <marrow-runner> --accept-ceiling <id> [--out deploy]",
  );
  process.exit(2);
}

// `resolve` honors an absolute `--out` and roots a relative one at the app.
const deployDir = resolve(APP, out);
mkdirSync(deployDir, { recursive: true });

// 1. The toolchain release must be the one the app was built for.
const version = execFileSync(marrow, ["--version"], { encoding: "utf8" }).trim();
const release = version.replace(/^marrow\s+/, "");
if (release !== EXPECTED_RELEASE) {
  console.error(`compose: toolchain release ${release} != app-expected ${EXPECTED_RELEASE}`);
  process.exit(1);
}

// 2. Emit the verified image through the stock command with the owner's acceptance.
//    `marrow image` writes program.image only if the accepted ceiling matches.
const imageOut = execFileSync(marrow, ["image", "--out", deployDir, "--accept-ceiling", acceptCeiling], {
  cwd: APP,
  encoding: "utf8",
});
const facts = Object.fromEntries(
  imageOut
    .split("\n")
    .map((line) => line.split(" "))
    .filter((parts) => parts.length === 2)
    .map(([k, v]) => [k, v]),
);
const ceilingId = facts.ceiling;
if (ceilingId !== acceptCeiling) {
  console.error("compose: emitted ceiling id does not match the accepted id");
  process.exit(1);
}

// 3. Copy the runner beside the image and recompute both identities the way the
//    runtime will, so the manifest records exactly what resolveDeployment checks.
const runnerName = "marrow-runner";
copyFileSync(runner, join(deployDir, runnerName));
const runnerId = companionReleaseId(readFileSync(join(deployDir, runnerName)));

const imageName = "program.image";
const imageId = imageIdOf(readFileSync(join(deployDir, imageName)));
if (imageId !== facts.image) {
  console.error("compose: recomputed image id disagrees with the emitted fact");
  process.exit(1);
}

// 4. Write the manifest the trusted main verifies against.
const manifest =
  "marrow deployment v0\n" +
  `release ${release}\n` +
  `runner ${runnerName} ${runnerId}\n` +
  `image ${imageName} ${imageId}\n` +
  `ceiling ${ceilingId}\n` +
  "end\n";
writeFileSync(join(deployDir, "marrow-deployment"), manifest);

console.log(`composed deployment at ${out}`);
console.log(`  release  ${release}`);
console.log(`  runner   ${runnerName} ${runnerId}`);
console.log(`  image    ${imageName} ${imageId}`);
console.log(`  ceiling  ${ceilingId}`);
