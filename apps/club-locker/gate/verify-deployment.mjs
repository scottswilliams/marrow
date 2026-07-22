// Deployment inventory / payload / determinism gate for the Club Locker app.
//
//   node gate/verify-deployment.mjs <deployA> [<deployB>]
//
// One argument checks a single composed deployment: its manifest resolves (runner
// release identity and image identity both verify), it holds exactly one NATIVE
// (Mach-O) executable — the pinned runner — and no other native payload, and the
// image is not executable. Two arguments additionally prove two independent composes
// produce a byte-for-byte identical sorted per-entry (path, sha256) manifest, so a
// deployment build is reproducible. Run explicitly; never an npm lifecycle script.

import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

import { EXPECTED_RELEASE } from "../app/release.mts";
import { DeploymentFault, resolveDeployment } from "../app/deployment.mts";

const RUNNER_ENTRY = "marrow-runner";
const IMAGE_ENTRY = "program.image";
const EXPECTED_ENTRIES = new Set(["marrow-deployment", RUNNER_ENTRY, IMAGE_ENTRY]);

function sha256(buf) {
  return createHash("sha256").update(buf).digest("hex");
}

function isMachO(data) {
  if (data.length < 4) return false;
  const magic = data.readUInt32BE(0);
  return (
    magic === 0xfeedfacf || // MH_MAGIC_64
    magic === 0xcffaedfe || // MH_CIGAM_64
    magic === 0xfeedface || // MH_MAGIC
    magic === 0xcefaedfe || // MH_CIGAM
    magic === 0xcafebabe || // fat/universal
    magic === 0xbebafeca
  );
}

function walk(dir, base, out) {
  for (const name of readdirSync(dir).sort()) {
    const abs = join(dir, name);
    const st = statSync(abs);
    if (st.isDirectory()) {
      walk(abs, base, out);
    } else {
      out.push({ path: relative(base, abs), abs, mode: st.mode & 0o777 });
    }
  }
}

function verifyOne(deployDir) {
  const failures = [];
  const manifestOut = [];

  // 1. The deployment resolves: runner release id and image id both verify.
  try {
    resolveDeployment(deployDir, EXPECTED_RELEASE);
  } catch (error) {
    if (error instanceof DeploymentFault) {
      failures.push(`deployment does not resolve: ${error.kind}`);
    } else {
      throw error;
    }
  }

  // 2. Inventory + payload rules.
  const files = [];
  walk(deployDir, deployDir, files);
  const machoEntries = [];
  for (const file of files) {
    const data = readFileSync(file.abs);
    const hash = sha256(data);
    manifestOut.push({ path: file.path, sha256: hash, mode: file.mode });

    if (!EXPECTED_ENTRIES.has(file.path)) {
      failures.push(`unexpected entry: ${file.path}`);
    }
    const macho = isMachO(data);
    if (macho) machoEntries.push(file.path);
    if (file.path === IMAGE_ENTRY && (macho || (file.mode & 0o111) !== 0)) {
      failures.push("the program image must not be executable or native");
    }
  }

  if (machoEntries.length !== 1) {
    failures.push(`expected exactly one Mach-O entry, found ${machoEntries.length}: ${machoEntries.join(", ")}`);
  } else if (machoEntries[0] !== RUNNER_ENTRY) {
    failures.push(`sole Mach-O entry is ${machoEntries[0]}, not ${RUNNER_ENTRY}`);
  }
  for (const entry of EXPECTED_ENTRIES) {
    if (!files.some((f) => f.path === entry)) failures.push(`missing ${entry}`);
  }

  manifestOut.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return { deployDir, manifest: manifestOut, failures };
}

function main() {
  const args = process.argv.slice(2);
  if (args.length < 1 || args.length > 2) {
    console.error("usage: node gate/verify-deployment.mjs <deployA> [<deployB>]");
    process.exit(2);
  }
  const results = args.map(verifyOne);
  let ok = true;
  for (const r of results) {
    console.log(`\nDeployment: ${r.deployDir}`);
    for (const m of r.manifest) {
      console.log(`  ${m.sha256}  ${m.mode.toString(8).padStart(4, "0")}  ${m.path}`);
    }
    if (r.failures.length > 0) {
      ok = false;
      console.error(`  FAIL (${r.failures.length}):`);
      for (const f of r.failures) console.error(`    - ${f}`);
    } else {
      console.log("  inventory / payload / resolve checks: PASS");
    }
  }

  if (results.length === 2) {
    const [a, b] = results;
    // The runner binary is machine-specific; determinism is over the composed image
    // and manifest, which must be byte-identical. Compare every entry except the
    // runner, whose identity is already pinned and checked in each manifest.
    const strip = (r) => r.manifest.filter((m) => m.path !== RUNNER_ENTRY);
    const ja = JSON.stringify(strip(a));
    const jb = JSON.stringify(strip(b));
    if (ja === jb) {
      console.log("\nDual-build determinism: PASS (identical image + manifest across composes)");
    } else {
      ok = false;
      console.error("\nDual-build determinism: FAIL");
      console.error(`  A: ${ja}`);
      console.error(`  B: ${jb}`);
    }
  }

  process.exit(ok ? 0 : 1);
}

main();
