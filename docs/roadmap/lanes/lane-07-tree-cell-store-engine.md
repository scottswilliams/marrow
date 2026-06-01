# Lane 7: Tree-Cell Store And Engine Profile

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> Store conformance planning can run early; production key-shape code and
> semantic store fixtures wait for Lane 6 catalog identity.

Goal: replace source-name physical storage with tree-cell storage keyed by
stable catalog IDs, typed key values, sequence state, index cells, commit
metadata, and an explicit engine profile.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-07-tree-cell-store`

Status: read-only planning and engine-substrate checks may start now; production
key work waits for Lane 6.

## Parallel Safety

Read-only planning and engine-substrate checks may run beside Lane 5 and Lane 6.
Allowed early checks cover ordered bytes, snapshots, one-writer behavior,
rollback, typed engine errors, and read-only opens. Production key encoding,
tree-cell addresses, index cells, typed-reference encoding, archive format, and
redb layout changes wait until store IDs, catalog epoch, and proof-ledger
contracts are available on main.

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

Temporary bridge allowed: a compatibility reader may exist only if it is named
as an evolution/repair input and cannot write new production data.

## TDD Start

Start with engine-substrate checks that do not construct Marrow identity:

- snapshot isolation and one-writer behavior;
- rollback;
- typed engine errors;
- read-only opens cannot acquire writer capability.

After Lane 6 is integrated, add semantic tree-cell checks:

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

## Integration Gate

Run the full central gate. Add scans for source-name key construction:

```sh
rg -n 'path|root|field|index|enum|name' \
    /Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store/crates/marrow-store/src
```

Every production match must be typed metadata, debug rendering, or a rejection
test; source spelling cannot be the production key identity.

## Starter Prompt

Continue Marrow v0.1 Lane 7 in `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store`.
Use branch `lane-07-tree-cell-store`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-07-tree-cell-store`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
If Lane 6 catalog identity is not integrated, limit work to read-only design and
engine-substrate checks only: ordered bytes, snapshots, one writer, rollback,
typed engine errors, and read-only opens. Do not write semantic store fixtures,
stable-ID physical-key tests, typed-reference tests, index-cell tests, archive
format changes, or tree-cell address code until Lane 6 lands. Once dependencies
land, implement stable-ID tree-cell storage, commit metadata, sequence/index
cells, and engine profile behavior. Leave the worktree dirty for soundness and
idiom/spec review.
