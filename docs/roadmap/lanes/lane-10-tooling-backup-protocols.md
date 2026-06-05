# Lane 10: Tooling, Backup, Restore, And Protocols

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> Tooling is downstream of shared facts; do not rediscover semantics in adapters.

Goal: make CLI, LSP, data tools, serve, backup, restore, and future adapters
consume shared compiler/runtime facts and expose typed production protocols,
with raw protocols limited to explicit debug/admin surfaces.

Status: historical lane plan. Lane 10's typed backup/restore foundation landed;
Lane 16 supersedes the stale tooling/protocol blockers by moving data, explain,
integrity, metadata, and cursor facts into `marrow_check::tooling`. Treat this
file as audit/design context, not the active status source for current tooling
work.

Resolved or superseded blockers inherited from the Lane 8 repair:

- `marrow data get`, `marrow data dump`, and `marrow debug explain ^path` are
  diagnostic/admin inspection surfaces rendered from shared facts. They are not
  production preview APIs.
- Raw `marrow serve saved_children` is gone. Current serve inspection operations
  are explicitly `debug_data_*`; the production protocol must be checked-fact
  based, bounded, and snapshot/catalog-epoch scoped.
- No backup, restore, LSP, or production preview client may depend on raw saved
  path strings, backend bytes, or tool-local path classifiers; Lane 16 added
  architecture coverage for the public tooling signatures and serve/CLI
  separation.

## Completion Claim Discipline

Lane 10 has two valid early outcomes: **audit complete** and **blocked**. The
feature-surface verdict matrix is required, but it is not lane completion. The
lane may claim **lane complete** only after supported production tools and
protocols are rebuilt or deleted/demoted, docs and tests match the verdicts,
and typed backup/restore/protocol code passes review.

Before any completion claim, Lane 10 must prove all sibling protocol surfaces
were checked after each fix:

- fixing `serve debug_data_walk` also audits `debug_data_children`, data
  previews, cursor format, snapshot/catalog epoch, CLI docs, and serve tests;
- demoting raw saved paths also audits `data get`, `data dump`, `explain ^path`,
  backup, restore, LSP, docs, and tests;
- keeping trace, dry-run, maintenance, or debug/admin tools requires a product
  verdict, explicit namespace or flag, and absence from default production
  protocol semantics;
- replacing backup cannot leave raw archive replay as a production restore path;
- deleting or demoting a command must remove its default help/docs/test
  expectations, not hide it behind old fixtures.

Code shape is part of the claim. A broad adapter file may not absorb request
dispatch, path/key codecs, cursor handling, backup manifest validation, restore
activation, CLI rendering, and test fixtures in one module. Split by invariant
before review and remove comments that explain branch structure instead of
durable protocol constraints.

## Parallel Safety

This lane can inventory docs and protocol descriptions in parallel, but early
inventory is read-only: do not edit tracked protocol docs, define replacement
protocol shapes, or patch missing facts into tools before dependencies land.
The read-only audit may inspect language, runtime, store, and tooling surfaces,
but it returns findings to the owning lane unless the surface is in this lane's
owned files.
Tracked changes to CLI/LSP/serve/data/backup wait until the facts they render
are integrated. The typed backup manifest phase waits for source, catalog,
store, runtime, and generation facts, then becomes the contract that later
backup CLI and protocol adapters consume. Send missing semantic facts back to
the owning lane.

Own these files during the code pass:

- `crates/marrow/src/cmd_check.rs`
- `crates/marrow/src/cmd_data.rs`
- `crates/marrow/src/cmd_explain.rs`
- `crates/marrow/src/cmd_run.rs` only for tool flags and output rendering
- `crates/marrow/src/cmd_test.rs` only for tool flags and output rendering
- `crates/marrow/src/dry_run.rs`
- `crates/marrow/src/lsp.rs`
- `crates/marrow/src/main.rs`
- `crates/marrow/src/serve/protocol.rs`
- `crates/marrow/src/serve/**`
- `crates/marrow/src/trace.rs`
- `crates/marrow/tests/*data*.rs`
- `crates/marrow/tests/*explain*.rs`
- `crates/marrow/tests/*run*.rs` only for tool flags and output rendering
- `crates/marrow/tests/*test*.rs` only for trace/tool rendering
- `crates/marrow/tests/*serve*.rs`
- `crates/marrow/tests/*lsp*.rs`
- `crates/marrow/tests/*protocol*.rs`
- `docs/cli.md`
- `docs/lsp.md`
- `docs/serve-protocol.md`
- `docs/data-tools.md`
- `docs/backend-contract.md` only for backup/restore references

Audit-only inputs:

- `docs/language/**`, `docs/data-modeling.md`, `docs/data-evolution.md`, and
  runtime/checker/store files when deciding whether a surface is still a v0.1
  product surface;
- do not edit language, checker, runtime, or store code unless the owning lane
  explicitly hands that file to Lane 10.

## Feature-Surface Audit Gate

