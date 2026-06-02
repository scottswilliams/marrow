# Lane 10: Tooling, Backup, Restore, And Protocols

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> Tooling is downstream of shared facts; do not rediscover semantics in adapters.

Goal: make CLI, LSP, data tools, serve, backup, restore, and future adapters
consume shared compiler/runtime facts and expose typed production protocols,
with raw protocols limited to explicit debug/admin surfaces.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-10-tooling-protocols`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-10-tooling-protocols`

Status: read-only stale protocol and docs inventory may start now; tracked edits
wait for the relevant fact, store, runtime, and evolution generation contracts.
The first code phase in this lane defines the typed backup manifest and
production backup/restore API.

## Parallel Safety

This lane can inventory docs and protocol descriptions in parallel, but early
inventory is read-only: do not edit tracked protocol docs, define replacement
protocol shapes, or patch missing facts into tools before dependencies land.
Tracked changes to CLI/LSP/serve/data/backup wait until the facts they render
are integrated. The typed backup manifest phase waits for source, catalog,
store, runtime, and generation facts, then becomes the contract that later
backup CLI and protocol adapters consume. Send missing semantic facts back to
the owning lane.

Own these files during the code pass:

- `crates/marrow/src/cmd_check.rs`
- `crates/marrow/src/cmd_data.rs`
- `crates/marrow/src/lsp.rs`
- `crates/marrow/src/serve/protocol.rs`
- `crates/marrow/tests/*data*.rs`
- `crates/marrow/tests/*serve*.rs`
- `crates/marrow/tests/*lsp*.rs`
- `crates/marrow/tests/*protocol*.rs`
- `docs/cli.md`
- `docs/lsp.md`
- `docs/serve-protocol.md`
- `docs/data-tools.md`
- `docs/backend-contract.md` only for backup/restore references

## Area Cleanup Gate

This lane owns the complete cleanup of tooling and protocol surfaces across CLI,
LSP, data preview, serve, backup, restore, protocol docs, fixtures, and tests.
It must delete adapter-local semantic classifiers, raw production protocols, and
unbounded preview assumptions in its area instead of leaving a second tool model
for a later lane.

Before handing the lane to review:

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

- CLI, LSP, data tools, serve, backup, restore, and future adapters render
  shared facts.
- Diagnostics and activation details render the presence ledger.
- Raw physical keys and backend bytes are debug/admin only.
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

## Prototype Removal Ledger

Replacement behavior: tools display compiler/runtime/store facts without
becoming semantic owners.

Delete or isolate:

- raw data/serve protocol claims as stable production APIs;
- tool-local source-name or saved-path re-resolution;
- portable backup as raw path/value dump;
- unbounded data preview materialization;
- LSP patches that infer facts missing from Marrow itself.

Production bridge: none for protocols. Raw inspection commands are allowed only
when named debug/admin-only, excluded from default production docs, and unable to
serve as backup/restore, data-preview, LSP, or serve protocol semantics.

## TDD Start

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

- CLI and LSP render the same diagnostic from shared facts;
- presence-ledger proof details appear through CLI/LSP without reclassification;
- raw debug protocols are opt-in;
- stale epoch, snapshot, and generation produce typed errors;
- data previews are bounded and snapshot-bound;
- backup during concurrent read uses a stable snapshot;
- backup CLI and serve protocols consume the manifest/API from Phase A.

Focused commands:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-10-tooling-protocols \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-10-tooling-protocols/Cargo.toml \
    -p marrow
```

## Review Lenses

Soundness review attacks stale platform tokens, stale generations, restore
mismatch, raw debug exposure, unbounded previews, and LSP divergence.

Idiom/spec review checks adapters stay thin, transport-specific code has no
semantic classifiers, docs mark raw surfaces as debug/admin, and no new
dependency appears. It also rejects oversized adapter dispatchers, duplicate
semantic classifiers, raw fallback glue, comment sediment, and lane-local
cleanup deferred to Lane 11.

## Integration Gate

Run the full central gate. Add scans:

```sh
rg -n 'raw|debug|path|saved path|backend bytes|re-resolv|resolve' \
    /Users/scottwilliams/Dev/marrow-lane-10-tooling-protocols/crates/marrow/src \
    /Users/scottwilliams/Dev/marrow-lane-10-tooling-protocols/docs
```

Every match must be typed production rendering, explicit debug/admin scope, or a
test proving the old surface is rejected.

## Starter Prompt

Continue Marrow v0.1 Lane 10 in `/Users/scottwilliams/Dev/marrow-lane-10-tooling-protocols`.
Use branch `lane-10-tooling-protocols`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-10-tooling-protocols`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
Do a read-only inventory of stale raw protocol docs now; do not edit tracked
protocol docs, define replacement typed protocol shapes, or patch around missing
semantic facts in tools before dependencies land. Production code waits for Lane
5/6 shared checker facts, Lane 7 store/tree-cell facts, Lane 8 runtime facts,
and Lane 9 evolution/generation/activation facts. Once those dependencies
exist, first define the typed backup manifest and production backup/restore API,
then make CLI/LSP/data/serve/backup render or consume those facts directly and
restrict raw surfaces to debug/admin. No legacy survival for green tests:
migrate/delete tests, fixtures, and clients that depend on raw protocol bytes,
raw path JSON, tool-local semantic classifiers, or unbounded previews. Before
review, satisfy the Area Cleanup Gate: split backup manifest validation, restore
activation, CLI rendering, LSP rendering, data preview, and serve adapters;
delete raw protocol, saved-path re-resolution, raw archive, unbounded preview,
and LSP semantic-patch helpers. Leave the worktree dirty for soundness and
idiom/spec review.
