#!/usr/bin/env node
// Explicit-invocation identity gate for two complete, independently staged,
// packaged, and freshly isolated installed VS Code extension chains. This
// wrapper owns no artifact digest and publishes through the shared exclusive
// evidence boundary.

import {
  linkSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  ARTIFACT_FAULT_NAMES,
  IdentityError,
  compareDualBuilds,
  publishEvidence,
  runFaultMatrix,
  sha256,
  verifyArtifactChain,
} from "./artifact-identity.mjs";

const VALUE_FLAGS = new Set([
  "--repo",
  "--expected-head",
  "--target-dir",
  "--primary-stage",
  "--primary-vsix",
  "--primary-extensions-dir",
  "--reproduction-stage",
  "--reproduction-vsix",
  "--reproduction-extensions-dir",
  "--evidence",
]);
const REQUIRED_FLAGS = Object.freeze([
  "--repo",
  "--expected-head",
  "--target-dir",
  "--primary-stage",
  "--primary-vsix",
  "--primary-extensions-dir",
  "--reproduction-stage",
  "--reproduction-vsix",
  "--reproduction-extensions-dir",
  "--evidence",
]);
const SELF_TEST_FLAGS = new Set(["--self-test", "--fault-matrix"]);

function usage() {
  return [
    "usage:",
    "  node gate/verify-vsix.mjs --self-test",
    "  node gate/verify-vsix.mjs \\",
    "    --repo <clean-worktree> --expected-head <40hex> \\",
    "    --target-dir <external-cargo-target> \\",
    "    --primary-stage <external-primary-stage> \\",
    "    --primary-vsix <fresh-primary.vsix> \\",
    "    --primary-extensions-dir <fresh-primary-extensions-dir> \\",
    "    --reproduction-stage <external-reproduction-stage> \\",
    "    --reproduction-vsix <fresh-reproduction.vsix> \\",
    "    --reproduction-extensions-dir <fresh-reproduction-extensions-dir> \\",
    "    --evidence <external-evidence.json>",
  ].join("\n");
}

function parseArgs(argv) {
  if (argv.length === 1 && SELF_TEST_FLAGS.has(argv[0])) {
    return { selfTest: true };
  }
  const values = {};
  for (let index = 0; index < argv.length; index++) {
    const flag = argv[index];
    if (!VALUE_FLAGS.has(flag)) {
      throw new IdentityError("identity.arguments", "cli", flag ?? "", "unknown flag");
    }
    if (Object.hasOwn(values, flag)) {
      throw new IdentityError("identity.arguments", "cli", flag, "duplicate flag");
    }
    const value = argv[++index];
    if (value === undefined || value.startsWith("--")) {
      throw new IdentityError("identity.arguments", "cli", flag, "value required");
    }
    values[flag] = value;
  }
  for (const flag of REQUIRED_FLAGS) {
    if (!Object.hasOwn(values, flag)) {
      throw new IdentityError("identity.arguments", "cli", flag, "required flag absent");
    }
  }
  return {
    selfTest: false,
    repoRoot: values["--repo"],
    expectedHead: values["--expected-head"],
    targetDir: values["--target-dir"],
    primary: {
      stageRoot: values["--primary-stage"],
      vsixPath: values["--primary-vsix"],
      extensionsDir: values["--primary-extensions-dir"],
    },
    reproduction: {
      stageRoot: values["--reproduction-stage"],
      vsixPath: values["--reproduction-vsix"],
      extensionsDir: values["--reproduction-extensions-dir"],
    },
    evidencePath: values["--evidence"],
  };
}

function requireCondition(condition, code, edge, path, detail = "") {
  if (!condition) throw new IdentityError(code, edge, path, detail);
}

function isWithin(path, parent) {
  const rel = relative(parent, path);
  return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel));
}

function canonicalExistingPath(path, kind, edge, label) {
  requireCondition(
    typeof path === "string" && isAbsolute(path),
    "identity.dual_chain",
    edge,
    label,
    "absolute path required",
  );
  let info;
  try {
    info = lstatSync(path);
  } catch (error) {
    throw new IdentityError("identity.dual_chain", edge, label, error.message);
  }
  requireCondition(
    !info.isSymbolicLink(),
    "identity.dual_chain",
    edge,
    label,
    "symlink rejected",
  );
  requireCondition(
    kind === "directory" ? info.isDirectory() : info.isFile(),
    "identity.dual_chain",
    edge,
    label,
    `expected ${kind}`,
  );
  const canonical = realpathSync(path);
  const canonicalInfo = lstatSync(canonical);
  return {
    path: canonical,
    device: canonicalInfo.dev,
    inode: canonicalInfo.ino,
    mode: canonicalInfo.mode & 0o777,
  };
}