Before production code starts, produce a verdict matrix for every active or
documented tool, protocol, language/database-facing flag, and saved-data surface:

- **keep production**: explicitly supported by accepted ADRs, rendered from
  shared facts, typed, versioned where external, bounded, and snapshot or epoch
  bound when it reads durable data;
- **debug/admin only**: exposes raw physical keys, backend bytes, raw saved
  paths, repair-only capabilities, or diagnosis-only internals; it must require
  an explicit debug/admin or maintenance selection and must be absent from
  default production docs;
- **rename/rescope**: the underlying capability is valid, but the command name,
  docs, output, or protocol implies a broader product such as hidden execution planning,
  database server behavior, stable raw path access, or public generated APIs;
- **delete**: unsupported by accepted ADRs, overlapping with another production
  surface, preserving prototype behavior, or needing local semantic
  rediscovery.

Known suspects for the first audit:

- `marrow debug explain`: not hidden execution planning. Current saved-path/name inspection must
  either become a typed shared-fact explanation surface or leave production.
- `marrow serve`: v0.1 is not a public app server or remote database server.
  Any retained serve path must be typed, versioned, bounded, and explicit about
  debug/admin versus production purpose.
- `marrow data roots/stats/dump/get/integrity`: raw bytes and raw saved paths
  are not production protocols. Keep only typed, bounded fact rendering or
  explicit debug/admin inspection.
- `run --trace`, `test --trace`, `run --dry-run`, and `--maintenance`: classify
  the product story, verify they expose typed facts rather than raw storage, and
  ensure maintenance cannot become a semantic bypass.
- backup, restore, archive, and raw debug/admin store access: typed
  backup/restore is production; raw archive is debug/admin only or deleted.
- LSP, future DAP/MCP, and generated clients: render shared facts only; no
  fallback schema/path/name classifiers.
- language/database residue that tools must not preserve: `@id`, `lock`,
  `merge`, saved-path `inout`, raw saved paths as identity, source-order enum
  ordinals as stored meaning, source names as physical store keys, unbounded
  scans, migration scripts, public package/server/sync promises, and raw path
  compatibility APIs.

## Area Cleanup Gate

This lane owns the complete cleanup of tooling and protocol surfaces across CLI,
LSP, data preview, serve, backup, restore, protocol docs, fixtures, and tests.
It must delete adapter-local semantic classifiers, raw production protocols, and
unbounded preview assumptions in its area instead of leaving a second tool model
for a later lane.

Before handing the lane to review:

- complete the feature-surface verdict matrix and turn every supported verdict
  into a test, docs deletion, docs rewrite, or owning-lane blocker;
- delete or demote unsupported command entries, tests, docs, and protocol
  examples instead of leaving them as dormant product promises;
- split backup manifest validation, restore activation, CLI rendering, LSP
  rendering, data preview, and serve protocol adapters by invariant;
- migrate or delete tests, fixtures, and clients that depend on raw protocol
  bytes, raw path JSON, tool-local semantic classifiers, or unbounded previews
  instead of keeping legacy tool surfaces for them;
- send missing semantic facts back to owning lanes instead of reclassifying paths
  or diagnostics in tools;
- delete dead raw protocol, saved-path re-resolution, raw archive, unbounded
  preview, and LSP semantic-patch helpers introduced or exposed by this lane;
- delete comments that narrate adapter branches, defend temporary raw surfaces,
  or compensate for overgrown command functions;
- preserve only comments for non-obvious protocol, snapshot, manifest, or
  recovery constraints;
- ensure the idiom/spec reviewer explicitly checks touched Rust for oversized
  adapter functions, duplicate semantic classifiers, raw fallback glue, comment
  sediment, and lane-local cleanup deferred to Lane 11.

## Production Contract

- The production CLI is small: check, fmt, run, test, LSP/editor support, and
  only accepted typed data/evolution/backup tooling.
- CLI, LSP, data tools, serve, backup, restore, and future adapters render
  shared facts.
- Diagnostics and activation details render the presence ledger.
- Raw physical keys and backend bytes are debug/admin only.
- Raw saved paths are debug/admin output, not durable identity, typed references,
  backup entries, or stable protocol addresses.
- Data previews stream bounded chunks, preserve tree/sequence/keyed-layer
  shape, and are snapshot-bound.
- Internal continuations are catalog-epoch and snapshot bound.
- Backup is a typed Marrow artifact that validates source, catalog, data, engine
  profile, checksums, layout, codecs, indexes, and sequence state before
  activation.
- Lane 10 owns the typed backup manifest and production backup/restore API as
  its first code phase. CLI, serve, and data adapters consume that manifest; they
  do not define backup semantics locally.
- Runtime generation and stale-writer facts exist for local activation.
- A feature without an ADR-backed user story is deleted or explicitly demoted
  before code is written around it.

## Prototype Removal Ledger

Replacement behavior: tools display compiler/runtime/store facts without
becoming semantic owners.

Delete or isolate:

- unsupported product commands and flags found by the feature-surface audit;
- `marrow debug explain` as a raw saved-path/name resolver unless rebuilt as typed
  shared-fact explanation;
