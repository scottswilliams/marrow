# Lane 7: Tree-Cell Store And Engine Profile

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> Lane 7 is integrated on main. Use this file as the store contract record; new
> runtime/tooling work starts from Lane 8 or Lane 10.

Goal: replace source-name physical storage with tree-cell storage keyed by
stable catalog IDs, typed key values, sequence state, index cells, commit
metadata, and an explicit engine profile.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-07-tree-cell-store`

Status: integrated on main; Lane 8 may consume the tree-cell address and value
codec APIs.

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
fixtures. It must delete source-name physical-key paths, raw archive production
paths, and in-memory-list production assumptions in its area instead of leaving a
second storage model for a later lane.

Before handing the lane to review:

- split engine substrate, tree-cell address encoding, commit metadata, index
  cells, archive/backup inputs, and conformance helpers by invariant;
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
- raw archive as the portable backup contract;
- backend logic that decides Marrow schema semantics;
- in-memory list materialization where tree, sequence, or keyed-layer state is
  the actual contract.

Production bridge: none inside tree-cell store semantics. The saved-path
backend, path, and raw archive APIs are tracked as Lane 8 and Lane 10 cleanup
until runtime and tools stop consuming them; they must not become a second
tree-cell storage model. If old data needs repair input later, the owning
evolution/restore lane may add a read-only repair adapter outside production
open/write paths.

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
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-07-tree-cell-store \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store/Cargo.toml \
    -p marrow-store
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