function requireDisjointTrees(left, right, path) {
  requireCondition(
    !isWithin(left, right) && !isWithin(right, left),
    "identity.alias",
    "chain-disjoint",
    path,
    `${left} and ${right} overlap`,
  );
}

function requireDistinctFiles(left, right, path) {
  requireCondition(
    left.path !== right.path &&
      (left.device !== right.device || left.inode !== right.inode),
    "identity.alias",
    "chain-disjoint",
    path,
    `${left.path} and ${right.path} are not independent files`,
  );
}

function requireDistinctDirectories(left, right, path) {
  requireDisjointTrees(left.path, right.path, path);
  requireCondition(
    left.device !== right.device || left.inode !== right.inode,
    "identity.alias",
    "chain-disjoint",
    path,
    `${left.path} and ${right.path} are not independent directories`,
  );
}

function canonicalChainPaths(chain, label) {
  requireCondition(
    chain !== null && typeof chain === "object",
    "identity.dual_chain",
    "chain-disjoint",
    label,
    "verified chain required",
  );
  const stage = canonicalExistingPath(
    chain?.stage?.metadata?.root,
    "directory",
    "chain-disjoint",
    `${label}.stage`,
  );
  const vsix = canonicalExistingPath(
    chain?.archive?.path,
    "file",
    "chain-disjoint",
    `${label}.vsix`,
  );
  const extensions = canonicalExistingPath(
    chain?.install?.extensionsDir,
    "directory",
    "chain-disjoint",
    `${label}.extensions`,
  );
  const installed = canonicalExistingPath(
    chain?.install?.installedRoot,
    "directory",
    "chain-disjoint",
    `${label}.installed`,
  );
  const index = canonicalExistingPath(
    chain?.install?.index?.path,
    "file",
    "chain-disjoint",
    `${label}.extensions-index`,
  );
  requireCondition(
    installed.path !== extensions.path && isWithin(installed.path, extensions.path),
    "identity.dual_chain",
    "chain-disjoint",
    `${label}.installed`,
    "installed root is not inside its isolated extensions directory",
  );
  requireCondition(
    isWithin(index.path, extensions.path),
    "identity.dual_chain",
    "chain-disjoint",
    `${label}.extensions-index`,
    "extensions index is not inside its isolated extensions directory",
  );
  requireCondition(
    !isWithin(vsix.path, stage.path) && !isWithin(vsix.path, extensions.path),
    "identity.alias",
    "chain-disjoint",
    `${label}.vsix`,
    "VSIX must be outside its stage and extensions trees",
  );
  return { stage, vsix, extensions, installed, index };
}

