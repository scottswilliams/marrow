#!/usr/bin/env node
// Explicit-invocation VSIX inventory / payload / hash gate for the Marrow installed
// artifact. Never an npm lifecycle script: it is run by hand and by the lane gate.
//
// Usage:
//   node gate/verify-vsix.mjs <a.vsix> [<b.vsix>]
//
// One argument checks a single VSIX against the closed inventory allowlist, the
// hash-pinned assets, and the single-Mach-O payload rule. Two arguments additionally
// prove the two independent builds produce a byte-for-byte identical sorted per-entry
// (path, sha256, mode) manifest. Any deviation exits nonzero.

import { readFileSync } from "node:fs";
import { inflateRawSync } from "node:zlib";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const EXT_ROOT = join(HERE, "..");

// The canonical aarch64-apple-darwin `marrow` release binary of the exact integration
// base. The VSIX payload must be hash-equal to this and to nothing else native.
const CANONICAL_BINARY_SHA256 =
  "1f8947dbfb696973166eecb3d77bcb0df62613578780d6a71ab5bcc4dc78ef8b";
const ICON_GALLERY_SHA256 =
  "544e5243603ddfd3d6a9b0c1ca9acaaeeed5332b03433dc207af17c875fe0435";
const ICON_SVG_SHA256 =
  "1c52928bd156a73fbd4af6785cc3a701fbd4504d7c4fa08da54cedf7e230a300";

// Closed allowlist of exact non-node_modules entries. LICENSE may serialize with or
// without a `.txt` suffix depending on the packager; both spellings are permitted, no
// other variant is.
const ALLOWED_EXACT = new Set([
  "extension.vsixmanifest",
  "[Content_Types].xml",
  "extension/package.json",
  "extension/out/extension.js",
  // vsce lowercases the packaged readme; both spellings are permitted.
  "extension/README.md",
  "extension/readme.md",
  "extension/LICENSE",
  "extension/LICENSE.txt",
  "extension/icons/marrow-gallery.png",
  "extension/icons/marrow.svg",
  "extension/server/marrow",
]);

const SERVER_ENTRY = "extension/server/marrow";
const NODE_MODULES_PREFIX = "extension/node_modules/";

// The security invariant §4 enforces is: exactly one NATIVE (Mach-O) executable —
// the pinned canonical server — and no other native executable, PATH fallback, or
// machine-selection branch. The pinned, license-reviewed `vscode-languageclient=9.0.1`
// closure ships one non-native darwin runtime script that carries a Unix exec bit:
// `terminateProcess.sh`, spawned by the client's force-terminate path on macOS. It is
// a text shell script, never a native payload and never a second server; it is
// admitted here by exact path and proven NOT Mach-O. Any other executable-bit entry
// fails. (The unused `semver/bin` CLI is excluded from the package via `.vscodeignore`.)
const KNOWN_NONNATIVE_EXEC = new Set([
  "extension/node_modules/vscode-languageclient/lib/node/terminateProcess.sh",
]);

// ---- minimal pure-JS zip central-directory reader (no third-party dependency) ----

function u16(buf, off) {
  return buf.readUInt16LE(off);
}
function u32(buf, off) {
  return buf.readUInt32LE(off);
}

function findEocd(buf) {
  // The End Of Central Directory record ends the archive; scan backwards for its
  // signature (0x06054b50). VSIX archives carry no trailing comment in practice, but
  // scan a bounded window to tolerate one.
  const min = Math.max(0, buf.length - 22 - 0xffff);
  for (let i = buf.length - 22; i >= min; i--) {
    if (u32(buf, i) === 0x06054b50) {
      return i;
    }
  }
  throw new Error("not a zip archive: no end-of-central-directory record");
}

