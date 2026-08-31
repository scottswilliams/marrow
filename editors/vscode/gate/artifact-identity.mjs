#!/usr/bin/env node
// Canonical artifact identity for the Marrow VS Code package.
//
// This module deliberately derives every digest from one invocation's clean
// candidate and build outputs. It contains no pinned artifact digest and is shared
// by the package verifier, installed probe, and real-host controller.

import { createHash, randomBytes } from "node:crypto";
import {
  closeSync,
  constants as fsConstants,
  fchmodSync,
  fstatSync,
  fsyncSync,
  linkSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
} from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { inflateRawSync } from "node:zlib";

export const SERVER_PATH = "server/marrow-lsp";
export const ARTIFACT_FAULT_NAMES = Object.freeze([
  "stale agreeing chain",
  "stage to VSIX",
  "VSIX to install",
  "extra inventory",
  "missing inventory",
  "mode",
  "stage ordinary mode",
  "installed ordinary mode",
  "wrong Mach-O architecture",
  "second Mach-O",
  "ordinary mode",
  "installed manifest delta",
  "installed metadata shape",
  "candidate HEAD",
  "candidate lock",
  "candidate helper timeout",
  "duplicate install record",
  "outside install root",
  "VSIX identity",
  "VSIX TargetPlatform",
  "dual build manifest",
  "dual build provenance",
  "stage aliases target",
  "install aliases stage",
]);
export const REQUIRED_EXTENSION_FILES = Object.freeze([
  "package.json",
  "out/extension.js",
  "readme.md",
  "LICENSE.txt",
  "icons/marrow-gallery.png",
  "icons/marrow.svg",
  "language-configuration.json",
  "syntaxes/marrow.tmLanguage.json",
  SERVER_PATH,
]);

const REQUIRED_VSIX_ROOT_FILES = Object.freeze([
  "extension.vsixmanifest",
  "[Content_Types].xml",
]);
const CANDIDATE_STAGE_INPUTS = Object.freeze([
  ".npmrc",
  ".vscodeignore",
  "package.json",
  "package-lock.json",
  "tsconfig.json",
  "src/extension.ts",
  "README.md",
  "LICENSE",
  "icons/marrow-gallery.png",
  "icons/marrow.svg",
  "language-configuration.json",
  "syntaxes/marrow.tmLanguage.json",
]);
const VSIX_STAGE_ALIASES = new Map([
  ["readme.md", "README.md"],
  ["LICENSE.txt", "LICENSE"],
]);
const KNOWN_NONNATIVE_EXECUTABLE =
  "node_modules/vscode-languageclient/lib/node/terminateProcess.sh";
const MAX_ARCHIVE_ENTRY_BYTES = 128 * 1024 * 1024;
const MAX_ARCHIVE_TOTAL_BYTES = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES = 20_000;
const MAX_GIT_OUTPUT_BYTES = 4 * 1024 * 1024;
const GIT_TIMEOUT_MS = 5_000;
export const MAX_EVIDENCE_BYTES = 4 * 1024 * 1024;
const MAX_EVIDENCE_TEMP_ATTEMPTS = 32;
const ARM64_CPU_TYPE = 0x0100000c;
const VSIX_TARGET_PLATFORM = "darwin-arm64";
const VSIX_IDENTITY_KEYS = Object.freeze([
  "Id",
  "Language",
  "Publisher",
  "TargetPlatform",
  "Version",
]);
const UTF8 = new TextDecoder("utf-8", { fatal: true });

/** A stable, machine-readable artifact-identity failure. */
export class IdentityError extends Error {
  constructor(code, edge, path, detail = "") {
    super(`${code}: ${edge}: ${path}${detail ? `: ${detail}` : ""}`);
    this.name = "IdentityError";
    this.code = code;
    this.edge = edge;
    this.path = path;
    this.detail = detail;
  }
}

function fail(code, edge, path, detail = "") {
  throw new IdentityError(code, edge, path, detail);
}

function requireCondition(condition, code, edge, path, detail = "") {
  if (!condition) {
    fail(code, edge, path, detail);
  }
}

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function displayBytes(bytes, limit = 300) {
  return bytes.toString("utf8", 0, Math.min(bytes.length, limit)).replaceAll("\0", "\\0");
}

function canonicalExistingPath(path, kind, edge) {
  requireCondition(
    typeof path === "string" && isAbsolute(path),
    "identity.path",
    edge,
    String(path),
    "path must be absolute",
  );
  let info;
  try {
    info = lstatSync(path);
  } catch (error) {
    fail("identity.path", edge, path, `unavailable: ${error.message}`);
  }
  requireCondition(!info.isSymbolicLink(), "identity.path", edge, path, "symlink rejected");
  if (kind === "directory") {
    requireCondition(info.isDirectory(), "identity.path", edge, path, "not a directory");
  } else {
    requireCondition(info.isFile(), "identity.path", edge, path, "not a regular file");
  }
  return realpathSync(path);
}

function isWithin(path, parent) {
  const rel = relative(parent, path);
  return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel));
}

function requireOutside(path, forbidden, edge) {
  requireCondition(
    !isWithin(path, forbidden),
    "identity.path",
    edge,
    path,
    `must be outside ${forbidden}`,
  );
}

function requireDisjointTrees(left, right, edge) {
  requireOutside(left, right, edge);
  requireOutside(right, left, edge);
}

function requireDistinctInodes(left, right, edge, path) {
  requireCondition(
    left.device !== right.device || left.inode !== right.inode,
    "identity.alias",
    edge,
    path,
    "surfaces alias one inode",
  );
}

function lstatIfPresent(path) {
  try {
    return lstatSync(path);
  } catch (error) {
    if (error?.code === "ENOENT") return undefined;
    throw error;
  }
}

