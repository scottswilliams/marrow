# editors/vscode — agent instructions

This is the installed VS Code artifact for the shipped `marrow lsp` server. It is a
thin host, downstream of Marrow semantics. These rules are lane-local and narrow the
workspace rules; they do not override them.

## Thin-host law

`src/extension.ts` is the only source file. It imports only `vscode` and
`vscode-languageclient/node`. It must not import `fs`, `net`, `http`, `https`, `dns`,
or `child_process`; must not read or scan document text, compute positions, classify
paths, or add client middleware, retry loops, or diagnostic filtering. Every language
fact is the server's. Missing editor behavior is added to Marrow first, never
reconstructed here.

## What this package does not contain

No TextMate grammar or `syntaxes/`, no `language-configuration.json` of any content,
no snippets, themes, debuggers, views, or settings contributions, no on-type
formatting, no telemetry, network client, updater, or downloader, and no second
executable. There is no server-path override setting: the server is the bundled
absolute-path `server/marrow`, launched with the fixed arguments `["lsp"]`.

## Build and packaging

- Install only with `npm ci`. `.npmrc` sets `ignore-scripts=true`; there are no
  lifecycle scripts to run. `package-lock.json` is frozen; regenerating it reruns the
  dependency and license review.
- Build: `npm ci` → `npx tsc -p ./` → copy the canonical `marrow` release binary to
  `server/marrow` (mode 0755) → `npx vsce package --target darwin-arm64`.
- The bundled binary is byte-identical to the canonical release build of the exact
  integrated base; its SHA-256 is pinned in `gate/verify-vsix.mjs`.

## Gates

- `node gate/verify-vsix.mjs <a.vsix> [<b.vsix>]` — closed inventory allowlist, the
  hash-pinned assets, the single-Mach-O payload rule, and dual-build manifest
  equivalence. It is run explicitly, never as an npm lifecycle script.
- `node gate/installed-probe.mjs` — the automatable installed-journey checks. Clauses
  needing the real VS Code host and interactive Workspace Trust UI are reported
  PENDING-HUMAN.
