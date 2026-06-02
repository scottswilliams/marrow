# Lane 7: Tree-Cell Store And Engine Profile

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> Lane 7 is reopened for this perfection pass. Use this file as the store
> contract record; new runtime/tooling work starts from Lane 8 or Lane 10.

Goal: replace source-name physical storage with tree-cell storage keyed by
stable catalog IDs, typed key values, sequence state, index cells, commit
metadata, and an explicit engine profile.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-07-perfect-store-boundary`

Status: perfection pass in review. The store crate owns the tree-cell model,
redb substrate, and debug/admin raw archive boundary; Lane 8 and Lane 10 own the
remaining runtime and tooling raw-path callers.

## Parallel Safety

Follow-up store fixes stay inside this lane's store/backend ownership. Runtime
write planning, tooling protocols, backup formats, and evolution activation use
the tree-cell APIs from their own lanes.

Own these files during the code pass:

- `crates/marrow-store/src/backend.rs`
- `crates/marrow-store/src/path.rs`
- `crates/marrow-store/src/value.rs`
- `crates/marrow-store/src/mem.rs`
- `crates/marrow-store/src/redb.rs`
- `crates/marrow-store/src/archive.rs`
- `crates/marrow-store/src/conformance.rs`
- `crates/marrow-store/tests/*.rs`
- `docs/backend-contract.md`

Do not edit checker facts, catalog file ownership, runtime write planning, or
tooling protocols in this lane except to consume stable APIs already integrated.

## Area Cleanup Gate

This lane owns the complete cleanup of the storage area across engine adapters,
tree-cell keys and values, commit metadata, conformance tests, backend docs, and
fixtures. Store-local cleanup is complete only when source-name physical-key
paths, raw archive production paths, and in-memory-list production assumptions
are absent from the store area instead of left as a second storage model.

Before handing the lane to review:

- split engine substrate, tree-cell address encoding, commit metadata, index
  cells, debug/admin archive inputs, and conformance helpers by invariant;
- migrate or delete tests, fixtures, and callers that depend on source-name
  physical keys, raw archive production behavior, or flat-list storage instead
  of keeping legacy storage branches for them;
- keep redb as ordered bytes and transactions only; semantic key decisions live
  in Marrow storage modules, not backend adapters;
- delete dead source-name key, raw archive, schema-in-backend, and flat-list
  helpers introduced or exposed by this lane;
- delete comments that narrate encoding branches, repeat type names, or explain
  compatibility behavior;
- preserve only comments for non-obvious ordering, durability, corruption, or
  recovery rationale;
- ensure the idiom/spec reviewer explicitly checks touched Rust for oversized
  store functions, duplicate key classifiers, redb semantic leakage, comment
  sediment, and lane-local cleanup deferred to Lane 11.

## Production Contract

- The engine contract is ordered bytes, snapshots, one writer, transactions,
  internal range iterators, engine profile, typed errors, and no semantic
  ownership in redb.
- Marrow tree cells own node, leaf, index, sequence, catalog/meta, and
  blob/chunk cells.
- Physical keys derive from stable catalog IDs, typed store keys, and the
  reserved empty placement prefix from ADR 0208, never source names.
- Commit metadata records commit id, catalog epoch, layout epoch, engine profile
  digest, changed roots, and changed indexes.
- Read-only opens cannot accidentally acquire writer capabilities.

## Prototype Removal Ledger

Replacement behavior: tree-cell keys and values are typed Marrow storage
contracts above redb ordered bytes.

Delete or isolate:

- source root, field, layer, index, enum-member, or raw path text in production
  physical keys;
- portable backup identity is typed tree-cell data; the only public raw archive
  surface is explicitly debug/admin;
- backend logic that decides Marrow schema semantics;
- in-memory list materialization where tree, sequence, or keyed-layer state is
  the actual contract.

Production bridge: none inside tree-cell store semantics. The saved-path
backend and path APIs remain the ordered-byte substrate currently consumed by
runtime and tooling; Lane 8 and Lane 10 own those callers. Raw archive is
isolated under `marrow_store::debug_admin` and is not the portable backup
contract or a second tree-cell storage model.

## TDD Start

Engine-substrate checks cover:

- snapshot isolation and one-writer behavior;
- rollback;
- typed engine errors;
- read-only opens cannot acquire writer capability.

Semantic tree-cell checks cover:

- commit metadata with catalog epoch and engine profile digest;
- node-cell existence and leaf absence;
- source-rename-stable physical keys;
- enum reorder stable meaning;
- range-iterator traversal and bounded scan state;
- sequence state preservation;
- absent index components, non-unique tie-breakers, composite ordering, binary
  string ordering, unique duplicate rollback, duplicate build failure before
  publish, and data/index atomicity;
- typed reference encoding stores store identity plus key, not a raw scalar.

Focused command:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-07-perfect-store-boundary \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store/Cargo.toml \
    -p marrow-store --features native
```

## Review Lenses

Soundness review attacks rollback, ambiguous commits, corrupt metadata, source
renames, enum reorders, index atomicity, read-only opens, and repair behavior.

Idiom/spec review checks redb stays a substrate, storage modules stay small, no
new dependency appears, and tree-cell contracts match ADR 0204, 0207, and 0208.
It also rejects oversized store dispatchers, duplicate key classifiers, redb
semantic leakage, comment sediment, and lane-local cleanup deferred to Lane 11.

## Integration Gate

Run the full central gate. Add scans for source-name key construction:

```sh
rg -n 'path|root|field|index|enum|name' \
    /Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store/crates/marrow-store/src
```

Every production match must be typed metadata, debug rendering, or a rejection
test; source spelling cannot be the production key identity.