- `marrow serve` as a raw saved-data server or public app-server stand-in;
- raw data/serve protocol claims as stable production APIs;
- tool-local source-name or saved-path re-resolution;
- portable backup as raw path/value dump;
- unbounded data preview materialization;
- LSP patches that infer facts missing from Marrow itself.

Production bridge: none for protocols. Raw inspection commands are allowed only
when named debug/admin-only, excluded from default production docs, and unable to
serve as backup/restore, data-preview, LSP, or serve protocol semantics.

## TDD Start

Phase 0 is the feature-surface audit. It produces the verdict matrix, known
delete/demote targets, owning-lane blockers, and the first failing tests for any
surface Lane 10 is allowed to change.

Phase A writes failing manifest/API checks:

- typed backup manifest records source, accepted catalog, data snapshot, engine
  profile, layout, codecs, checksums, indexes, sequence state, and generation
  facts;
- manifest validation rejects catalog/data/store/profile mismatch and corrupt
  chunks;
- backup/restore round-trips typed references as store identity plus key;
- restore verifies or rebuilds derived index data before exposing it;
- restore preserves or safely repairs per-store sequence state.

Phase B writes failing adapter checks:

- default help and production docs omit deleted/debug-only surfaces;
- debug/admin raw inspection requires an explicit flag or command namespace;
- `debug explain`, `serve`, trace, dry-run, maintenance, and data inspection
  surfaces match the verdict matrix;
- CLI and LSP render the same diagnostic from shared facts;
- presence-ledger proof details appear through CLI/LSP without reclassification;
- raw debug protocols are opt-in;
- stale epoch, snapshot, and generation produce typed errors;
- data previews are bounded and snapshot-bound;
- backup during concurrent read uses a stable snapshot;
- backup CLI and serve protocols consume the manifest/API from Phase A.

Focused gates must run from the active integration worktree with an explicit
isolated target directory and explicit manifest path.

## Review Lenses

Soundness review attacks stale platform tokens, stale generations, restore
mismatch, raw debug exposure, unbounded previews, feature-surface drift, and LSP
divergence.

Idiom/spec review checks adapters stay thin, transport-specific code has no
semantic classifiers, docs mark raw surfaces as debug/admin, and no new
dependency appears. It also rejects oversized adapter dispatchers, duplicate
semantic classifiers, raw fallback glue, comment sediment, and lane-local
cleanup deferred to Lane 11.

Feature/spec review checks every verdict in the feature-surface matrix against
the accepted ADR packet. It rejects keeping a command, flag, endpoint, doc page,
or test fixture just because it existed in the prototype.

## Integration Gate

Run the full central gate. Add scans:

Scan the active stack for `explain`, `serve`, `trace`, `dry-run`,
`maintenance`, `raw`, `debug`, `path`, `saved path`, `backend bytes`,
`re-resolve`, `query`, `server`, `sync`, and generated API wording.

Every match must be typed production rendering, explicit debug/admin scope, or a
test proving the old surface is rejected.

## Historical Execution Criteria

Deliver a verdict matrix for every current or documented tool/protocol/language
or database-facing surface: keep production, debug/admin only, rename/rescope,
or delete. Audit at least `marrow debug explain`, `marrow serve`, `marrow data`
roots/stats/dump/get/integrity, `run --trace`, `test --trace`, `run --dry-run`,
`--maintenance`, backup/restore/archive/debug_admin, LSP/future adapters, raw
saved paths, raw backend bytes, public server/sync/generated API promises,
unbounded scans, source-order enum ordinals, `@id`, `lock`, `merge`, and saved
`inout`. A retained surface must be ADR-backed, typed, bounded, rendered from
shared facts, and explicit about epoch/snapshot/generation when it touches
durable data. Anything else is a delete, demotion, or owning-lane blocker.

Production code waits for Lane 5/6 shared checker facts, Lane 7 store/tree-cell
facts, Lane 8 runtime facts, and Lane 9 evolution/generation/activation facts.
Once those dependencies exist, first define the typed backup manifest and
production backup/restore API, then make CLI/LSP/data/serve/backup render or
consume those facts directly and restrict raw surfaces to debug/admin.

Do not stop after fixing one endpoint or command. After each fix, scan the
sibling family: `serve debug_data_*`, `data *`, `debug explain`,
backup/restore/archive, LSP, docs, tests, help output, and protocol examples.
No legacy survival for green tests: migrate/delete tests, fixtures, docs,
commands, flags, and clients that depend on raw protocol bytes, raw path JSON,
tool-local semantic classifiers, or unbounded previews. Before review, satisfy
the Area Cleanup Gate: split backup manifest validation, restore activation,
CLI rendering, LSP rendering, data preview, key/path codecs, cursor handling,
and serve adapters; delete raw protocol, saved-path re-resolution, raw archive,
unbounded preview, and LSP semantic-patch helpers. A final done claim must
include the completion evidence packet required by the central plan.