function readEntries(buf) {
  const eocd = findEocd(buf);
  const count = u16(buf, eocd + 10);
  let p = u32(buf, eocd + 16);
  const entries = [];
  for (let i = 0; i < count; i++) {
    if (u32(buf, p) !== 0x02014b50) {
      throw new Error(`corrupt central directory header at ${p}`);
    }
    const method = u16(buf, p + 10);
    const compSize = u32(buf, p + 20);
    const nameLen = u16(buf, p + 28);
    const extraLen = u16(buf, p + 30);
    const commentLen = u16(buf, p + 32);
    const externalAttrs = u32(buf, p + 38);
    const localOffset = u32(buf, p + 42);
    const name = buf.toString("utf8", p + 46, p + 46 + nameLen);
    const mode = (externalAttrs >>> 16) & 0xffff;

    // Locate the payload via the local file header (its name/extra lengths can differ
    // from the central record's).
    if (u32(buf, localOffset) !== 0x04034b50) {
      throw new Error(`corrupt local header for ${name}`);
    }
    const localNameLen = u16(buf, localOffset + 26);
    const localExtraLen = u16(buf, localOffset + 28);
    const dataStart = localOffset + 30 + localNameLen + localExtraLen;
    const raw = buf.subarray(dataStart, dataStart + compSize);
    let data;
    if (method === 0) {
      data = Buffer.from(raw);
    } else if (method === 8) {
      data = inflateRawSync(raw);
    } else {
      throw new Error(`unsupported compression method ${method} for ${name}`);
    }
    entries.push({ name, mode, data });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

function sha256(buf) {
  return createHash("sha256").update(buf).digest("hex");
}

function isMachO(data) {
  if (data.length < 4) {
    return false;
  }
  const magic = u32(data, 0);
  // MH_MAGIC_64 / MH_CIGAM_64 / MH_MAGIC / MH_CIGAM / fat (universal) big & little.
  return (
    magic === 0xfeedfacf ||
    magic === 0xcffaedfe ||
    magic === 0xfeedface ||
    magic === 0xcefaedfe ||
    magic === 0xcafebabe ||
    magic === 0xbebafeca
  );
}

function isExecutableMode(mode) {
  return (mode & 0o111) !== 0;
}

// ---- allowed production package prefixes, derived from the frozen lock ----

function allowedPackagePrefixes() {
  const lock = JSON.parse(readFileSync(join(EXT_ROOT, "package-lock.json"), "utf8"));
  const prefixes = new Set();
  for (const [path, meta] of Object.entries(lock.packages ?? {})) {
    if (!path.startsWith("node_modules/")) {
      continue;
    }
    if (meta.dev === true) {
      continue;
    }
    // `node_modules/<pkg>` (possibly scoped) -> allow `extension/node_modules/<pkg>/`.
    const rel = path.slice("node_modules/".length);
    prefixes.add(`${NODE_MODULES_PREFIX}${rel}/`);
  }
  if (prefixes.size === 0) {
    throw new Error("no production packages found in package-lock.json");
  }
  return [...prefixes];
}

// ---- per-VSIX verification ----

function verifyOne(vsixPath, allowedPrefixes) {
  const outerBuf = readFileSync(vsixPath);
  const outerHash = sha256(outerBuf);
  const entries = readEntries(outerBuf);
  const failures = [];
  const manifest = [];
  const machoEntries = [];
  const execEntries = [];

  for (const entry of entries) {
    const hash = sha256(entry.data);
    manifest.push({ path: entry.name, sha256: hash, mode: entry.mode });

    const allowed =
      ALLOWED_EXACT.has(entry.name) ||
      allowedPrefixes.some((pre) => entry.name.startsWith(pre));
    if (!allowed) {
      failures.push(`disallowed entry: ${entry.name}`);
    }

    const macho = isMachO(entry.data);
    if (macho) {
      machoEntries.push({ name: entry.name, hash });
    }
    if (macho || isExecutableMode(entry.mode)) {
      execEntries.push({ name: entry.name, macho });
    }

    if (entry.name === "extension/icons/marrow-gallery.png" && hash !== ICON_GALLERY_SHA256) {
      failures.push(`gallery icon hash ${hash} != pinned ${ICON_GALLERY_SHA256}`);
    }
    if (entry.name === "extension/icons/marrow.svg" && hash !== ICON_SVG_SHA256) {
      failures.push(`svg icon hash ${hash} != pinned ${ICON_SVG_SHA256}`);
    }
    if (entry.name === SERVER_ENTRY && hash !== CANONICAL_BINARY_SHA256) {
      failures.push(`server binary hash ${hash} != canonical ${CANONICAL_BINARY_SHA256}`);
    }
  }

  // Exactly one NATIVE (Mach-O) executable, and it is the server binary.
  if (machoEntries.length !== 1) {
    failures.push(
      `expected exactly one Mach-O entry, found ${machoEntries.length}: ` +
        machoEntries.map((e) => e.name).join(", "),
    );
  } else if (machoEntries[0].name !== SERVER_ENTRY) {
    failures.push(`sole Mach-O entry is ${machoEntries[0].name}, not ${SERVER_ENTRY}`);
  }

  // Every executable-bit entry is either the native server or a known, non-native,
  // license-reviewed dependency script; nothing else may carry the exec bit.
  for (const e of execEntries) {
    if (e.name === SERVER_ENTRY) {
      continue;
    }
    if (KNOWN_NONNATIVE_EXEC.has(e.name)) {
      if (e.macho) {
        failures.push(`known non-native exec entry ${e.name} is unexpectedly Mach-O`);
      }
      continue;
    }
    failures.push(`unexpected executable-bit entry: ${e.name}`);
  }

  // A server binary must be present at all.
  if (!entries.some((e) => e.name === SERVER_ENTRY)) {
    failures.push(`missing ${SERVER_ENTRY}`);
  }

  manifest.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return { vsixPath, outerHash, manifest, failures };
}

function main() {
  const args = process.argv.slice(2);
  if (args.length < 1 || args.length > 2) {
    console.error("usage: node gate/verify-vsix.mjs <a.vsix> [<b.vsix>]");
    process.exit(2);
  }

  const allowedPrefixes = allowedPackagePrefixes();
  const results = args.map((p) => verifyOne(p, allowedPrefixes));

  let ok = true;
  for (const r of results) {
    console.log(`\nVSIX: ${r.vsixPath}`);
    console.log(`  outer sha256: ${r.outerHash}`);
    console.log(`  entries: ${r.manifest.length}`);
    for (const m of r.manifest) {
      console.log(`    ${m.sha256}  ${m.mode.toString(8).padStart(6, "0")}  ${m.path}`);
    }
    if (r.failures.length > 0) {
      ok = false;
      console.error(`  FAIL (${r.failures.length}):`);
      for (const f of r.failures) {
        console.error(`    - ${f}`);
      }
    } else {
      console.log("  inventory/payload/hash checks: PASS");
    }
  }

  if (results.length === 2) {
    const [a, b] = results;
    const ja = JSON.stringify(a.manifest);
    const jb = JSON.stringify(b.manifest);
    if (ja === jb) {
      console.log("\nDual-build manifest equivalence: PASS (identical sorted per-entry manifest)");
    } else {
      ok = false;
      console.error("\nDual-build manifest equivalence: FAIL (manifests differ)");
      const max = Math.max(a.manifest.length, b.manifest.length);
      for (let i = 0; i < max; i++) {
        const ea = a.manifest[i];
        const eb = b.manifest[i];
        if (JSON.stringify(ea) !== JSON.stringify(eb)) {
          console.error(`  [${i}] A: ${ea ? `${ea.path} ${ea.sha256} ${ea.mode}` : "—"}`);
          console.error(`  [${i}] B: ${eb ? `${eb.path} ${eb.sha256} ${eb.mode}` : "—"}`);
        }
      }
    }
  }

  process.exit(ok ? 0 : 1);
}

main();