function syncDirectory(path, edge) {
  let descriptor;
  try {
    descriptor = openSync(
      path,
      fsConstants.O_RDONLY | (fsConstants.O_DIRECTORY ?? 0),
    );
    fsyncSync(descriptor);
  } catch (error) {
    if (
      process.platform === "win32" &&
      ["EBADF", "EINVAL", "ENOTSUP", "EPERM"].includes(error?.code)
    ) {
      return;
    }
    if (error instanceof IdentityError) throw error;
    fail("identity.evidence", edge, path, `parent fsync failed: ${error.message}`);
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

function evidenceTempPath(parent, destinationName) {
  return join(
    parent,
    `.${destinationName}.marrow-evidence-${randomBytes(24).toString("hex")}.tmp`,
  );
}

function writeAll(descriptor, bytes, edge, path) {
  let offset = 0;
  while (offset < bytes.length) {
    const written = writeSync(descriptor, bytes, offset, bytes.length - offset, offset);
    requireCondition(
      written > 0,
      "identity.evidence",
      edge,
      path,
      `write stopped at ${offset}/${bytes.length}`,
    );
    offset += written;
  }
}

/**
 * Durably publishes one bounded evidence artifact without replacing any path.
 * Callers own serialization and must enumerate every transient or authoritative
 * root that cannot contain the retained evidence.
 */
export function publishEvidence({ destination, bytes, forbiddenRoots }) {
  const edge = "evidence-publish";
  requireCondition(
    typeof destination === "string" && isAbsolute(destination),
    "identity.evidence",
    edge,
    String(destination),
    "destination must be absolute",
  );
  requireCondition(
    Buffer.isBuffer(bytes),
    "identity.evidence",
    edge,
    destination,
    "bytes must be a Buffer",
  );
  const body = Buffer.from(bytes);
  requireCondition(
    body.length > 0 && body.length <= MAX_EVIDENCE_BYTES,
    "identity.evidence_limit",
    edge,
    destination,
    `${body.length} is outside 1..${MAX_EVIDENCE_BYTES}`,
  );
  requireCondition(
    Array.isArray(forbiddenRoots) && forbiddenRoots.length > 0,
    "identity.evidence",
    edge,
    destination,
    "at least one forbidden root is required",
  );

  const requestedParent = dirname(destination);
  const parent = canonicalExistingPath(requestedParent, "directory", edge);
  const output = join(parent, basename(destination));
  requireCondition(
    lstatIfPresent(output) === undefined,
    "identity.evidence",
    edge,
    output,
    "destination already exists",
  );
  const canonicalForbiddenRoots = forbiddenRoots.map((root, index) => {
    requireCondition(
      typeof root === "string" && isAbsolute(root),
      "identity.evidence",
      edge,
      String(root),
      `forbiddenRoots[${index}] must be absolute`,
    );
    return canonicalExistingPath(root, "directory", edge);
  });
  requireCondition(
    new Set(canonicalForbiddenRoots).size === canonicalForbiddenRoots.length,
    "identity.evidence",
    edge,
    output,
    "duplicate forbidden root",
  );
  for (const forbidden of canonicalForbiddenRoots) {
    requireOutside(output, forbidden, edge);
  }

  let descriptor;
  let temporary;
  try {
    for (let attempt = 0; attempt < MAX_EVIDENCE_TEMP_ATTEMPTS; attempt++) {
      const candidate = evidenceTempPath(parent, basename(output));
      try {
        descriptor = openSync(
          candidate,
          fsConstants.O_WRONLY |
            fsConstants.O_CREAT |
            fsConstants.O_EXCL |
            (fsConstants.O_NOFOLLOW ?? 0),
          0o600,
        );
        temporary = candidate;
        break;
      } catch (error) {
        if (error?.code !== "EEXIST") throw error;
      }
    }
    requireCondition(
      descriptor !== undefined && temporary !== undefined,
      "identity.evidence",
      edge,
      output,
      `unable to reserve an exclusive temporary after ${MAX_EVIDENCE_TEMP_ATTEMPTS} attempts`,
    );
    fchmodSync(descriptor, 0o600);
    writeAll(descriptor, body, edge, temporary);
    fsyncSync(descriptor);
    const temporaryInfo = fstatSync(descriptor);
    requireCondition(
      temporaryInfo.isFile() && (temporaryInfo.mode & 0o777) === 0o600 &&
        temporaryInfo.size === body.length,
      "identity.evidence",
      edge,
      temporary,
      "temporary verification failed",
    );
    closeSync(descriptor);
    descriptor = undefined;

    try {
      linkSync(temporary, output);
    } catch (error) {
      if (error?.code === "EEXIST") {
        fail("identity.evidence", edge, output, "destination appeared during publication");
      }
      throw error;
    }
    const finalInfo = lstatSync(output);
    requireCondition(
      finalInfo.isFile() && !finalInfo.isSymbolicLink(),
      "identity.evidence",
      edge,
      output,
      "published path is not a regular non-symlink file",
    );
    requireCondition(
      finalInfo.dev === temporaryInfo.dev && finalInfo.ino === temporaryInfo.ino,
      "identity.evidence",
      edge,
      output,
      "published path does not name the exclusive temporary inode",
    );
    requireCondition(
      (finalInfo.mode & 0o777) === 0o600 && finalInfo.size === body.length,
      "identity.evidence",
      edge,
      output,
      `mode/size ${(finalInfo.mode & 0o777).toString(8)}/${finalInfo.size}`,
    );
    const digest = sha256(body);
    requireCondition(
      sha256(readFileSync(output)) === digest,
      "identity.evidence",
      edge,
      output,
      "published digest mismatch",
    );
    const finalAfterRead = lstatSync(output);
    requireCondition(
      finalAfterRead.isFile() && !finalAfterRead.isSymbolicLink() &&
        finalAfterRead.dev === temporaryInfo.dev &&
        finalAfterRead.ino === temporaryInfo.ino &&
        (finalAfterRead.mode & 0o777) === 0o600 &&
        finalAfterRead.size === body.length,
      "identity.evidence",
      edge,
      output,
      "published path changed during verification",
    );
    syncDirectory(parent, edge);
    unlinkSync(temporary);
    temporary = undefined;
    syncDirectory(parent, edge);
    return {
      path: output,
      bytes: body.length,
      sha256: digest,
      mode: finalInfo.mode & 0o777,
    };
  } catch (error) {
    if (error instanceof IdentityError) throw error;
    fail("identity.evidence", edge, output, error.message);
  } finally {
    let cleanupError;
    if (descriptor !== undefined) {
      try {
        closeSync(descriptor);
      } catch (error) {
        cleanupError = error;
      }
    }
    if (temporary !== undefined) {
      try {
        unlinkSync(temporary);
      } catch (error) {
        if (error?.code !== "ENOENT" && cleanupError === undefined) cleanupError = error;
      }
    }
    if (cleanupError !== undefined) {
      fail("identity.evidence", edge, temporary ?? output, `temporary cleanup failed: ${cleanupError.message}`);
    }
  }
}

function validateRelativePath(path, edge = "manifest") {
  requireCondition(typeof path === "string" && path.length > 0, "identity.path", edge, String(path));
  requireCondition(!path.includes("\0"), "identity.path", edge, path, "NUL rejected");
  requireCondition(!path.includes("\\"), "identity.path", edge, path, "backslash rejected");
  requireCondition(!path.startsWith("/"), "identity.path", edge, path, "absolute path rejected");
  requireCondition(!path.endsWith("/"), "identity.path", edge, path, "directory entry rejected");
  const pieces = path.split("/");
  requireCondition(
    pieces.every((piece) => piece.length > 0 && piece !== "." && piece !== ".."),
    "identity.path",
    edge,
    path,
    "empty or traversal component rejected",
  );
  requireCondition(path.normalize("NFC") === path, "identity.path", edge, path, "non-NFC path rejected");
  return path;
}

function assertNoSymlinkPath(root, relativePath, edge) {
  validateRelativePath(relativePath, edge);
  let cursor = root;
  const pieces = relativePath.split("/");
  for (let i = 0; i < pieces.length; i++) {
    cursor = join(cursor, pieces[i]);
    let info;
    try {
      info = lstatSync(cursor);
    } catch (error) {
      fail("identity.path", edge, relativePath, `unavailable: ${error.message}`);
    }
    requireCondition(!info.isSymbolicLink(), "identity.path", edge, relativePath, "symlink rejected");
    if (i + 1 < pieces.length) {
      requireCondition(info.isDirectory(), "identity.path", edge, relativePath, "non-directory parent");
    } else {
      requireCondition(info.isFile(), "identity.path", edge, relativePath, "not a regular file");
    }
  }
  return cursor;
}

function fileRecord(root, relativePath, logicalPath = relativePath, edge = "tree") {
  const path = assertNoSymlinkPath(root, relativePath, edge);
  const info = statSync(path);
  const data = readFileSync(path);
  return {
    path: validateRelativePath(logicalPath, edge),
    data,
    sha256: sha256(data),
    mode: info.mode & 0o777,
    size: data.length,
    device: info.dev,
    inode: info.ino,
    sourcePath: path,
  };
}

function makeFileSet(name, files, metadata = {}) {
  const exact = new Set();
  const folded = new Set();
  for (const file of files) {
    validateRelativePath(file.path, name);
    requireCondition(!exact.has(file.path), "identity.inventory", name, file.path, "duplicate path");
    exact.add(file.path);
    const key = file.path.toLocaleLowerCase("en-US");
    requireCondition(!folded.has(key), "identity.inventory", name, file.path, "case-fold collision");
    folded.add(key);
  }
  const sorted = [...files].sort((a, b) => a.path.localeCompare(b.path, "en-US"));
  return { name, files: sorted, metadata };
}

function surfaceMap(surface) {
  return new Map(surface.files.map((file) => [file.path, file]));
}

function publicManifest(surface) {
  return surface.files.map(({ path, sha256: digest, mode, size }) => ({
    path,
    sha256: digest,
    mode,
    size,
  }));
}

function installedProbeRecord(surface) {
  const root = canonicalExistingPath(surface.root ?? surface.metadata?.root, "directory", "installed-probe-handoff");
  return Object.freeze({
    root,
    files: Object.freeze(surface.files.map((file) => Object.freeze({
      path: file.path,
      sha256: file.sha256,
      mode: file.mode,
      size: file.size,
      device: file.device,
      inode: file.inode,
      sourcePath: file.sourcePath,
    }))),
  });
}

export function captureInstalledProbeRecord(installedRoot) {
  return installedProbeRecord(
    readTreeManifest(installedRoot, { name: "installed-probe-handoff" }),
  );
}

export function assertInstalledProbeRecordCurrent(expected) {
  requireCondition(
    isPlainObject(expected) && typeof expected.root === "string" && Array.isArray(expected.files),
    "identity.install_handoff",
    "installed-probe-handoff",
    "*",
    "verified installed record is absent or malformed",
  );
  const current = captureInstalledProbeRecord(expected.root);
  requireCondition(
    stableJson(current) === stableJson(expected),
    "identity.install_handoff",
    "installed-probe-handoff",
    expected.root,
    "installed tree changed after artifact verification",
  );
  return current;
}

export function readTreeManifest(rootPath, { name = "tree" } = {}) {
  const root = canonicalExistingPath(rootPath, "directory", name);
  const files = [];
  let totalBytes = 0;

  function visit(directory, prefix) {
    const names = readdirSync(directory).sort((a, b) => a.localeCompare(b, "en-US"));
    for (const entry of names) {
      const path = join(directory, entry);
      const rel = prefix ? `${prefix}/${entry}` : entry;
      validateRelativePath(rel, name);
      const info = lstatSync(path);
      requireCondition(!info.isSymbolicLink(), "identity.path", name, rel, "symlink rejected");
      if (info.isDirectory()) {
        visit(path, rel);
      } else if (info.isFile()) {
        const data = readFileSync(path);
        totalBytes += data.length;
        requireCondition(
          files.length + 1 <= MAX_ARCHIVE_ENTRIES && totalBytes <= MAX_ARCHIVE_TOTAL_BYTES,
          "identity.bounds",
          name,
          rel,
          "tree exceeds file or byte bound",
        );
        files.push({
          path: rel,
          data,
          sha256: sha256(data),
          mode: info.mode & 0o777,
          size: data.length,
          device: info.dev,
          inode: info.ino,
          sourcePath: path,
        });
      } else {
        fail("identity.path", name, rel, "special file rejected");
      }
    }
  }

  visit(root, "");
  return { ...makeFileSet(name, files, { root }), root };
}

function readU16(buffer, offset, edge, path) {
  requireCondition(offset >= 0 && offset + 2 <= buffer.length, "identity.zip", edge, path, "truncated u16");
  return buffer.readUInt16LE(offset);
}

function readU32(buffer, offset, edge, path) {
  requireCondition(offset >= 0 && offset + 4 <= buffer.length, "identity.zip", edge, path, "truncated u32");
  return buffer.readUInt32LE(offset);
}

let crcTable;
function crc32(buffer) {
  if (crcTable === undefined) {
    crcTable = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let value = i;
      for (let bit = 0; bit < 8; bit++) {
        value = (value & 1) !== 0 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
      }
      crcTable[i] = value >>> 0;
    }
  }
  let value = 0xffffffff;
  for (const byte of buffer) {
    value = crcTable[(value ^ byte) & 0xff] ^ (value >>> 8);
  }
  return (value ^ 0xffffffff) >>> 0;
}

function decodeUtf8(buffer, edge, path) {
  try {
    return UTF8.decode(buffer);
  } catch (error) {
    fail("identity.zip", edge, path, `invalid UTF-8 name: ${error.message}`);
  }
}

function findEocd(buffer, edge) {
  requireCondition(buffer.length >= 22, "identity.zip", edge, "EOCD", "archive too short");
  const minimum = Math.max(0, buffer.length - 22 - 0xffff);
  for (let offset = buffer.length - 22; offset >= minimum; offset--) {
    if (readU32(buffer, offset, edge, "EOCD") === 0x06054b50) {
      const commentLength = readU16(buffer, offset + 20, edge, "EOCD");
      if (offset + 22 + commentLength === buffer.length) {
        return offset;
      }
    }
  }
  fail("identity.zip", edge, "EOCD", "end-of-central-directory record absent");
}

export function readVsix(vsixPath) {
  const path = canonicalExistingPath(vsixPath, "file", "vsix");
  const archiveInfo = statSync(path);
  const archive = readFileSync(path);
  const eocd = findEocd(archive, "vsix");
  const disk = readU16(archive, eocd + 4, "vsix", "EOCD");
  const centralDisk = readU16(archive, eocd + 6, "vsix", "EOCD");
  const diskEntries = readU16(archive, eocd + 8, "vsix", "EOCD");
  const entryCount = readU16(archive, eocd + 10, "vsix", "EOCD");
  const centralSize = readU32(archive, eocd + 12, "vsix", "EOCD");
  const centralOffset = readU32(archive, eocd + 16, "vsix", "EOCD");
  requireCondition(
    disk === 0 && centralDisk === 0 && diskEntries === entryCount,
    "identity.zip",
    "vsix",
    "EOCD",
    "multi-disk archive rejected",
  );
  requireCondition(
    entryCount !== 0xffff && centralSize !== 0xffffffff && centralOffset !== 0xffffffff,
    "identity.zip",
    "vsix",
    "EOCD",
    "ZIP64 archive rejected",
  );
  requireCondition(entryCount <= MAX_ARCHIVE_ENTRIES, "identity.bounds", "vsix", "EOCD", "too many entries");
  requireCondition(
    centralOffset + centralSize === eocd,
    "identity.zip",
    "vsix",
    "central-directory",
    "central directory is not contiguous with EOCD",
  );

  const files = [];
  const localRanges = [];
  let totalBytes = 0;
  let cursor = centralOffset;
  for (let index = 0; index < entryCount; index++) {
    requireCondition(
      readU32(archive, cursor, "vsix", `central[${index}]`) === 0x02014b50,
      "identity.zip",
      "vsix",
      `central[${index}]`,
      "bad central header signature",
    );
    const flags = readU16(archive, cursor + 8, "vsix", `central[${index}]`);
    const method = readU16(archive, cursor + 10, "vsix", `central[${index}]`);
    const expectedCrc = readU32(archive, cursor + 16, "vsix", `central[${index}]`);
    const compressedSize = readU32(archive, cursor + 20, "vsix", `central[${index}]`);
    const size = readU32(archive, cursor + 24, "vsix", `central[${index}]`);
    const nameLength = readU16(archive, cursor + 28, "vsix", `central[${index}]`);
    const extraLength = readU16(archive, cursor + 30, "vsix", `central[${index}]`);
    const commentLength = readU16(archive, cursor + 32, "vsix", `central[${index}]`);
    const externalAttributes = readU32(archive, cursor + 38, "vsix", `central[${index}]`);
    const localOffset = readU32(archive, cursor + 42, "vsix", `central[${index}]`);
    const headerEnd = cursor + 46 + nameLength + extraLength + commentLength;
    requireCondition(headerEnd <= eocd, "identity.zip", "vsix", `central[${index}]`, "truncated header");
    const nameBytes = archive.subarray(cursor + 46, cursor + 46 + nameLength);
    const entryPath = validateRelativePath(decodeUtf8(nameBytes, "vsix", `central[${index}]`), "vsix");
    requireCondition((flags & 0x1) === 0, "identity.zip", "vsix", entryPath, "encrypted entry rejected");
    requireCondition(method === 0 || method === 8, "identity.zip", "vsix", entryPath, `method ${method} rejected`);
    requireCondition(
      size !== 0xffffffff && compressedSize !== 0xffffffff && size <= MAX_ARCHIVE_ENTRY_BYTES,
      "identity.bounds",
      "vsix",
      entryPath,
      "entry exceeds size bound or uses ZIP64",
    );
    requireCondition(
      readU32(archive, localOffset, "vsix", entryPath) === 0x04034b50,
      "identity.zip",
      "vsix",
      entryPath,
      "bad local header signature",
    );
    const localFlags = readU16(archive, localOffset + 6, "vsix", entryPath);
    const localMethod = readU16(archive, localOffset + 8, "vsix", entryPath);
    const localNameLength = readU16(archive, localOffset + 26, "vsix", entryPath);
    const localExtraLength = readU16(archive, localOffset + 28, "vsix", entryPath);
    const localNameStart = localOffset + 30;
    const localNameEnd = localNameStart + localNameLength;
    requireCondition(localNameEnd + localExtraLength <= centralOffset, "identity.zip", "vsix", entryPath, "truncated local header");
    const localName = decodeUtf8(archive.subarray(localNameStart, localNameEnd), "vsix", entryPath);
    requireCondition(localName === entryPath, "identity.zip", "vsix", entryPath, "local/central name mismatch");
    requireCondition(localFlags === flags && localMethod === method, "identity.zip", "vsix", entryPath, "local/central flags or method mismatch");
    const dataStart = localNameEnd + localExtraLength;
    const dataEnd = dataStart + compressedSize;
    requireCondition(dataEnd <= centralOffset, "identity.zip", "vsix", entryPath, "compressed data out of bounds");
    const compressed = archive.subarray(dataStart, dataEnd);
    let data;
    try {
      data = method === 0 ? Buffer.from(compressed) : inflateRawSync(compressed, { maxOutputLength: MAX_ARCHIVE_ENTRY_BYTES });
    } catch (error) {
      fail("identity.zip", "vsix", entryPath, `inflate failed: ${error.message}`);
    }
    requireCondition(data.length === size, "identity.zip", "vsix", entryPath, "uncompressed size mismatch");
    requireCondition(crc32(data) === expectedCrc, "identity.zip", "vsix", entryPath, "CRC mismatch");
    let localRecordEnd = dataEnd;
    if ((flags & 0x8) !== 0) {
      const descriptorMatches = (offset) =>
        offset + 12 <= centralOffset &&
        readU32(archive, offset, "vsix", entryPath) === expectedCrc &&
        readU32(archive, offset + 4, "vsix", entryPath) === compressedSize &&
        readU32(archive, offset + 8, "vsix", entryPath) === size;
      const signed =
        dataEnd + 16 <= centralOffset &&
        readU32(archive, dataEnd, "vsix", entryPath) === 0x08074b50 &&
        descriptorMatches(dataEnd + 4);
      const unsigned = descriptorMatches(dataEnd);
      requireCondition(
        signed || unsigned,
        "identity.zip",
        "vsix",
        entryPath,
        "data descriptor disagrees with central directory",
      );
      localRecordEnd = dataEnd + (signed ? 16 : 12);
    } else {
      requireCondition(
        readU32(archive, localOffset + 14, "vsix", entryPath) === expectedCrc &&
          readU32(archive, localOffset + 18, "vsix", entryPath) === compressedSize &&
          readU32(archive, localOffset + 22, "vsix", entryPath) === size,
        "identity.zip",
        "vsix",
        entryPath,
        "local sizes or CRC disagree with central directory",
      );
    }
    totalBytes += data.length;
    requireCondition(totalBytes <= MAX_ARCHIVE_TOTAL_BYTES, "identity.bounds", "vsix", entryPath, "archive exceeds byte bound");
    const rawMode = (externalAttributes >>> 16) & 0xffff;
    const fileKind = rawMode & 0o170000;
    requireCondition(fileKind === 0 || fileKind === 0o100000, "identity.zip", "vsix", entryPath, "non-regular entry rejected");
    files.push({
      path: entryPath,
      data,
      sha256: sha256(data),
      mode: rawMode & 0o777,
      size: data.length,
    });
    localRanges.push({ start: localOffset, end: localRecordEnd, path: entryPath });
    cursor = headerEnd;
  }
  requireCondition(cursor === centralOffset + centralSize, "identity.zip", "vsix", "central-directory", "size mismatch");
  localRanges.sort((a, b) => a.start - b.start);
  for (let i = 1; i < localRanges.length; i++) {
    requireCondition(
      localRanges[i - 1].end <= localRanges[i].start,
      "identity.zip",
      "vsix",
      localRanges[i].path,
      "overlapping local records rejected",
    );
  }
  return {
    ...makeFileSet("vsix-archive", files, {
      path,
      outerSha256: sha256(archive),
      device: archiveInfo.dev,
      inode: archiveInfo.ino,
      mode: archiveInfo.mode & 0o777,
      size: archiveInfo.size,
    }),
    path,
    outerSha256: sha256(archive),
    device: archiveInfo.dev,
    inode: archiveInfo.ino,
    mode: archiveInfo.mode & 0o777,
    size: archiveInfo.size,
  };
}

function packagedPayloadMode(path) {
  const payloadPath = path.startsWith("extension/")
    ? path.slice("extension/".length)
    : path;
  return payloadPath === SERVER_PATH || payloadPath === KNOWN_NONNATIVE_EXECUTABLE
    ? 0o755
    : 0o644;
}

export function normalizeVsixModes(vsixPath) {
  const before = readVsix(vsixPath);
  const path = before.path;
  const archive = readFileSync(path);
  const eocd = findEocd(archive, "vsix-mode-normalization");
  const entryCount = readU16(archive, eocd + 10, "vsix-mode-normalization", "EOCD");
  let cursor = readU32(archive, eocd + 16, "vsix-mode-normalization", "EOCD");
  let changedEntries = 0;
  for (let index = 0; index < entryCount; index++) {
    requireCondition(
      readU32(archive, cursor, "vsix-mode-normalization", `central[${index}]`) === 0x02014b50,
      "identity.zip",
      "vsix-mode-normalization",
      `central[${index}]`,
      "bad central header signature",
    );
    const nameLength = readU16(archive, cursor + 28, "vsix-mode-normalization", `central[${index}]`);
    const extraLength = readU16(archive, cursor + 30, "vsix-mode-normalization", `central[${index}]`);
    const commentLength = readU16(archive, cursor + 32, "vsix-mode-normalization", `central[${index}]`);
    const headerEnd = cursor + 46 + nameLength + extraLength + commentLength;
    requireCondition(
      headerEnd <= eocd,
      "identity.zip",
      "vsix-mode-normalization",
      `central[${index}]`,
      "truncated central header",
    );
    const entryPath = validateRelativePath(
      decodeUtf8(
        archive.subarray(cursor + 46, cursor + 46 + nameLength),
        "vsix-mode-normalization",
        `central[${index}]`,
      ),
      "vsix-mode-normalization",
    );
    const externalAttributes = readU32(
      archive,
      cursor + 38,
      "vsix-mode-normalization",
      entryPath,
    );
    const rawMode = (externalAttributes >>> 16) & 0xffff;
    const expectedMode = packagedPayloadMode(entryPath);
    if ((rawMode & 0o777) !== expectedMode) {
      const normalizedRawMode = (rawMode & ~0o777) | expectedMode;
      const normalizedAttributes =
        (((normalizedRawMode & 0xffff) << 16) | (externalAttributes & 0xffff)) >>> 0;
      archive.writeUInt32LE(normalizedAttributes, cursor + 38);
      changedEntries += 1;
    }
    cursor = headerEnd;
  }
  writeFileSync(path, archive);
  const after = readVsix(path);
  for (const file of after.files) {
    requireCondition(
      file.mode === packagedPayloadMode(file.path),
      "identity.mode",
      "vsix-mode-normalization",
      file.path,
      `${file.mode.toString(8)} != ${packagedPayloadMode(file.path).toString(8)}`,
    );
  }
  return Object.freeze({
    path,
    entries: after.files.length,
    changedEntries,
    beforeSha256: before.outerSha256,
    afterSha256: after.outerSha256,
  });
}

function runBoundedSync(command, args, options, { code, edge, path, timeoutMs = GIT_TIMEOUT_MS }) {
  const result = spawnSync(command, args, {
    ...options,
    timeout: timeoutMs,
    killSignal: "SIGKILL",
  });
  if (result.error !== undefined || result.status !== 0 || result.signal !== null) {
    const timedOut = result.error?.code === "ETIMEDOUT";
    const detail = timedOut
      ? `timed out after ${timeoutMs} ms`
      : result.error?.message ?? displayBytes(result.stderr ?? Buffer.alloc(0));
    fail(code, edge, path, detail);
  }
  return result;
}

function runGit(repoRoot, args, edge) {
  const result = runBoundedSync("git", ["-C", repoRoot, ...args], {
    encoding: null,
    maxBuffer: MAX_GIT_OUTPUT_BYTES,
    stdio: ["ignore", "pipe", "pipe"],
  }, {
    code: "identity.candidate",
    edge,
    path: args.join(" "),
  });
  return result.stdout;
}

function assertExpectedHead(actual, expected) {
  requireCondition(
    typeof expected === "string" && /^[0-9a-f]{40}$/.test(expected),
    "identity.candidate",
    "candidate-head",
    "HEAD",
    "expected HEAD must be a lowercase 40-hex commit",
  );
  requireCondition(actual === expected, "identity.candidate", "candidate-head", "HEAD", `${actual} != ${expected}`);
}

function assertSameBytes(left, right, edge, path) {
  requireCondition(left.equals(right), "identity.digest", edge, path, `${sha256(left)} != ${sha256(right)}`);
}

export function assertCandidateAuthority({ repoRoot, expectedHead }) {
  const root = canonicalExistingPath(repoRoot, "directory", "candidate");
  const top = realpathSync(runGit(root, ["rev-parse", "--show-toplevel"], "candidate-root").toString("utf8").trim());
  requireCondition(top === root, "identity.candidate", "candidate-root", root, `Git root is ${top}`);
  const head = runGit(root, ["rev-parse", "HEAD"], "candidate-head").toString("utf8").trim();
  assertExpectedHead(head, expectedHead);
  const staged = runGit(root, ["diff", "--cached", "--name-only", "-z"], "candidate-clean");
  requireCondition(staged.length === 0, "identity.candidate", "candidate-clean", root, displayBytes(staged));
  const changed = [
    ...runGit(root, ["diff", "--name-only", "-z"], "candidate-clean")
      .toString("utf8").split("\0").filter(Boolean),
    ...runGit(root, ["ls-files", "--others", "--exclude-standard", "-z"], "candidate-clean")
      .toString("utf8").split("\0").filter(Boolean),
  ];
  requireCondition(
    changed.length === 0,
    "identity.candidate",
    "candidate-clean",
    root,
    `uncommitted paths: ${changed.join(",")}`,
  );
  runGit(root, ["ls-files", "--error-unmatch", "--", "Cargo.lock"], "candidate-lock");
  const lockPath = assertNoSymlinkPath(root, "Cargo.lock", "candidate-lock");
  const lockBytes = readFileSync(lockPath);
  const committedLock = runGit(root, ["show", `${head}:Cargo.lock`], "candidate-lock");
  assertSameBytes(committedLock, lockBytes, "candidate-lock", "Cargo.lock");
  return {
    repoRoot: root,
    head,
    cargoLock: { path: lockPath, sha256: sha256(lockBytes), size: lockBytes.length },
  };
}

function parseJson(bytes, edge, path) {
  try {
    return JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    fail("identity.manifest", edge, path, error.message);
  }
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stableJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(stableJson).join(",")}]`;
  }
  if (isPlainObject(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function decodeXmlAttribute(value, edge, path) {
  requireCondition(!value.includes("<"), "identity.vsix_identity", edge, path, "markup in attribute");
  const unsupported = value.match(/&(?!(?:amp|quot|apos|lt|gt);)/u);
  requireCondition(unsupported === null, "identity.vsix_identity", edge, path, "unsupported entity");
  const decoded = value.replaceAll(/&(amp|quot|apos|lt|gt);/g, (_, entity) => {
    switch (entity) {
      case "amp": return "&";
      case "quot": return '"';
      case "apos": return "'";
      case "lt": return "<";
      case "gt": return ">";
      default: throw new Error(`unreachable XML entity ${entity}`);
    }
  });
  return decoded;
}

function parseXmlAttributes(source, edge, path) {
  const attributes = {};
  let cursor = 0;
  while (cursor < source.length) {
    while (/\s/u.test(source[cursor] ?? "")) cursor++;
    if (cursor === source.length) break;
    const name = /^[A-Za-z_][A-Za-z0-9_.:-]*/u.exec(source.slice(cursor));
    requireCondition(name !== null, "identity.vsix_identity", edge, path, "invalid attribute name");
    cursor += name[0].length;
    while (/\s/u.test(source[cursor] ?? "")) cursor++;
    requireCondition(source[cursor] === "=", "identity.vsix_identity", edge, path, `missing '=' after ${name[0]}`);
    cursor++;
    while (/\s/u.test(source[cursor] ?? "")) cursor++;
    const quote = source[cursor];
    requireCondition(quote === '"' || quote === "'", "identity.vsix_identity", edge, path, `unquoted ${name[0]}`);
    const end = source.indexOf(quote, cursor + 1);
    requireCondition(end >= 0, "identity.vsix_identity", edge, path, `unterminated ${name[0]}`);
    requireCondition(!Object.hasOwn(attributes, name[0]), "identity.vsix_identity", edge, path, `duplicate ${name[0]}`);
    attributes[name[0]] = decodeXmlAttribute(source.slice(cursor + 1, end), edge, `${path}.${name[0]}`);
    cursor = end + 1;
  }
  return attributes;
}

function parseVsixIdentity(manifestBytes) {
  let xml;
  try {
    xml = UTF8.decode(manifestBytes);
  } catch (error) {
    fail("identity.vsix_identity", "vsix", "extension.vsixmanifest", `invalid UTF-8: ${error.message}`);
  }
  requireCondition(
    !xml.includes("<!--") && !xml.includes("<![CDATA[") && !xml.includes("<!DOCTYPE"),
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest",
    "comments, CDATA, and doctypes are rejected",
  );
  const metadata = [...xml.matchAll(/<Metadata(?=[\s>])[^<>]*>([\s\S]*?)<\/Metadata\s*>/gu)];
  requireCondition(
    metadata.length === 1,
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest/Metadata",
    `expected one Metadata element, found ${metadata.length}`,
  );
  const starts = [...xml.matchAll(/<Identity(?=[\s/>])/gu)];
  requireCondition(
    starts.length === 1,
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest/Identity",
    `expected one Identity element, found ${starts.length}`,
  );
  const tag = /<Identity(?=[\s/>])([^<>]*?)\/>/gu.exec(metadata[0][1]);
  requireCondition(
    tag !== null,
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest/Identity",
    "Identity must be one self-closing element",
  );
  const attributes = parseXmlAttributes(tag[1], "vsix", "extension.vsixmanifest/Identity");
  const keys = Object.keys(attributes).sort((a, b) => a.localeCompare(b, "en-US"));
  requireCondition(
    stableJson(keys) === stableJson(VSIX_IDENTITY_KEYS),
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest/Identity",
    `unexpected attributes: ${keys.join(",")}`,
  );
  requireCondition(
    attributes.Language === "en-US",
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest/Identity.Language",
    String(attributes.Language),
  );
  return attributes;
}

export function assertVsixIdentity(vsix) {
  const map = surfaceMap(vsix);
  const manifest = map.get("extension.vsixmanifest");
  const packageJson = map.get("extension/package.json");
  requireCondition(manifest !== undefined, "identity.inventory", "vsix", "extension.vsixmanifest", "required entry absent");
  requireCondition(packageJson !== undefined, "identity.inventory", "vsix", "extension/package.json", "required entry absent");
  const expected = extensionIdentity(packageJson.data);
  const actual = parseVsixIdentity(manifest.data);
  for (const [attribute, value] of [
    ["Publisher", expected.publisher],
    ["Id", expected.name],
    ["Version", expected.version],
  ]) {
    requireCondition(
      actual[attribute] === value,
      "identity.vsix_identity",
      "vsix",
      `extension.vsixmanifest/Identity.${attribute}`,
      `${actual[attribute]} != ${value}`,
    );
  }
  requireCondition(
    actual.TargetPlatform === VSIX_TARGET_PLATFORM,
    "identity.vsix_identity",
    "vsix",
    "extension.vsixmanifest/Identity.TargetPlatform",
    `${actual.TargetPlatform} != ${VSIX_TARGET_PLATFORM}`,
  );
  return Object.freeze({
    publisher: actual.Publisher,
    id: actual.Id,
    version: actual.Version,
    targetPlatform: actual.TargetPlatform,
  });
}

export function normalizeInstalledPackage(packagedBytes, installedBytes) {
  const packaged = parseJson(packagedBytes, "vsix-install", "package.json");
  const installed = parseJson(installedBytes, "vsix-install", "package.json");
  requireCondition(isPlainObject(packaged) && isPlainObject(installed), "identity.installed_manifest", "vsix-install", "package.json", "object required");
  requireCondition(!Object.hasOwn(packaged, "__metadata"), "identity.installed_manifest", "vsix-install", "package.json", "packaged manifest already has __metadata");
  const packagedKeys = Object.keys(packaged).sort();
  const installedKeys = Object.keys(installed).filter((key) => key !== "__metadata").sort();
  requireCondition(
    stableJson(packagedKeys) === stableJson(installedKeys),
    "identity.installed_manifest",
    "vsix-install",
    "package.json",
    "only top-level __metadata may be added",
  );
  const metadata = installed.__metadata;
  requireCondition(isPlainObject(metadata), "identity.installed_manifest", "vsix-install", "package.json.__metadata", "object required");
  const metadataKeys = Object.keys(metadata).sort();
  requireCondition(
    stableJson(metadataKeys) === stableJson(["installedTimestamp", "size", "targetPlatform"]),
    "identity.installed_manifest",
    "vsix-install",
    "package.json.__metadata",
    `unexpected keys: ${metadataKeys.join(",")}`,
  );
  requireCondition(
    Number.isSafeInteger(metadata.installedTimestamp) && metadata.installedTimestamp > 0,
    "identity.installed_manifest",
    "vsix-install",
    "package.json.__metadata.installedTimestamp",
  );
  requireCondition(
    Number.isSafeInteger(metadata.size) && metadata.size > 0,
    "identity.installed_manifest",
    "vsix-install",
    "package.json.__metadata.size",
  );
  requireCondition(
    metadata.targetPlatform === "darwin-arm64" || metadata.targetPlatform === "undefined",
    "identity.installed_manifest",
    "vsix-install",
    "package.json.__metadata.targetPlatform",
    String(metadata.targetPlatform),
  );
  const normalized = { ...installed };
  delete normalized.__metadata;
  requireCondition(
    stableJson(normalized) === stableJson(packaged),
    "identity.installed_manifest",
    "vsix-install",
    "package.json",
    "manifest changed outside installed metadata",
  );
  return packaged;
}

function deepestNodePackageRoot(path) {
  const pieces = path.split("/");
  let start = -1;
  for (let i = 0; i < pieces.length; i++) {
    if (pieces[i] === "node_modules") {
      start = i;
    }
  }
  requireCondition(start >= 0 && start + 1 < pieces.length, "identity.inventory", "vsix", path, "invalid node_modules path");
  let end = start + 2;
  if (pieces[start + 1].startsWith("@")) {
    requireCondition(start + 2 < pieces.length, "identity.inventory", "vsix", path, "incomplete scoped package path");
    end++;
  }
  return pieces.slice(0, end).join("/");
}

function productionPackageRoots(lockBytes) {
  const lock = parseJson(lockBytes, "candidate-vsix", "editors/vscode/package-lock.json");
  requireCondition(isPlainObject(lock.packages), "identity.manifest", "candidate-vsix", "package-lock.json", "packages object absent");
  const roots = new Set();
  for (const [path, metadata] of Object.entries(lock.packages)) {
    if (path.startsWith("node_modules/") && metadata?.dev !== true) {
      validateRelativePath(path, "candidate-vsix");
      roots.add(path);
    }
  }
  requireCondition(roots.size > 0, "identity.inventory", "candidate-vsix", "package-lock.json", "no production packages");
  return roots;
}

function machOInfo(data) {
  if (data.length < 4) {
    return undefined;
  }
  const magic = data.subarray(0, 4).toString("hex");
  if (magic === "cffaedfe" || magic === "cefaedfe") {
    return {
      kind: magic === "cffaedfe" ? "thin64" : "thin32",
      endian: "little",
      cpuType: data.length >= 8 ? data.readUInt32LE(4) : undefined,
    };
  }
  if (magic === "feedfacf" || magic === "feedface") {
    return {
      kind: magic === "feedfacf" ? "thin64" : "thin32",
      endian: "big",
      cpuType: data.length >= 8 ? data.readUInt32BE(4) : undefined,
    };
  }
  if (["cafebabe", "bebafeca", "cafebabf", "bfbafeca"].includes(magic)) {
    return { kind: "fat", endian: "mixed", cpuType: undefined };
  }
  return undefined;
}

function assertMachOLaw(surface, edge = surface.name) {
  const map = surfaceMap(surface);
  const server = map.get(SERVER_PATH);
  requireCondition(server !== undefined, "identity.inventory", edge, SERVER_PATH, "server absent");
  const serverMachO = machOInfo(server.data);
  requireCondition(
    serverMachO?.kind === "thin64" && serverMachO.cpuType === ARM64_CPU_TYPE,
    "identity.macho",
    edge,
    SERVER_PATH,
    `expected thin arm64, got ${serverMachO ? `${serverMachO.kind}/${serverMachO.cpuType}` : "non-Mach-O"}`,
  );
  requireCondition(server.mode === 0o755, "identity.mode", edge, SERVER_PATH, `${server.mode.toString(8)} != 755`);
  const machos = surface.files.filter((file) => machOInfo(file.data) !== undefined);
  requireCondition(
    machos.length === 1 && machos[0].path === SERVER_PATH,
    "identity.macho",
    edge,
    SERVER_PATH,
    `Mach-O entries: ${machos.map((file) => file.path).join(",")}`,
  );
  for (const file of surface.files) {
    const expectedMode =
      file.path === SERVER_PATH || file.path === KNOWN_NONNATIVE_EXECUTABLE
        ? 0o755
        : 0o644;
    requireCondition(
      file.mode === expectedMode,
      "identity.mode",
      edge,
      file.path,
      `${file.mode.toString(8)} != ${expectedMode.toString(8)}`,
    );
    if ((file.mode & 0o111) === 0 || file.path === SERVER_PATH) {
      continue;
    }
    requireCondition(
      file.path === KNOWN_NONNATIVE_EXECUTABLE && machOInfo(file.data) === undefined,
      "identity.mode",
      edge,
      file.path,
      "unexpected executable entry",
    );
  }
}

function assertVsixInventory(vsix, packageLockBytes) {
  const map = surfaceMap(vsix);
  for (const path of REQUIRED_VSIX_ROOT_FILES) {
    requireCondition(map.has(path), "identity.inventory", "vsix", path, "required entry absent");
  }
  const identity = assertVsixIdentity(vsix);
  const packageRoots = productionPackageRoots(packageLockBytes);
  const extensionFiles = [];
  for (const entry of vsix.files) {
    if (REQUIRED_VSIX_ROOT_FILES.includes(entry.path)) {
      continue;
    }
    requireCondition(entry.path.startsWith("extension/"), "identity.inventory", "vsix", entry.path, "unexpected root entry");
    const path = entry.path.slice("extension/".length);
    if (path.startsWith("node_modules/")) {
      const root = deepestNodePackageRoot(path);
      requireCondition(packageRoots.has(root), "identity.inventory", "vsix", entry.path, `package ${root} is not a production lock entry`);
    } else {
      requireCondition(REQUIRED_EXTENSION_FILES.includes(path), "identity.inventory", "vsix", entry.path, "unapproved extension entry");
    }
    extensionFiles.push({ ...entry, path });
  }
  const extensionPaths = new Set(extensionFiles.map((file) => file.path));
  for (const path of REQUIRED_EXTENSION_FILES) {
    requireCondition(extensionPaths.has(path), "identity.inventory", "vsix", `extension/${path}`, "required entry absent");
  }
  for (const root of packageRoots) {
    requireCondition(
      extensionPaths.has(`${root}/package.json`),
      "identity.inventory",
      "vsix",
      `extension/${root}/package.json`,
      "production package absent",
    );
  }
  const payload = makeFileSet("vsix", extensionFiles, {
    archivePath: vsix.path,
    outerSha256: vsix.outerSha256,
    archiveManifest: publicManifest(vsix),
    identity,
  });
  assertMachOLaw(payload, "vsix");
  return payload;
}

export function verifyVsix({ vsixPath, packageLockPath }) {
  const lockPath = canonicalExistingPath(packageLockPath, "file", "candidate-vsix");
  const archive = readVsix(vsixPath);
  const payload = assertVsixInventory(archive, readFileSync(lockPath));
  return { archive, payload, identity: payload.metadata.identity };
}

function compareRecords(left, right, edge, path) {
  requireCondition(left.sha256 === right.sha256, "identity.digest", edge, path, `${left.sha256} != ${right.sha256}`);
  requireCondition(
    left.mode === right.mode,
    "identity.mode",
    edge,
    path,
    `${left.mode.toString(8)} != ${right.mode.toString(8)}`,
  );
}

export function compareEdge(left, right, edge) {
  const leftMap = surfaceMap(left);
  const rightMap = surfaceMap(right);
  const leftPaths = [...leftMap.keys()].sort();
  const rightPaths = [...rightMap.keys()].sort();
  requireCondition(
    stableJson(leftPaths) === stableJson(rightPaths),
    "identity.inventory",
    edge,
    "*",
    `left=${leftPaths.join(",")} right=${rightPaths.join(",")}`,
  );
  for (const path of leftPaths) {
    compareRecords(leftMap.get(path), rightMap.get(path), edge, path);
  }
}

function projectStage({ repoRoot, stageRoot, canonicalServerPath, vsixPayload }) {
  const repo = canonicalExistingPath(repoRoot, "directory", "candidate-stage");
  const stage = canonicalExistingPath(stageRoot, "directory", "candidate-stage");
  requireOutside(stage, repo, "candidate-stage");
  const editorRoot = join(repo, "editors", "vscode");
  canonicalExistingPath(editorRoot, "directory", "candidate-stage");
  for (const input of CANDIDATE_STAGE_INPUTS) {
    const candidate = fileRecord(editorRoot, input, input, "candidate-stage");
    const staged = fileRecord(stage, input, input, "candidate-stage");
    compareRecords(candidate, staged, "candidate-stage", input);
  }
  const canonicalServerRoot = dirname(canonicalServerPath);
  const canonicalServer = fileRecord(canonicalServerRoot, "marrow-lsp", SERVER_PATH, "canonical-stage");
  const stagedServer = fileRecord(stage, SERVER_PATH, SERVER_PATH, "canonical-stage");
  requireDistinctInodes(canonicalServer, stagedServer, "canonical-stage", SERVER_PATH);
  compareRecords(canonicalServer, stagedServer, "canonical-stage", SERVER_PATH);
  assertMachOLaw(makeFileSet("canonical", [canonicalServer]), "canonical");

  const stagedPayload = makeFileSet(
    "stage",
    vsixPayload.files.map((packaged) => {
      const stagePath = VSIX_STAGE_ALIASES.get(packaged.path) ?? packaged.path;
      return fileRecord(stage, stagePath, packaged.path, "stage-vsix");
    }),
    { root: stage },
  );
  assertMachOLaw(stagedPayload, "stage");
  compareEdge(stagedPayload, vsixPayload, "stage-vsix");
  return { stage: stagedPayload, canonical: makeFileSet("canonical", [canonicalServer]) };
}

function parseInstallLocation(record, extensionsRoot) {
  const candidates = [];
  if (typeof record.relativeLocation === "string") {
    try {
      validateRelativePath(record.relativeLocation, "install-resolution");
    } catch (error) {
      if (error instanceof IdentityError) {
        fail(
          "identity.install_resolution",
          "install-resolution",
          record.relativeLocation,
          error.detail,
        );
      }
      throw error;
    }
    candidates.push(resolve(extensionsRoot, record.relativeLocation));
  }
  const location = record.location;
  if (isPlainObject(location)) {
    if (typeof location.fsPath === "string") {
      candidates.push(resolve(location.fsPath));
    }
    if (typeof location.path === "string") {
      candidates.push(resolve(location.path));
    }
    if (typeof location.external === "string") {
      try {
        const url = new URL(location.external);
        requireCondition(url.protocol === "file:", "identity.install_resolution", "install-resolution", location.external, "non-file URI rejected");
        candidates.push(resolve(fileURLToPath(url)));
      } catch (error) {
        if (error instanceof IdentityError) throw error;
        fail("identity.install_resolution", "install-resolution", location.external, error.message);
      }
    }
  }
  requireCondition(candidates.length > 0, "identity.install_resolution", "install-resolution", "extensions.json", "record has no location");
  const first = candidates[0];
  requireCondition(
    candidates.every((candidate) => candidate === first),
    "identity.install_resolution",
    "install-resolution",
    "extensions.json",
    `location fields disagree: ${candidates.join(",")}`,
  );
  return first;
}

export function resolveInstalledExtension({
  extensionsDir,
  extensionId,
  version,
  forbiddenRoots = [],
}) {
  const root = canonicalExistingPath(extensionsDir, "directory", "install-resolution");
  const indexPath = assertNoSymlinkPath(root, "extensions.json", "install-resolution");
  const indexInfo = statSync(indexPath);
  const indexBytes = readFileSync(indexPath);
  requireCondition(
    (indexInfo.mode & 0o777) === 0o644,
    "identity.mode",
    "install-resolution",
    "extensions.json",
    `${(indexInfo.mode & 0o777).toString(8)} != 644`,
  );
  const index = parseJson(indexBytes, "install-resolution", "extensions.json");
  requireCondition(Array.isArray(index), "identity.install_resolution", "install-resolution", "extensions.json", "array required");
  const matches = index.filter(
    (entry) => entry?.identifier?.id === extensionId && entry?.version === version,
  );
  requireCondition(
    matches.length === 1,
    "identity.install_resolution",
    "install-resolution",
    extensionId,
    `expected one ${version} record, found ${matches.length}`,
  );
  requireCondition(
    matches[0]?.metadata?.source === "vsix",
    "identity.install_resolution",
    "install-resolution",
    extensionId,
    "record is not a VSIX install",
  );
  const unresolved = parseInstallLocation(matches[0], root);
  requireCondition(isWithin(unresolved, root) && unresolved !== root, "identity.install_resolution", "install-resolution", unresolved, "outside isolated extensions directory");
  const installedRoot = canonicalExistingPath(unresolved, "directory", "install-resolution");
  requireCondition(isWithin(installedRoot, root), "identity.install_resolution", "install-resolution", installedRoot, "real path escapes isolated extensions directory");
  for (const forbidden of forbiddenRoots) {
    const canonicalForbidden = realpathSync(forbidden);
    requireOutside(installedRoot, canonicalForbidden, "install-resolution");
  }
  const selected = matches[0];
  const selectedRecord = Buffer.from(stableJson(selected), "utf8");
  return {
    installedRoot,
    record: selected,
    extensionsDir: root,
    index: {
      path: indexPath,
      sha256: sha256(indexBytes),
      mode: indexInfo.mode & 0o777,
      size: indexBytes.length,
      device: indexInfo.dev,
      inode: indexInfo.ino,
      selectedRecordSha256: sha256(selectedRecord),
      selected: {
        extensionId,
        version,
        source: selected.metadata.source,
        installedRoot,
      },
    },
  };
}

function projectedInstalledPath(archivePath) {
  if (archivePath === "[Content_Types].xml") {
    return undefined;
  }
  if (archivePath === "extension.vsixmanifest") {
    return ".vsixmanifest";
  }
  requireCondition(archivePath.startsWith("extension/"), "identity.inventory", "vsix-install", archivePath);
  return archivePath.slice("extension/".length);
}

function compareVsixInstall({ archive, vsixPayload, installedTree, stage }) {
  const installedMap = surfaceMap(installedTree);
  const actualManifest = publicManifest(installedTree);
  const expectedPaths = archive.files.map((file) => projectedInstalledPath(file.path)).filter(Boolean).sort();
  const actualPaths = installedTree.files.map((file) => file.path).sort();
  requireCondition(
    stableJson(expectedPaths) === stableJson(actualPaths),
    "identity.inventory",
    "vsix-install",
    "*",
    `expected=${expectedPaths.join(",")} actual=${actualPaths.join(",")}`,
  );
  const installedPayloadFiles = [];
  for (const packaged of archive.files) {
    const path = projectedInstalledPath(packaged.path);
    if (path === undefined) continue;
    const installed = installedMap.get(path);
    requireCondition(installed !== undefined, "identity.inventory", "vsix-install", path, "installed entry absent");
    if (path === "package.json") {
      normalizeInstalledPackage(packaged.data, installed.data);
      compareRecords(
        { ...packaged, sha256: installed.sha256 },
        installed,
        "vsix-install",
        path,
      );
      installedPayloadFiles.push({
        ...installed,
        path,
        data: packaged.data,
        sha256: packaged.sha256,
        size: packaged.size,
        actualSha256: installed.sha256,
        normalizedSha256: packaged.sha256,
      });
    } else {
      compareRecords(packaged, installed, "vsix-install", path);
      installedPayloadFiles.push({ ...installed, path });
    }
  }
  const installedPayload = makeFileSet("installed", installedPayloadFiles, {
    root: installedTree.root,
    actualManifest,
    inodeDisjointFromStageEntries: vsixPayload.files.length,
  });
  assertMachOLaw(installedPayload, "installed");
  const stageMap = surfaceMap(stage);
  const installedPayloadMap = surfaceMap(installedPayload);
  for (const path of vsixPayload.files.map((file) => file.path)) {
    const staged = stageMap.get(path);
    const installed = installedPayloadMap.get(path);
    requireDistinctInodes(staged, installed, "vsix-install", path);
  }
  return installedPayload;
}

function extensionIdentity(packageBytes) {
  const manifest = parseJson(packageBytes, "candidate", "editors/vscode/package.json");
  requireCondition(
    typeof manifest.publisher === "string" && manifest.publisher.length > 0 &&
      typeof manifest.name === "string" && manifest.name.length > 0 &&
      typeof manifest.version === "string" && manifest.version.length > 0,
    "identity.manifest",
    "candidate",
    "editors/vscode/package.json",
    "publisher, name, and version are required",
  );
  return {
    publisher: manifest.publisher,
    name: manifest.name,
    extensionId: `${manifest.publisher}.${manifest.name}`,
    version: manifest.version,
  };
}

export function buildArtifactSets(options) {
  const { repoRoot, expectedHead, targetDir, stageRoot, vsixPath, extensionsDir } = options;
  const authority = assertCandidateAuthority({ repoRoot, expectedHead });
  const target = canonicalExistingPath(targetDir, "directory", "canonical");
  requireOutside(target, authority.repoRoot, "canonical");
  const stagePath = canonicalExistingPath(stageRoot, "directory", "candidate-stage");
  requireDisjointTrees(stagePath, target, "canonical-stage");
  const canonicalServerPath = assertNoSymlinkPath(target, "release/marrow-lsp", "canonical");
  const editorRoot = join(authority.repoRoot, "editors", "vscode");
  const packageLockPath = join(editorRoot, "package-lock.json");
  const { archive, payload: vsix, identity: vsixIdentity } = verifyVsix({ vsixPath, packageLockPath });
  const { stage, canonical } = projectStage({
    repoRoot: authority.repoRoot,
    stageRoot: stagePath,
    canonicalServerPath,
    vsixPayload: vsix,
  });
  const install = resolveInstalledExtension({
    extensionsDir,
    extensionId: `${vsixIdentity.publisher}.${vsixIdentity.id}`,
    version: vsixIdentity.version,
    forbiddenRoots: [authority.repoRoot, stage.metadata.root],
  });
  const installedTree = readTreeManifest(install.installedRoot, { name: "installed-tree" });
  const installed = compareVsixInstall({ archive, vsixPayload: vsix, installedTree, stage });
  const installedRecord = installedProbeRecord(installedTree);
  return {
    authority,
    canonical,
    stage,
    archive,
    vsix,
    vsixIdentity,
    install,
    installed,
    installedRecord,
  };
}

export function verifyArtifactChain(options) {
  const result = buildArtifactSets(options);
  return {
    ...result,
    evidence: {
      candidate: {
        head: result.authority.head,
        cargoLock: result.authority.cargoLock,
      },
      canonical: publicManifest(result.canonical),
      stage: publicManifest(result.stage),
      vsix: {
        path: result.archive.path,
        outerSha256: result.archive.outerSha256,
        identity: result.vsixIdentity,
        provenance: {
          device: result.archive.device,
          inode: result.archive.inode,
          mode: result.archive.mode,
          size: result.archive.size,
        },
        manifest: publicManifest(result.archive),
      },
      installed: {
        root: result.install.installedRoot,
        extensionsIndex: result.install.index,
        actualManifest: result.installed.metadata.actualManifest,
        normalizedManifest: publicManifest(result.installed),
        inodeDisjointFromStageEntries:
          result.installed.metadata.inodeDisjointFromStageEntries,
      },
    },
  };
}

export function compareDualBuilds(left, right) {
  requireCondition(
    isPlainObject(left?.archive) && isPlainObject(right?.archive),
    "identity.dual_build",
    "vsix-vsix",
    "provenance",
    "two complete verified build results are required",
  );
  const leftArchive = left.archive;
  const rightArchive = right.archive;
  requireCondition(
    typeof leftArchive.path === "string" && typeof rightArchive.path === "string" &&
      leftArchive.path !== rightArchive.path &&
      (leftArchive.device !== rightArchive.device || leftArchive.inode !== rightArchive.inode),
    "identity.dual_build",
    "vsix-vsix",
    "provenance",
    "build archives must be distinct paths and inodes",
  );
  const leftManifest = publicManifest(leftArchive);
  const rightManifest = publicManifest(rightArchive);
  requireCondition(
    stableJson(leftManifest) === stableJson(rightManifest),
    "identity.dual_build",
    "vsix-vsix",
    "*",
    "sorted per-entry manifests differ",
  );
  return {
    manifest: leftManifest,
    builds: [leftArchive, rightArchive].map((archive) => ({
      path: archive.path,
      outerSha256: archive.outerSha256,
      device: archive.device,
      inode: archive.inode,
      mode: archive.mode,
      size: archive.size,
    })),
  };
}

function modelFileSet(name) {
  const hash = (character) => character.repeat(64);
  return makeFileSet(name, [
    { path: SERVER_PATH, sha256: hash("a"), mode: 0o755, size: 1, data: Buffer.from("server") },
    { path: "out/extension.js", sha256: hash("b"), mode: 0o644, size: 1, data: Buffer.from("bundle") },
    { path: "syntaxes/marrow.tmLanguage.json", sha256: hash("c"), mode: 0o644, size: 1, data: Buffer.from("grammar") },
  ]);
}

function cloneFileSet(surface, name = surface.name) {
  return makeFileSet(name, surface.files.map((file) => ({ ...file, data: Buffer.from(file.data) })), { ...surface.metadata });
}

function mutateRecord(surface, path, changes) {
  const record = surface.files.find((file) => file.path === path);
  if (record === undefined) throw new Error(`fault setup: missing ${path}`);
  Object.assign(record, changes);
}

function compareArtifactModels({ canonical, stage, vsix, installed }) {
  compareEdge(canonical, stage, "canonical-stage");
  compareEdge(stage, vsix, "stage-vsix");
  compareEdge(vsix, installed, "vsix-install");
}

function expectFault(results, name, callback, expected) {
  let caught;
  try {
    callback();
  } catch (error) {
    caught = error;
  }
  requireCondition(caught instanceof IdentityError, "identity.fault_matrix", "self-test", name, `expected IdentityError, got ${caught ?? "success"}`);
  requireCondition(
    caught.code === expected.code && caught.edge === expected.edge && caught.path === expected.path,
    "identity.fault_matrix",
    "self-test",
    name,
    `got ${caught.code}/${caught.edge}/${caught.path}`,
  );
  results.push({ name, code: caught.code, edge: caught.edge, path: caught.path });
}

function syntheticMachO(cpuType = ARM64_CPU_TYPE) {
  const data = Buffer.alloc(16);
  data.writeUInt32LE(0xfeedfacf, 0);
  data.writeUInt32LE(cpuType, 4);
  return data;
}

function inMemoryRecord(path, data, mode = 0o644) {
  const bytes = Buffer.from(data);
  return {
    path,
    data: bytes,
    sha256: sha256(bytes),
    mode,
    size: bytes.length,
  };
}

function modelVsixIdentity({ publisher = "marrow-project", targetPlatform = VSIX_TARGET_PLATFORM } = {}) {
  const packageJson = Buffer.from(JSON.stringify({
    publisher: "marrow-project",
    name: "marrow",
    version: "0.1.1",
  }));
  const manifest = Buffer.from(
    `<PackageManifest><Metadata><Identity Language="en-US" Id="marrow" Version="0.1.1" Publisher="${publisher}" TargetPlatform="${targetPlatform}"/></Metadata></PackageManifest>`,
  );
  return makeFileSet("vsix-archive", [
    inMemoryRecord("extension.vsixmanifest", manifest),
    inMemoryRecord("extension/package.json", packageJson),
  ]);
}

function modelVerifiedBuild(path, device, inode) {
  const archive = cloneFileSet(modelFileSet("vsix-archive"), "vsix-archive");
  return { archive: { ...archive, path, device, inode } };
}

const SELF_TEST_WAIT_CELL = new Int32Array(new SharedArrayBuffer(4));

function waitForSelfTestPath(path, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (lstatIfPresent(path) !== undefined) return;
    Atomics.wait(SELF_TEST_WAIT_CELL, 0, 0, 5);
  }
  fail("identity.evidence_self_test", "self-test", path, `not observed within ${timeoutMs} ms`);
}

function assertNoEvidenceTemps(parent, destinationName) {
  const prefix = `.${destinationName}.marrow-evidence-`;
  const leftovers = readdirSync(parent).filter((name) => name.startsWith(prefix));
  requireCondition(
    leftovers.length === 0,
    "identity.evidence_self_test",
    "self-test",
    destinationName,
    `temporary leftovers: ${leftovers.join(",")}`,
  );
}

/** Production-path faults for the shared bounded evidence publisher. */
export function runEvidencePublisherSelfTests() {
  const results = [];
  const root = realpathSync(mkdtempSync(join(tmpdir(), "marrow-evidence-publisher-")));
  let racer;
  try {
    const outputParent = join(root, "retained");
    const forbidden = join(root, "forbidden");
    mkdirSync(outputParent, { mode: 0o700 });
    mkdirSync(forbidden, { mode: 0o700 });

    const happyPath = join(outputParent, "happy.json");
    const happyBytes = Buffer.from('{"result":"PASS"}\n');
    const receipt = publishEvidence({
      destination: happyPath,
      bytes: happyBytes,
      forbiddenRoots: [forbidden],
    });
    const happyInfo = lstatSync(happyPath);
    requireCondition(
      receipt.path === happyPath && receipt.bytes === happyBytes.length &&
        receipt.sha256 === sha256(happyBytes) && receipt.mode === 0o600 &&
        happyInfo.isFile() && !happyInfo.isSymbolicLink() &&
        (happyInfo.mode & 0o777) === 0o600 && readFileSync(happyPath).equals(happyBytes),
      "identity.evidence_self_test",
      "self-test",
      happyPath,
      "happy-path receipt or file mismatch",
    );
    assertNoEvidenceTemps(outputParent, basename(happyPath));

    const exactBoundPath = join(outputParent, "exact-bound.bin");
    const exactBoundBytes = Buffer.alloc(MAX_EVIDENCE_BYTES, 0x61);
    const exactBound = publishEvidence({
      destination: exactBoundPath,
      bytes: exactBoundBytes,
      forbiddenRoots: [forbidden],
    });
    requireCondition(
      exactBound.bytes === MAX_EVIDENCE_BYTES &&
        statSync(exactBoundPath).size === MAX_EVIDENCE_BYTES,
      "identity.evidence_self_test",
      "self-test",
      exactBoundPath,
      "exact upper bound rejected",
    );
    assertNoEvidenceTemps(outputParent, basename(exactBoundPath));

    expectFault(results, "relative evidence destination", () => publishEvidence({
      destination: "relative-evidence.json",
      bytes: happyBytes,
      forbiddenRoots: [forbidden],
    }), {
      code: "identity.evidence", edge: "evidence-publish", path: "relative-evidence.json",
    });

    const linkedParent = join(root, "linked-parent");
    symlinkSync(outputParent, linkedParent);
    expectFault(results, "symlink evidence parent", () => publishEvidence({
      destination: join(linkedParent, "evidence.json"),
      bytes: happyBytes,
      forbiddenRoots: [forbidden],
    }), {
      code: "identity.path", edge: "evidence-publish", path: linkedParent,
    });

    expectFault(results, "existing evidence destination", () => publishEvidence({
      destination: happyPath,
      bytes: Buffer.from("replacement"),
      forbiddenRoots: [forbidden],
    }), {
      code: "identity.evidence", edge: "evidence-publish", path: happyPath,
    });
    requireCondition(
      readFileSync(happyPath).equals(happyBytes),
      "identity.evidence_self_test",
      "self-test",
      happyPath,
      "existing destination changed",
    );

    const forbiddenPath = join(forbidden, "evidence.json");
    expectFault(results, "forbidden evidence root", () => publishEvidence({
      destination: forbiddenPath,
      bytes: happyBytes,
      forbiddenRoots: [forbidden],
    }), {
      code: "identity.path", edge: "evidence-publish", path: forbiddenPath,
    });

    const oversizedPath = join(outputParent, "oversized.bin");
    expectFault(results, "evidence byte bound", () => publishEvidence({
      destination: oversizedPath,
      bytes: Buffer.alloc(MAX_EVIDENCE_BYTES + 1),
      forbiddenRoots: [forbidden],
    }), {
      code: "identity.evidence_limit", edge: "evidence-publish", path: oversizedPath,
    });
    requireCondition(
      lstatIfPresent(oversizedPath) === undefined,
      "identity.evidence_self_test",
      "self-test",
      oversizedPath,
      "oversized destination was created",
    );
    assertNoEvidenceTemps(outputParent, basename(oversizedPath));

    const racePath = join(outputParent, "race.json");
    const readyPath = join(root, "racer.ready");
    const donePath = join(root, "racer.done");
    const racerScript = join(root, "racer.cjs");
    writeFileSync(
      racerScript,
      [
        'const { readdirSync, writeFileSync } = require("node:fs");',
        'const { basename } = require("node:path");',
        'const [parent, destination, ready, done] = process.argv.slice(2);',
        'const prefix = `.${basename(destination)}.marrow-evidence-`;',
        'writeFileSync(ready, "ready", { flag: "wx", mode: 0o600 });',
        'const deadline = Date.now() + 10000;',
        'let result = "timeout";',
        'while (Date.now() < deadline) {',
        '  if (!readdirSync(parent).some((name) => name.startsWith(prefix))) continue;',
        '  try {',
        '    writeFileSync(destination, "racer", { flag: "wx", mode: 0o600 });',
        '    result = "won";',
        '  } catch (error) { result = error.code ?? "error"; }',
        '  break;',
        '}',
        'writeFileSync(done, result, { flag: "wx", mode: 0o600 });',
      ].join("\n"),
      { mode: 0o600, flag: "wx" },
    );
    racer = spawn(
      process.execPath,
      [racerScript, outputParent, racePath, readyPath, donePath],
      { stdio: "ignore" },
    );
    waitForSelfTestPath(readyPath, 5_000);
    expectFault(results, "evidence publication race", () => publishEvidence({
      destination: racePath,
      bytes: Buffer.alloc(MAX_EVIDENCE_BYTES, 0x62),
      forbiddenRoots: [forbidden],
    }), {
      code: "identity.evidence", edge: "evidence-publish", path: racePath,
    });
    waitForSelfTestPath(donePath, 5_000);
    requireCondition(
      readFileSync(donePath, "utf8") === "won" &&
        readFileSync(racePath, "utf8") === "racer",
      "identity.evidence_self_test",
      "self-test",
      racePath,
      "racing destination was overwritten",
    );
    assertNoEvidenceTemps(outputParent, basename(racePath));

    return results;
  } finally {
    if (racer !== undefined && racer.exitCode === null) racer.kill("SIGKILL");
    rmSync(root, { recursive: true, force: true });
  }
}

/** Focused model faults for every identity edge and normalized dimension. */
export function runFaultMatrix() {
  const results = [];
  const canonical = modelFileSet("canonical");
  const stage = cloneFileSet(canonical, "stage");
  const vsix = cloneFileSet(canonical, "vsix");
  const installed = cloneFileSet(canonical, "installed");
  compareArtifactModels({ canonical, stage, vsix, installed });

  const staleStage = cloneFileSet(stage);
  const staleVsix = cloneFileSet(vsix);
  const staleInstalled = cloneFileSet(installed);
  for (const surface of [staleStage, staleVsix, staleInstalled]) {
    mutateRecord(surface, SERVER_PATH, { sha256: "d".repeat(64) });
  }
  expectFault(results, "stale agreeing chain", () => compareArtifactModels({ canonical, stage: staleStage, vsix: staleVsix, installed: staleInstalled }), {
    code: "identity.digest", edge: "canonical-stage", path: SERVER_PATH,
  });

  const stalePackaged = cloneFileSet(vsix);
  const stalePackageInstall = cloneFileSet(installed);
  for (const surface of [stalePackaged, stalePackageInstall]) {
    mutateRecord(surface, "out/extension.js", { sha256: "e".repeat(64) });
  }
  expectFault(results, "stage to VSIX", () => compareArtifactModels({ canonical, stage, vsix: stalePackaged, installed: stalePackageInstall }), {
    code: "identity.digest", edge: "stage-vsix", path: "out/extension.js",
  });

  const staleInstall = cloneFileSet(installed);
  mutateRecord(staleInstall, "syntaxes/marrow.tmLanguage.json", { sha256: "f".repeat(64) });
  expectFault(results, "VSIX to install", () => compareArtifactModels({ canonical, stage, vsix, installed: staleInstall }), {
    code: "identity.digest", edge: "vsix-install", path: "syntaxes/marrow.tmLanguage.json",
  });

  const extraVsix = cloneFileSet(vsix);
  extraVsix.files.push({ path: "extra", sha256: "1".repeat(64), mode: 0o644, size: 1, data: Buffer.from("x") });
  const extraInstall = cloneFileSet(extraVsix, "installed");
  expectFault(results, "extra inventory", () => compareArtifactModels({ canonical, stage, vsix: extraVsix, installed: extraInstall }), {
    code: "identity.inventory", edge: "stage-vsix", path: "*",
  });

  const missingInstall = cloneFileSet(installed);
  missingInstall.files = missingInstall.files.filter((file) => file.path !== "out/extension.js");
  expectFault(results, "missing inventory", () => compareArtifactModels({ canonical, stage, vsix, installed: missingInstall }), {
    code: "identity.inventory", edge: "vsix-install", path: "*",
  });

  const modeStage = cloneFileSet(stage);
  const modeVsix = cloneFileSet(vsix);
  const modeInstall = cloneFileSet(installed);
  for (const surface of [modeStage, modeVsix, modeInstall]) mutateRecord(surface, SERVER_PATH, { mode: 0o644 });
  expectFault(results, "mode", () => compareArtifactModels({ canonical, stage: modeStage, vsix: modeVsix, installed: modeInstall }), {
    code: "identity.mode", edge: "canonical-stage", path: SERVER_PATH,
  });

  const ordinaryVsixMode = cloneFileSet(vsix);
  mutateRecord(ordinaryVsixMode, "out/extension.js", { mode: 0o664 });
  expectFault(results, "stage ordinary mode", () => compareArtifactModels({
    canonical,
    stage,
    vsix: ordinaryVsixMode,
    installed,
  }), {
    code: "identity.mode", edge: "stage-vsix", path: "out/extension.js",
  });

  const ordinaryInstalledMode = cloneFileSet(installed);
  mutateRecord(ordinaryInstalledMode, "out/extension.js", { mode: 0o664 });
  expectFault(results, "installed ordinary mode", () => compareArtifactModels({
    canonical,
    stage,
    vsix,
    installed: ordinaryInstalledMode,
  }), {
    code: "identity.mode", edge: "vsix-install", path: "out/extension.js",
  });

  const armServer = { path: SERVER_PATH, data: syntheticMachO(), sha256: "a".repeat(64), mode: 0o755, size: 16 };
  const wrongArch = { ...armServer, data: syntheticMachO(0x01000007) };
  expectFault(results, "wrong Mach-O architecture", () => assertMachOLaw(makeFileSet("macho", [wrongArch]), "macho"), {
    code: "identity.macho", edge: "macho", path: SERVER_PATH,
  });
  const secondMachO = { path: "other", data: syntheticMachO(), sha256: "b".repeat(64), mode: 0o644, size: 16 };
  expectFault(results, "second Mach-O", () => assertMachOLaw(makeFileSet("macho", [armServer, secondMachO]), "macho"), {
    code: "identity.macho", edge: "macho", path: SERVER_PATH,
  });
  const worldWritable = {
    path: "out/extension.js",
    data: Buffer.from("bundle"),
    sha256: "c".repeat(64),
    mode: 0o666,
    size: 6,
  };
  expectFault(results, "ordinary mode", () => assertMachOLaw(makeFileSet("installed", [armServer, worldWritable]), "installed"), {
    code: "identity.mode", edge: "installed", path: "out/extension.js",
  });

  const packagedManifest = Buffer.from('{"name":"marrow","version":"0.1.1"}\n');
  const validInstalledManifest = Buffer.from('{"name":"marrow","version":"0.1.1","__metadata":{"installedTimestamp":1,"size":1,"targetPlatform":"darwin-arm64"}}\n');
  normalizeInstalledPackage(packagedManifest, validInstalledManifest);
  const invalidManifest = Buffer.from('{"name":"marrow","version":"0.1.1","unexpected":true,"__metadata":{"installedTimestamp":1,"size":1,"targetPlatform":"darwin-arm64"}}\n');
  expectFault(results, "installed manifest delta", () => normalizeInstalledPackage(packagedManifest, invalidManifest), {
    code: "identity.installed_manifest", edge: "vsix-install", path: "package.json",
  });
  const invalidMetadata = Buffer.from('{"name":"marrow","version":"0.1.1","__metadata":{"installedTimestamp":1,"size":1,"targetPlatform":"darwin-arm64","other":true}}\n');
  expectFault(results, "installed metadata shape", () => normalizeInstalledPackage(packagedManifest, invalidMetadata), {
    code: "identity.installed_manifest", edge: "vsix-install", path: "package.json.__metadata",
  });

  expectFault(results, "candidate HEAD", () => assertExpectedHead("a".repeat(40), "b".repeat(40)), {
    code: "identity.candidate", edge: "candidate-head", path: "HEAD",
  });
  expectFault(results, "candidate lock", () => assertSameBytes(Buffer.from("a"), Buffer.from("b"), "candidate-lock", "Cargo.lock"), {
    code: "identity.digest", edge: "candidate-lock", path: "Cargo.lock",
  });
  expectFault(results, "candidate helper timeout", () => runBoundedSync(
    process.execPath,
    ["-e", "setInterval(() => {}, 1_000)"],
    { encoding: null, maxBuffer: 1024, stdio: ["ignore", "pipe", "pipe"] },
    {
      code: "identity.candidate",
      edge: "candidate-helper",
      path: "timeout",
      timeoutMs: 10,
    },
  ), {
    code: "identity.candidate", edge: "candidate-helper", path: "timeout",
  });

  const temporary = realpathSync(mkdtempSync(join(tmpdir(), "marrow-artifact-identity-")));
  try {
    const extensions = join(temporary, "extensions");
    const installedPath = join(extensions, "marrow-project.marrow-0.1.1");
    mkdirSync(installedPath, { recursive: true });
    writeFileSync(join(installedPath, "package.json"), packagedManifest);
    const record = {
      identifier: { id: "marrow-project.marrow" },
      version: "0.1.1",
      relativeLocation: "marrow-project.marrow-0.1.1",
      metadata: { source: "vsix" },
    };
    writeFileSync(join(extensions, "extensions.json"), JSON.stringify([record]));
    resolveInstalledExtension({ extensionsDir: extensions, extensionId: "marrow-project.marrow", version: "0.1.1" });
    writeFileSync(join(extensions, "extensions.json"), JSON.stringify([record, record]));
    expectFault(results, "duplicate install record", () => resolveInstalledExtension({ extensionsDir: extensions, extensionId: "marrow-project.marrow", version: "0.1.1" }), {
      code: "identity.install_resolution", edge: "install-resolution", path: "marrow-project.marrow",
    });
    const outsidePath = join(temporary, "outside");
    mkdirSync(outsidePath);
    writeFileSync(join(extensions, "extensions.json"), JSON.stringify([{ ...record, relativeLocation: "../outside" }]));
    expectFault(results, "outside install root", () => resolveInstalledExtension({ extensionsDir: extensions, extensionId: "marrow-project.marrow", version: "0.1.1" }), {
      code: "identity.install_resolution", edge: "install-resolution", path: "../outside",
    });
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }

  assertVsixIdentity(modelVsixIdentity());
  expectFault(results, "VSIX identity", () => assertVsixIdentity(modelVsixIdentity({ publisher: "other" })), {
    code: "identity.vsix_identity", edge: "vsix", path: "extension.vsixmanifest/Identity.Publisher",
  });
  expectFault(results, "VSIX TargetPlatform", () => assertVsixIdentity(modelVsixIdentity({ targetPlatform: "darwin-x64" })), {
    code: "identity.vsix_identity", edge: "vsix", path: "extension.vsixmanifest/Identity.TargetPlatform",
  });

  const firstBuild = modelVerifiedBuild("/tmp/first.vsix", 1, 1);
  const secondBuild = modelVerifiedBuild("/tmp/second.vsix", 1, 2);
  compareDualBuilds(firstBuild, secondBuild);
  const changedBuild = modelVerifiedBuild("/tmp/changed.vsix", 1, 3);
  mutateRecord(changedBuild.archive, "out/extension.js", { sha256: "9".repeat(64) });
  expectFault(results, "dual build manifest", () => compareDualBuilds(firstBuild, changedBuild), {
    code: "identity.dual_build", edge: "vsix-vsix", path: "*",
  });
  const aliasedBuild = modelVerifiedBuild("/tmp/aliased.vsix", 1, 1);
  expectFault(results, "dual build provenance", () => compareDualBuilds(firstBuild, aliasedBuild), {
    code: "identity.dual_build", edge: "vsix-vsix", path: "provenance",
  });

  expectFault(results, "stage aliases target", () => requireDistinctInodes(
    { device: 1, inode: 1 },
    { device: 1, inode: 1 },
    "canonical-stage",
    SERVER_PATH,
  ), {
    code: "identity.alias", edge: "canonical-stage", path: SERVER_PATH,
  });
  expectFault(results, "install aliases stage", () => requireDistinctInodes(
    { device: 1, inode: 2 },
    { device: 1, inode: 2 },
    "vsix-install",
    "package.json",
  ), {
    code: "identity.alias", edge: "vsix-install", path: "package.json",
  });
  requireCondition(
    stableJson(results.map(({ name }) => name)) === stableJson(ARTIFACT_FAULT_NAMES),
    "identity.fault_matrix",
    "self-test",
    "fault inventory",
    `expected=${ARTIFACT_FAULT_NAMES.join(",")} actual=${results.map(({ name }) => name).join(",")}`,
  );
  return results;
}

const THIS_FILE = fileURLToPath(import.meta.url);
if (process.argv[1] !== undefined && resolve(process.argv[1]) === THIS_FILE) {
  try {
    const results = runFaultMatrix();
    for (const result of results) {
      console.log(`PASS ${result.name}: ${result.code}/${result.edge}/${result.path}`);
    }
    console.log(`fault matrix: ${results.length} biting faults passed`);
    const evidenceResults = runEvidencePublisherSelfTests();
    for (const result of evidenceResults) {
      console.log(`PASS ${result.name}: ${result.code}/${result.edge}/${result.path}`);
    }
    console.log(`evidence publisher: ${evidenceResults.length} biting faults passed`);
  } catch (error) {
    if (error instanceof IdentityError) {
      console.error(JSON.stringify({ code: error.code, edge: error.edge, path: error.path, detail: error.detail }));
    } else {
      console.error(error?.stack ?? String(error));
    }
    process.exitCode = 1;
  }
}
