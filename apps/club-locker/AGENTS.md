# apps/club-locker — agent instructions

The packaged Club Locker desktop app: an Electron trusted main over the F02 native
attachment, driving the generated strict TypeScript client. These rules are
lane-local and narrow the workspace rules; they do not override them.

## Containment law (do not relax)

- The renderer is untrusted. It runs with `contextIsolation` on, `nodeIntegration`
  off, and the OS `sandbox` on (the frozen `WEB_PREFERENCES` in `app/security.mts`),
  behind the frozen `CONTENT_SECURITY_POLICY`. It reaches the trusted main through
  exactly one channel and may name only an export in `DOMAIN_EXPORTS`, with domain
  arguments. It never selects a runner, image, store, path, grant, or ceiling.
- The trusted main (`app/main.mts`) owns every process/store/path/authority choice.
  It resolves the runner ONLY from the packaged deployment directory, verified by
  release identity and image identity (`app/deployment.mts`), never from PATH, the
  working directory, or the environment. It gates every channel message through
  `isTrustedSender` before dispatching.
- `app/security.mts` and `app/deployment.mts` hold no Electron import so the probe
  suite exercises the exact values and fail-closed predicates. Keep containment
  logic there, not in the Electron wiring.

## Three artifacts

1. **Generated client** (`gen/`, regenerated, gitignored) — `marrow client
   typescript --out gen`.
2. **Deployment** (`deploy/`, composed, gitignored) — the pinned `marrow-runner`,
   the verified `program.image`, and the `marrow-deployment` manifest, built by
   `tools/compose-deployment.mjs`. The image is emitted by the stock `marrow image`
   command, which requires the owner to accept the image's own ceiling id.
3. **Packaged app** (`out/`, built, gitignored) — `tsc` emits the `.mjs`;
   `tools/stage.mjs` copies the supervisor and renderer assets. `main` points at
   `out/app/main.mjs`. Source-only distribution: no signing, DMG/PKG, updater,
   marketplace, or cross-platform packaging.

## Build and gates

- Install only with `npm ci`. `.npmrc` sets `ignore-scripts=true`; the Electron
  binary is not fetched, so an interactive GUI launch is a human step. The app has
  no production dependencies; the devDependency closure is `electron`, `typescript`,
  and `@types/node`. `package-lock.json` is frozen; regenerating it reruns the
  dependency and license review.
- `npm run typecheck` — strict `tsc --noEmit`.
- `npm run build` — `tsc -p .` then `tools/stage.mjs`.
- `node tools/compose-deployment.mjs --marrow <marrow> --runner <marrow-runner>
  --accept-ceiling <id>` — compose the deployment.
- `node gate/verify-deployment.mjs <deployA> [<deployB>]` — inventory, single-native
  payload, resolve, and dual-build determinism.
- `MARROW=<marrow> node gate/probe.mjs` — the automatable containment and journey
  probes. Clauses needing a live Chromium/GUI or a truly clean machine are printed
  PENDING-HUMAN; the B6 unaided ceiling-expansion walkthrough is BLOCKED-ON-F03b.

## Trust boundary (accepted limitation)

The deployment integrity checks refuse partial tampering: altered runner bytes, an
altered image, a missing/malformed manifest, a symlinked or traversing/absolute
component, and a plain release-string skew all fail closed. They do NOT, on their
own, catch a wholesale re-compose — an actor who can write the deployment directory
can substitute a runner and a fully self-consistent manifest (matching identities
and release). That is inherent to a source-only, unsigned distribution (no signing
apparatus exists in v0.1) and is outside the renderer threat model: write access to
the bundle already implies the ability to patch the app's own code. Bundle integrity
is the responsibility of the OS install/permissions, not this app. `resolveDeployment`
is a tamper check over an authentic bundle, not an authenticator of an untrusted one.

## Downstream of Marrow semantics

A missing capability is added to Marrow first. This app reconstructs no types,
paths, authority, or wire meaning: it consumes the generated client and the pinned
supervisor. The two Marrow identity constructions it recomputes (companion release
id, image id) mirror the frozen Rust owners byte for byte and are integrity checks,
not semantic reconstruction.
