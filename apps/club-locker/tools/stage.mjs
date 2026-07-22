// Stage the non-compiled assets into `out/` after `tsc` has emitted the `.mjs`.
//
// `tsc` emits every `.mts` to `out/` mirroring the source tree, but does not copy the
// pinned supervisor module or the renderer's static assets. This step copies exactly
// those, so `out/` is the complete runnable app the Electron `main` field points at.
// It is a plain build step, run explicitly (never an npm lifecycle script).

import { copyFileSync, mkdirSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const APP = dirname(dirname(fileURLToPath(import.meta.url)));

/** Each `[from, to]` is copied verbatim; `to` is created if needed. */
const ASSETS = [
  ["gen/marrow-supervisor.mjs", "out/gen/marrow-supervisor.mjs"],
  ["renderer/index.html", "out/renderer/index.html"],
  ["renderer/styles.css", "out/renderer/styles.css"],
];

let missing = 0;
for (const [from, to] of ASSETS) {
  const src = join(APP, from);
  const dst = join(APP, to);
  if (!existsSync(src)) {
    console.error(`stage: missing ${from} (generate the client and build first)`);
    missing += 1;
    continue;
  }
  mkdirSync(dirname(dst), { recursive: true });
  copyFileSync(src, dst);
  console.log(`staged ${to}`);
}

if (missing > 0) process.exit(1);
