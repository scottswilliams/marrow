# Club Locker

A minimal desktop application for a squash/racquet club's front desk: register
members and assets, check equipment out and back in, and read a member's recent
history — over a local Marrow store, with no database or Rust toolchain on the
end-user machine. It is the campaign's contained-application evidence: the complete
Club Locker domain runs through the generated strict TypeScript client, supervised by
an Electron trusted main over the native attachment (F02/G03).

The app is **not** a web service, an ORM, or a query tool. The renderer collects
domain arguments and renders replies; every read and write travels typed paths and
ordinary functions in the Marrow program, run against the local store by the trusted
main.

## Architecture

```
renderer (untrusted)  ──contextBridge──▶  preload  ──one IPC channel──▶  trusted main
   window.club                                                              │
                                                                            ▼
                                              deployment (verified)   trusted-main core
                                              runner + image + manifest      │
                                                                    generated strict client
                                                                            │
                                                                     marrow-runner (native
                                                                     attached session over
                                                                     the local store)
```

- **The renderer** (`renderer/`) is plain HTML/CSS/TypeScript with no framework. It
  runs isolated and sandboxed under a strict CSP and can only ask, over one channel,
  for a named domain export to run with domain arguments.
- **The trusted main** (`app/main.mts`) owns every process/store/path/authority
  choice. It verifies the bundled deployment, chooses the fixed data directory,
  provisions the store on first launch, opens one native attached session through
  the generated client, and dispatches only allowlisted domain calls from its own
  window.
- **The core and deployment** (`app/core.mts`, `app/deployment.mts`) hold the
  process/store logic and the integrity verification, free of Electron so the probe
  suite drives them directly.

## The domain

The durable model (`src/clublocker.mw`, the E07 Club Locker fixture) has members,
assets in categories with a unique tag, and the loan lifecycle, with managed indexes
and gapless per-sequence counters. The UI exercises the register / checkout / return
/ member-history journeys.

## The three artifacts

| Artifact | Built by | Committed? |
|---|---|---|
| Generated client (`gen/`) | `marrow client typescript --out gen` | no (regenerated) |
| Deployment (`deploy/`) | `tools/compose-deployment.mjs` | no (composed) |
| Packaged app (`out/`) | `npm run build` | no (built) |

The deployment is `MarrowDeployment` = the pinned `marrow-runner`, the verified
`program.image` (emitted by the stock `marrow image` command against an owner-accepted
ceiling), and the `marrow-deployment` manifest binding their identities.

## Build and run (local, from source)

The interactive path is two commands. `setup` walks the whole pipeline —
client generation, the image's authority review with an explicit acceptance
prompt, deployment composition, build, and the deployment gate:

```sh
cd apps/club-locker
npm ci          # ignore-scripts: no Electron binary is fetched
npm run setup   # interactive; prints the demand and asks before composing
npm start
```

The same pipeline as explicit steps (the scripted/CI form — the acceptance id
is passed by hand):

```sh
# 1. generate the strict client from the domain
marrow client typescript --out gen

# 2. review the image's demand and accept its ceiling, then compose the deployment
marrow image --out /tmp/x                 # prints the ceiling id and the demand to review
node tools/compose-deployment.mjs \
  --marrow "$(command -v marrow)" \
  --runner /path/to/marrow-runner \
  --accept-ceiling <ceiling-id-from-step-2>

# 3. build the app
npm run typecheck
npm run build

# 4. launch (needs Electron installed locally; source-only, unsigned)
npx electron .
```

The store lives under the OS user-data directory and is created on first launch. A
second instance focuses the first rather than opening a second store.

## Gates

- `node gate/verify-deployment.mjs deploy [deploy-b]` — deployment inventory, the
  single-native-payload rule, manifest resolution, and dual-build determinism.
- `MARROW=<marrow> node gate/probe.mjs` — the automatable containment and journey
  probes: the security-floor config, the fail-closed sender/deployment checks,
  single-winner first-launch provisioning, the renderer's closed call surface, the
  staged compiled build, and terminal/TypeScript read identity. GUI/clean-machine
  clauses are reported PENDING-HUMAN.

## Scope

Source-only local build; no signing, notarization, DMG/PKG, updater, marketplace, or
cross-platform packaging in v0.1.

## Known-benign console noise

With DevTools open, Chromium's DevTools front-end logs
`Request Autofill.enable failed … 'Autofill.enable' wasn't found` (and the
matching `Autofill.setAddresses` line). Electron does not ship Chrome's
autofill subsystem, so DevTools' request comes back unimplemented. The lines
appear in every Electron app while DevTools is open, stop when it closes,
and touch nothing in this application.
