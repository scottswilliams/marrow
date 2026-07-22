// Deployment resolution and integrity verification for the trusted main.
//
// A `MarrowDeployment` is the directory a packaged app ships: a pinned
// `marrow-runner` binary, the verified `program.image`, and a `marrow-deployment`
// manifest that records their identities and the accepted ceiling id. The trusted
// main resolves the runner ONLY from this directory — never from PATH, the working
// directory, or the environment — and refuses to spawn anything whose bytes do not
// match the manifest.
//
// The two integrity constructions mirror the Rust owners exactly, so a deployment
// the terminal toolchain composes and one this app resolves agree byte for byte:
//
//   companion release id = SHA-256( "marrow.release.companion" ‖ u64_be(len) ‖ bytes )
//   image id             = SHA-256( "marrow.image.v0"          ‖ u64_be(len(tail)) ‖ tail )
//
// where the program image container is `"MWI\0" ‖ version(0x00) ‖ image_id(32) ‖ tail`.
// The image id is recomputed over the tail and checked against both the embedded
// digest and the manifest, so a tampered image is refused before launch; the runner
// re-verifies the image semantically when it serves it.

import { lstatSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { isAbsolute, join } from "node:path";

/** The fixed manifest filename inside a deployment directory. */
const MANIFEST_NAME = "marrow-deployment";

// Law-9 bounds: read nothing unbounded. A stock runner is a few MB; the image is
// capped far below this; the manifest is tiny.
const MAX_MANIFEST_BYTES = 64 * 1024;
const MAX_RUNNER_BYTES = 1 << 30;
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;

const COMPANION_RELEASE_KIND = Buffer.from("marrow.release.companion", "ascii"); // 24 bytes
const IMAGE_DIGEST_KIND = Buffer.from("marrow.image.v0", "ascii"); // 15 bytes
const IMAGE_MAGIC = Buffer.from([0x4d, 0x57, 0x49, 0x00]); // "MWI\0"
const IMAGE_VERSION = 0x00;
/** Offset of the embedded 32-byte digest: magic(4) ‖ version(1). */
const IMAGE_DIGEST_OFFSET = 5;
const IMAGE_HEADER_LEN = IMAGE_DIGEST_OFFSET + 32; // magic ‖ version ‖ digest

const HEX64 = /^[0-9a-f]{64}$/;

/** Every way a deployment can fail to resolve. Each is deployment damage the app
 * reports as an install problem; none is worked around. */
export type DeploymentFaultKind =
  | "manifest_missing"
  | "manifest_malformed"
  | "release_mismatch"
  | "runner_missing"
  | "runner_mismatch"
  | "image_missing"
  | "image_malformed"
  | "image_mismatch";

export class DeploymentFault extends Error {
  readonly kind: DeploymentFaultKind;
  constructor(kind: DeploymentFaultKind, detail: string) {
    super(`${kind}: ${detail}`);
    this.name = "DeploymentFault";
    this.kind = kind;
  }
}

/** The verified deployment the trusted main launches from. */
export interface Deployment {
  /** The absolute path of the verified runner binary, ready to spawn. */
  runner: string;
  /** The absolute path of the verified program image. */
  image: string;
  /** The deployment ceiling id the store is provisioned under. */
  ceilingId: string;
  /** The toolchain release the deployment was composed with. */
  release: string;
  /** The verified image id, for the launch/handshake to prove back. */
  imageId: string;
}

interface Manifest {
  release: string;
  runnerName: string;
  runnerId: string;
  imageName: string;
  imageId: string;
  ceilingId: string;
}

/** The 64-hex companion release identity of a runner binary's bytes. */
export function companionReleaseId(bytes: Buffer): string {
  return frameDigest(COMPANION_RELEASE_KIND, bytes);
}

/** Recompute the 64-hex image id from a program-image container, after checking its
 * fixed magic and version. Throws a malformed fault on a container that is not the
 * frozen v0 shape. */
export function imageIdOf(container: Buffer): string {
  if (container.length < IMAGE_HEADER_LEN) {
    throw new DeploymentFault("image_malformed", "image shorter than its header");
  }
  if (!container.subarray(0, 4).equals(IMAGE_MAGIC)) {
    throw new DeploymentFault("image_malformed", "bad container magic");
  }
  if (container[4] !== IMAGE_VERSION) {
    throw new DeploymentFault("image_malformed", `unsupported container version ${container[4]}`);
  }
  const tail = container.subarray(IMAGE_HEADER_LEN);
  return frameDigest(IMAGE_DIGEST_KIND, tail);
}

/** SHA-256( kind ‖ u64_be(len) ‖ payload ), lowercase hex — the shared Marrow
 * length-delimited domain-separated identity construction. */
function frameDigest(kind: Buffer, payload: Buffer): string {
  const len = Buffer.alloc(8);
  len.writeBigUInt64BE(BigInt(payload.length));
  return createHash("sha256").update(kind).update(len).update(payload).digest("hex");
}

/**
 * Resolve and verify the deployment in `deploymentDir` against `expectedRelease`
 * (the app's own toolchain release). Reads the manifest, verifies the runner's
 * release identity and the image's identity, and returns the verified paths. Any
 * mismatch throws a typed `DeploymentFault` and no path is returned.
 */
export function resolveDeployment(deploymentDir: string, expectedRelease: string): Deployment {
  const manifest = readManifest(deploymentDir);
  if (manifest.release !== expectedRelease) {
    throw new DeploymentFault(
      "release_mismatch",
      `deployment is release ${manifest.release}, app is ${expectedRelease}`,
    );
  }

  const runner = resolveComponent(deploymentDir, manifest.runnerName, "runner");
  const runnerBytes = readBounded(runner, MAX_RUNNER_BYTES, "runner_missing");
  const runnerId = companionReleaseId(runnerBytes);
  if (runnerId !== manifest.runnerId) {
    throw new DeploymentFault("runner_mismatch", "runner bytes do not match the manifest identity");
  }

  const image = resolveComponent(deploymentDir, manifest.imageName, "image");
  const imageBytes = readBounded(image, MAX_IMAGE_BYTES, "image_missing");
  const imageId = imageIdOf(imageBytes);
  const embedded = imageBytes.subarray(IMAGE_DIGEST_OFFSET, IMAGE_HEADER_LEN).toString("hex");
  if (imageId !== embedded) {
    throw new DeploymentFault("image_mismatch", "image digest does not match its own bytes");
  }
  if (imageId !== manifest.imageId) {
    throw new DeploymentFault("image_mismatch", "image bytes do not match the manifest identity");
  }

  return {
    runner,
    image,
    ceilingId: manifest.ceilingId,
    release: manifest.release,
    imageId,
  };
}

function readManifest(dir: string): Manifest {
  const path = join(dir, MANIFEST_NAME);
  let stat;
  try {
    stat = lstatSync(path); // no-follow: a symlinked manifest is not the pinned file
  } catch {
    throw new DeploymentFault("manifest_missing", `no ${MANIFEST_NAME} in the deployment`);
  }
  if (!stat.isFile()) {
    throw new DeploymentFault("manifest_malformed", "manifest is not a regular file");
  }
  if (stat.size > MAX_MANIFEST_BYTES) {
    throw new DeploymentFault("manifest_malformed", "manifest is oversized");
  }
  return parseManifest(readFileSync(path, "utf8"));
}

/** Parse the fixed line-oriented manifest:
 *
 *   marrow deployment v0
 *   release <toolchain version>
 *   runner <name> <64hex companion release id>
 *   image <name> <64hex image id>
 *   ceiling <64hex ceiling id>
 *   end
 */
export function parseManifest(text: string): Manifest {
  const lines = text.split("\n");
  if (lines[0] !== "marrow deployment v0") {
    throw new DeploymentFault("manifest_malformed", "bad manifest header");
  }
  const release = strip(lines[1], "release ");
  const [runnerName, runnerId] = pair(strip(lines[2], "runner "));
  const [imageName, imageId] = pair(strip(lines[3], "image "));
  const ceilingId = strip(lines[4], "ceiling ");
  if (lines[5] !== "end") {
    throw new DeploymentFault("manifest_malformed", "missing end marker");
  }
  if (release.length === 0) {
    throw new DeploymentFault("manifest_malformed", "empty release");
  }
  for (const id of [runnerId, imageId, ceilingId]) {
    if (!HEX64.test(id)) {
      throw new DeploymentFault("manifest_malformed", "an identity is not 64 lowercase hex");
    }
  }
  return { release, runnerName, runnerId, imageName, imageId, ceilingId };
}

function strip(line: string | undefined, prefix: string): string {
  if (line === undefined || !line.startsWith(prefix)) {
    throw new DeploymentFault("manifest_malformed", `expected a \`${prefix.trim()}\` line`);
  }
  return line.slice(prefix.length);
}

function pair(rest: string): [string, string] {
  const at = rest.indexOf(" ");
  if (at <= 0) {
    throw new DeploymentFault("manifest_malformed", "expected `<name> <id>`");
  }
  return [rest.slice(0, at), rest.slice(at + 1)];
}

/**
 * Resolve a manifest-named component to a path inside `dir`, refusing any name that
 * is not a single plain relative path component. A separator (POSIX `/` or Windows
 * `\`), a parent/current reference, a NUL, an absolute path, or a Windows
 * drive-relative name (`C:foo`) could escape the deployment directory. `readBounded`
 * then rejects a symlinked component (no-follow), so the resolved path is only ever a
 * regular file that is a direct child of the deployment directory — the
 * ambient-discovery defense: the runner is only ever the pinned file beside the
 * manifest.
 */
export function resolveComponent(dir: string, name: string, what: "runner" | "image"): string {
  const missing: DeploymentFaultKind = what === "runner" ? "runner_missing" : "image_missing";
  if (
    name.length === 0 ||
    name.includes("/") ||
    name.includes("\\") ||
    name.includes("\0") ||
    name.includes(":") || // a Windows drive/stream specifier is not a plain component
    name === "." ||
    name === ".." ||
    isAbsolute(name)
  ) {
    throw new DeploymentFault(missing, `unsafe ${what} name`);
  }
  return join(dir, name);
}

function readBounded(path: string, max: number, missing: DeploymentFaultKind): Buffer {
  let stat;
  try {
    // No-follow: a symlink at the component path is not the pinned file, even when its
    // name is a plain component. Only a regular file beside the manifest is admitted.
    stat = lstatSync(path);
  } catch {
    throw new DeploymentFault(missing, "a deployment component is missing");
  }
  if (!stat.isFile()) {
    throw new DeploymentFault(missing, "a deployment component is not a regular file");
  }
  if (stat.size > max) {
    throw new DeploymentFault(missing, "a deployment component is oversized");
  }
  return readFileSync(path);
}