function assertDualChainDisjoint(primaryChain, reproductionChain) {
  const primary = canonicalChainPaths(primaryChain, "primary");
  const reproduction = canonicalChainPaths(reproductionChain, "reproduction");

  requireDistinctDirectories(primary.stage, reproduction.stage, "stage roots");
  requireDistinctFiles(primary.vsix, reproduction.vsix, "VSIX paths/inodes");
  requireDistinctDirectories(
    primary.extensions,
    reproduction.extensions,
    "extensions directories",
  );
  requireDistinctDirectories(
    primary.installed,
    reproduction.installed,
    "installed roots/inodes",
  );
  requireDistinctFiles(primary.index, reproduction.index, "extensions index paths/inodes");

  for (const stage of [primary.stage, reproduction.stage]) {
    for (const extensions of [primary.extensions, reproduction.extensions]) {
      requireDisjointTrees(stage.path, extensions.path, "stage/extensions trees");
    }
  }
  for (const vsix of [primary.vsix, reproduction.vsix]) {
    for (const tree of [
      primary.stage,
      reproduction.stage,
      primary.extensions,
      reproduction.extensions,
    ]) {
      requireCondition(
        !isWithin(vsix.path, tree.path),
        "identity.alias",
        "chain-disjoint",
        "VSIX/tree surfaces",
        `${vsix.path} is inside ${tree.path}`,
      );
    }
  }

  return Object.freeze({
    primary,
    reproduction,
  });
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function assertSharedAuthority(primary, reproduction, options) {
  const repo = canonicalExistingPath(options.repoRoot, "directory", "shared-authority", "repo");
  const target = canonicalExistingPath(options.targetDir, "directory", "shared-authority", "target");
  requireDisjointTrees(repo.path, target.path, "repo/target roots");
  for (const [label, chain] of [
    ["primary", primary],
    ["reproduction", reproduction],
  ]) {
    requireCondition(
      chain?.authority?.repoRoot === repo.path && chain?.authority?.head === options.expectedHead,
      "identity.dual_chain",
      "shared-authority",
      label,
      "chain is not bound to the requested repository and HEAD",
    );
  }
  requireCondition(
    stableJson(primary.evidence?.candidate) === stableJson(reproduction.evidence?.candidate) &&
      stableJson(primary.evidence?.canonical) === stableJson(reproduction.evidence?.canonical),
    "identity.dual_chain",
    "shared-authority",
    "candidate/target",
    "verified chains disagree on candidate or canonical target",
  );
  return Object.freeze({ repo, target });
}

function assertArchivesOutsideAuthority(surfaces, authority) {
  for (const vsix of [surfaces.primary.vsix, surfaces.reproduction.vsix]) {
    requireCondition(
      !isWithin(vsix.path, authority.repo.path) && !isWithin(vsix.path, authority.target.path),
      "identity.alias",
      "chain-disjoint",
      "VSIX/authority roots",
      `${vsix.path} is inside the candidate repository or Cargo target`,
    );
  }
}

function verifyBothChains(options) {
  const common = {
    repoRoot: options.repoRoot,
    expectedHead: options.expectedHead,
    targetDir: options.targetDir,
  };
  const primary = verifyArtifactChain({ ...common, ...options.primary });
  const reproduction = verifyArtifactChain({ ...common, ...options.reproduction });
  const authority = assertSharedAuthority(primary, reproduction, options);
  const surfaces = assertDualChainDisjoint(primary, reproduction);
  assertArchivesOutsideAuthority(surfaces, authority);
  const dualBuild = compareDualBuilds(primary, reproduction);
  return { primary, reproduction, authority, surfaces, dualBuild };
}

function publicPathEvidence(paths) {
  return Object.fromEntries(
    Object.entries(paths).map(([name, value]) => [name, {
      path: value.path,
      device: value.device,
      inode: value.inode,
      mode: value.mode,
    }]),
  );
}

function publishVerificationEvidence({ destination, result }) {
  const evidence = {
    schema: "marrow.vscode.artifact-identity.vsq01.v1",
    primary: result.primary.evidence,
    reproduction: result.reproduction.evidence,
    dualBuild: result.dualBuild,
    disjointPaths: {
      primary: publicPathEvidence(result.surfaces.primary),
      reproduction: publicPathEvidence(result.surfaces.reproduction),
    },
  };
  const bytes = Buffer.from(`${JSON.stringify(evidence, null, 2)}\n`, "utf8");
  const forbiddenRoots = [...new Set([
    result.authority.repo.path,
    result.authority.target.path,
    result.surfaces.primary.stage.path,
    result.surfaces.reproduction.stage.path,
    result.surfaces.primary.extensions.path,
    result.surfaces.reproduction.extensions.path,
    result.surfaces.primary.installed.path,
    result.surfaces.reproduction.installed.path,
    realpathSync(dirname(result.surfaces.primary.vsix.path)),
    realpathSync(dirname(result.surfaces.reproduction.vsix.path)),
  ])];
  const receipt = publishEvidence({
    destination,
    bytes,
    forbiddenRoots,
  });
  return { evidence, receipt };
}

function expectIdentity(results, name, callback, expected) {
  let caught;
  try {
    callback();
  } catch (error) {
    caught = error;
  }
  requireCondition(
    caught instanceof IdentityError,
    "identity.wrapper_self_test",
    "self-test",
    name,
    `expected IdentityError, got ${caught ?? "success"}`,
  );
  requireCondition(
    caught.code === expected.code && caught.edge === expected.edge && caught.path === expected.path,
    "identity.wrapper_self_test",
    "self-test",
    name,
    `got ${caught.code}/${caught.edge}/${caught.path}`,
  );
  results.push({ name, code: caught.code, edge: caught.edge, path: caught.path });
}

function completeSelfTestArgs() {
  return REQUIRED_FLAGS.flatMap((flag, index) => [flag, `/self-test/value-${index}`]);
}

function selfTestChain({ repo, stage, vsix, extensions, installed, index, label }) {
  return {
    authority: { repoRoot: repo, head: "a".repeat(40) },
    stage: { metadata: { root: stage } },
    archive: { path: vsix },
    install: {
      extensionsDir: extensions,
      installedRoot: installed,
      index: { path: index },
    },
    evidence: {
      candidate: { head: "a".repeat(40), cargoLock: { sha256: "b".repeat(64) } },
      canonical: [{ path: "server/marrow-lsp", sha256: "c".repeat(64), mode: 0o755, size: 1 }],
      stage: [{ path: "server/marrow-lsp", sha256: "c".repeat(64), mode: 0o755, size: 1 }],
      vsix: { label },
      installed: { label },
    },
  };
}

function runWrapperSelfTests(artifactFaults) {
  const results = [];
  const complete = completeSelfTestArgs();
  const omittedReproductionStage = complete.filter(
    (_, index) => complete[index - (index % 2)] !== "--reproduction-stage",
  );
  expectIdentity(results, "required reproduction chain", () => parseArgs(omittedReproductionStage), {
    code: "identity.arguments",
    edge: "cli",
    path: "--reproduction-stage",
  });
  expectIdentity(results, "unknown CLI flag", () => parseArgs(["--unknown"]), {
    code: "identity.arguments",
    edge: "cli",
    path: "--unknown",
  });
  expectIdentity(results, "duplicate CLI flag", () => parseArgs([...complete, "--repo", "/again"]), {
    code: "identity.arguments",
    edge: "cli",
    path: "--repo",
  });
  expectIdentity(results, "missing CLI value", () => parseArgs(["--repo"]), {
    code: "identity.arguments",
    edge: "cli",
    path: "--repo",
  });

  const publisherFault = artifactFaults.find(({ name }) => name === "VSIX identity");
  requireCondition(
    artifactFaults.map(({ name }) => name).join("\0") === ARTIFACT_FAULT_NAMES.join("\0") &&
      publisherFault?.code === "identity.vsix_identity" &&
      publisherFault?.path === "extension.vsixmanifest/Identity.Publisher",
    "identity.wrapper_self_test",
    "self-test",
    "VSIX Publisher boundary",
    "artifact fault matrix did not exercise the Publisher boundary",
  );
  results.push({
    name: "VSIX Publisher boundary",
    code: publisherFault.code,
    edge: publisherFault.edge,
    path: publisherFault.path,
  });

  const root = realpathSync(mkdtempSync(join(tmpdir(), "marrow-verify-vsix-wrapper-")));
  try {
    const paths = {
      repo: join(root, "repo"),
      target: join(root, "target"),
      primaryStage: join(root, "stage-primary"),
      reproductionStage: join(root, "stage-reproduction"),
      primaryExtensions: join(root, "extensions-primary"),
      reproductionExtensions: join(root, "extensions-reproduction"),
      archives: join(root, "archives"),
      evidenceParent: join(root, "retained"),
    };
    for (const path of Object.values(paths)) mkdirSync(path, { mode: 0o700 });
    const primaryInstalled = join(paths.primaryExtensions, "marrow-project.marrow-0.1.1");
    const reproductionInstalled = join(
      paths.reproductionExtensions,
      "marrow-project.marrow-0.1.1",
    );
    mkdirSync(primaryInstalled, { mode: 0o700 });
    mkdirSync(reproductionInstalled, { mode: 0o700 });
    const primaryVsix = join(paths.archives, "primary.vsix");
    const reproductionVsix = join(paths.archives, "reproduction.vsix");
    const primaryIndex = join(paths.primaryExtensions, "extensions.json");
    const reproductionIndex = join(paths.reproductionExtensions, "extensions.json");
    writeFileSync(primaryVsix, "primary", { mode: 0o600, flag: "wx" });
    writeFileSync(reproductionVsix, "reproduction", { mode: 0o600, flag: "wx" });
    writeFileSync(primaryIndex, "[]", { mode: 0o600, flag: "wx" });
    writeFileSync(reproductionIndex, "[]", { mode: 0o600, flag: "wx" });
    const primary = selfTestChain({
      repo: paths.repo,
      stage: paths.primaryStage,
      vsix: primaryVsix,
      extensions: paths.primaryExtensions,
      installed: primaryInstalled,
      index: primaryIndex,
      label: "primary",
    });
    const reproduction = selfTestChain({
      repo: paths.repo,
      stage: paths.reproductionStage,
      vsix: reproductionVsix,
      extensions: paths.reproductionExtensions,
      installed: reproductionInstalled,
      index: reproductionIndex,
      label: "reproduction",
    });
    const surfaces = assertDualChainDisjoint(primary, reproduction);

    const aliasedVsix = join(root, "reproduction-hardlink.vsix");
    linkSync(primaryVsix, aliasedVsix);
    expectIdentity(
      results,
      "dual chain inode alias",
      () => assertDualChainDisjoint(primary, {
        ...reproduction,
        archive: { path: aliasedVsix },
      }),
      { code: "identity.alias", edge: "chain-disjoint", path: "VSIX paths/inodes" },
    );

    const repoVsix = join(paths.repo, "inside-repo.vsix");
    writeFileSync(repoVsix, "inside", { mode: 0o600, flag: "wx" });
    const authority = {
      repo: canonicalExistingPath(paths.repo, "directory", "self-test", "repo"),
      target: canonicalExistingPath(paths.target, "directory", "self-test", "target"),
    };
    expectIdentity(
      results,
      "VSIX inside authority root",
      () => assertArchivesOutsideAuthority({
        primary: {
          ...surfaces.primary,
          vsix: canonicalExistingPath(repoVsix, "file", "self-test", "inside-repo.vsix"),
        },
        reproduction: surfaces.reproduction,
      }, authority),
      { code: "identity.alias", edge: "chain-disjoint", path: "VSIX/authority roots" },
    );

    const evidencePath = join(paths.evidenceParent, "artifact-identity.json");
    const published = publishVerificationEvidence({
      destination: evidencePath,
      result: {
        primary,
        reproduction,
        authority,
        surfaces,
        dualBuild: { manifest: [], builds: [{ path: primaryVsix }, { path: reproductionVsix }] },
      },
    });
    const evidenceBytes = readFileSync(evidencePath);
    requireCondition(
      published.receipt.path === evidencePath &&
        published.receipt.bytes === evidenceBytes.length &&
        published.receipt.sha256 === sha256(evidenceBytes) &&
        (lstatSync(evidencePath).mode & 0o777) === 0o600 &&
        published.evidence.primary === primary.evidence &&
        published.evidence.reproduction === reproduction.evidence,
      "identity.wrapper_self_test",
      "self-test",
      "shared evidence publisher",
      "wrapper did not retain both complete chains through the shared publisher",
    );
    results.push({
      name: "shared evidence publisher",
      code: "identity.evidence",
      edge: "evidence-publish",
      path: evidencePath,
    });
    expectIdentity(
      results,
      "evidence beside transient VSIX",
      () => publishVerificationEvidence({
        destination: join(paths.archives, "forbidden-evidence.json"),
        result: {
          primary,
          reproduction,
          authority,
          surfaces,
          dualBuild: { manifest: [], builds: [{ path: primaryVsix }, { path: reproductionVsix }] },
        },
      }),
      {
        code: "identity.path",
        edge: "evidence-publish",
        path: join(paths.archives, "forbidden-evidence.json"),
      },
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
  return results;
}

function printResults(results, label) {
  for (const result of results) {
    console.log(`PASS ${result.name}: ${result.code}/${result.edge}/${result.path}`);
  }
  console.log(`${label}: PASS (${results.length} checks)`);
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.selfTest) {
    const faults = runFaultMatrix();
    printResults(faults, "artifact fault matrix");
    printResults(runWrapperSelfTests(faults), "dual-chain wrapper");
    return;
  }

  const result = verifyBothChains(options);
  const published = publishVerificationEvidence({
    destination: options.evidencePath,
    result,
  });
  console.log(`candidate: ${result.primary.authority.head}`);
  console.log(`Cargo.lock: ${result.primary.authority.cargoLock.sha256}`);
  console.log(`canonical server: ${result.primary.evidence.canonical[0].sha256}`);
  console.log(
    `primary VSIX: ${result.primary.archive.outerSha256} ` +
      `(${result.primary.archive.files.length} entries)`,
  );
  console.log(
    `reproduction VSIX: ${result.reproduction.archive.outerSha256} ` +
      `(${result.reproduction.archive.files.length} entries)`,
  );
  console.log(`primary installed root: ${result.primary.install.installedRoot}`);
  console.log(`reproduction installed root: ${result.reproduction.install.installedRoot}`);
  console.log(`evidence: ${published.receipt.path} (${published.receipt.bytes} bytes)`);
  console.log("artifact identity: PASS (two complete chains)");
}

const THIS_FILE = fileURLToPath(import.meta.url);
if (process.argv[1] !== undefined && resolve(process.argv[1]) === THIS_FILE) {
  try {
    main();
  } catch (error) {
    if (error instanceof IdentityError) {
      console.error(
        JSON.stringify({
          code: error.code,
          edge: error.edge,
          path: error.path,
          detail: error.detail,
        }),
      );
    } else {
      console.error(error?.stack ?? String(error));
    }
    console.error(usage());
    process.exitCode = 1;
  }
}
