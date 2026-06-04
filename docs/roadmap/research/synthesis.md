# Marrow v0.1 Research Synthesis

Date: 2026-06-04

Status: archived research synthesis. This file records the architecture
direction extracted from the finalized research reports. It is not an ADR, an
implementation plan, or status record.

Canonical authority lives in the accepted ADR packet in `marrow-decisions`, the
language reference in `docs/language/`, the CLI/data/serve/backend/evolution
docs, and the active implementation tracker in
`docs/roadmap/prototype-to-v1-execution-plan.md`.

## Research Inputs

This synthesis integrated the finalized research reports for:

- resource/store surface;
- catalog identity, presence ledger, and enum member identity;
- typed tree-cell store and engine profile;
- checked runtime execution and write planning;
- source-native evolution and activation;
- tooling, backup, restore, serve, LSP, and data tools;
- holistic language ergonomics;
- architecture red-team review;
- future online evolution foundation.

Use those reports as historical evidence. Use the canonical docs and accepted
ADRs for current product truth.

## Settled Architecture

Marrow stays a source-native, typed tree language with built-in durable data. It
does not become SQL, an ORM, a document-store wrapper, an execution planner, or a
private database server.

The load-bearing model is:

- source declares resources, stores, functions, transactions, and evolution
  intent;
- the catalog owns durable identity;
- the compiler checks source, accepted catalog, attached saved data, and engine
  profile together;
- runtime executes checked facts, checked executable bodies, and explicit write
  plans;
- the store persists typed tree cells over a private ordered-byte engine;
- tools render compiler/runtime/store facts instead of rediscovering semantics;
- source is the access path. There is no hidden execution planner beneath
  ordinary Marrow code.

## Settled Research Findings

These findings were incorporated into canonical docs and accepted ADRs; this
list is not standalone authority.

- Keep the source/catalog/compiler/runtime/engine split.
- Keep resources as reusable typed tree shapes and stores as durable roots.
- Keep `Id(^store)` as the v0.1 store-identity type constructor. Resource-owned
  aliases such as `Book::Id` are rejected.
- Keep the accepted generated catalog as the durable ABI. Source-visible stable
  ID annotations such as `@id`, source-path identity, source-order enum
  ordinals, and regenerated IDs are rejected.
- Keep enum member identity catalog-backed so source reordering is
  non-destructive.
- Keep `TreeStore` as the typed tree-cell production model over a private redb
  ordered-byte substrate.
- Keep checked runtime execution and explicit write plans. Runtime does not
  execute source syntax bodies or resolve raw saved paths.
- Keep source-native evolution through explicit `rename`, `default`,
  `transform`, and `retire` intent, exact witnesses, and fail-closed apply.
- Keep typed backup/restore artifacts bound to source digest, accepted catalog
  epoch, engine profile, layout/value-codec facts, checksums, and tree-cell
  data.
- Keep sparse presence as checked address-presence facts. `exists`, optional
  chaining, and `??` discharge presence; they do not create implicit nulls.
- Keep whole-resource assignment as exact replacement. Partial updates are field
  writes, usually grouped in a transaction.
- Keep `Id(^store)` fields as typed values. Dangling references are allowed by
  default, but they are compiler-visible integrity facts.
- Keep history and audit state explicit in user-mode resources. Marrow v0.1 is
  not an automatic temporal database or audit-log system.
- Keep `unknown` as a boundary type, not `any`. Dynamic values must be checked
  before entering typed Marrow code or managed saved data.

## Rejected Or Reserved

These surfaces are not production authority:

- resource-owned identity aliases such as `Book::Id`;
- source-level stable IDs such as `@id`;
- `edit`, patch, or update DSLs for ordinary writes;
- normal `merge` and `lock` syntax paths. The words may stay reserved, but they
  are not supported v0.1 statements;
- saved-path `inout`;
- source-diff identity inference, best-effort rename inference, migration
  scripts, migration DSLs, or hidden schema-history ledgers;
- raw saved paths, raw physical keys, backend bytes, raw archive replay, or raw
  path/value dumps as production protocols;
- production restore that preserves orphaned managed cells under the current
  source/catalog;
- `marrow debug explain` wording that implies planner choices, statistics, or
  SQL-style execution details.

This classification informed the canonical reconciliation. Current docs and ADRs
carry authority for each surviving term.

## Debug/Admin And Future Boundaries

`marrow data` and `marrow serve` v0.1 may exist only as read-only
debug/admin inspection over checked facts. The `debug_data_*` serve operations
are loopback, bounded, snapshot-bound inspection operations, not a public app
server, sync server, generated API, backup format, or raw-path compatibility
protocol.

A future production local API remains allowed. It must be generated or checked
from resources, public functions, effects, catalog facts, and typed runtime
facts; it must be versioned, bounded, catalog-epoch aware, and local-first. It
does not inherit the current debug path/value protocol.

Production `~` roots remain future-only. They require checked effect classes and
runtime support before becoming executable saved state.

External engine adapters remain deferred. The v0.1 typed tree-cell contract
should leave room for future adapters, but adapters must implement Marrow's
semantics rather than SQL or ORM semantics.

## Evolution Direction

V0.1 remains strict and fail-closed: activation is exact, source/catalog/data
and engine profile are checked together, stale writers fail closed, and apply
uses the exact preview witness.

Future online activation should be compiler-owned and job-shaped. A job may have
conceptual phases such as preview, start, bridge, backfill, verify, publish, and
close. V0.1 may execute that shape immediately in one exact transaction.

Compatibility adapters are future-only, bounded, generated from checked
source/evolution facts, visible in tooling, and deleted when the window closes.
Old writes are rejected unless the compiler proves they lower to latest-format
write plans and preserve every active or building fact.

Key changes, resource reshapes, layout recompiles, and engine moves that cannot
be proven as ordinary backfill use future shadow decant: build a new
store/layout in chunks, bridge only a bounded set of writes, verify identity,
count, and checksum facts, publish a small binding change, then close and purge.
This is activation work, not raw store patching or migration scripting.

## Archive Boundary

Open product choices and implementation sequencing are tracked outside this
archive, in `docs/roadmap/prototype-to-v1-execution-plan.md` and the current
lane files under `docs/roadmap/lanes/`.
