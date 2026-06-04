# Lane 7: Tree-Cell Store And Engine Profile

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> Lane 7 owns the store crate boundary and the direct production callers needed
> to keep that boundary compiling.

Goal: make the Marrow v0.1 store foundation a typed tree-cell store keyed by
stable catalog IDs, typed key values, sequence state, index cells, commit
metadata, and an explicit engine profile.

Current perfection-pass worktree:
`/Users/scottwilliams/Dev/marrow-lane-07-production-boundary-recovery`

Current target dir:
`/Users/scottwilliams/Dev/.cargo-targets/lane-07-production-boundary-recovery`

## Store Contract

The store crate exposes one production model: `TreeStore` over typed tree-cell
operations. `TreeStore::memory()` opens the in-memory engine,
`TreeStore::open(path)` opens the native redb engine, and
`TreeStore::open_read_only(path)` opens a native read-only handle.

The public store surface consists of catalog IDs, typed data path segments,
sequence positions, scalar keys, canonical scalar value codecs, canonical
identity payload codecs, reference values, enum-member values, engine profile
metadata, commit metadata, exact index entries, exact index tuple pages,
opaque index cursors, and typed store errors.

The ordered-byte substrate is private. `backend`, `mem`, `redb`, and
`traversal` are implementation modules. `path`, `archive`, and `debug_admin`
are not public production modules. No public production API accepts raw
saved-path segments, raw backend key bytes, raw archive chunks, or source-shaped
physical keys. Physical tree-cell key constructors and byte codecs remain
crate-private.

## Production Invariants

- Tree-cell physical keys derive from catalog IDs, typed saved keys, and the
  reserved v0 placement prefix, never source names or source order.
- Reference and enum-member values store catalog IDs, not source spellings or
  enum ordinals.
- redb remains an ordered-byte engine with snapshots, one writer, read-only
  handles, bounded scans, and savepoint transactions.
- Runtime, checker, schema, and tooling semantics do not move into redb or the
  private backend trait.
- Read-only native opens can read existing tree cells and reject write
  capability as `store.read_only`.
- Malformed tree-cell metadata, node markers, reference/enum values, and index
  identity suffixes report `store.corruption`.

## Cleanup Completed Inside Lane 7

Lane 7 deletes the store-local prototype surfaces instead of preserving them for
old tests:

- no public raw saved-path module;
- no public raw backend trait;
- no public memory/redb raw store adapters;
- no raw archive or debug-admin archive module;
- no public saved-path traversal helpers such as presence, roots, child keys,
  sibling seeks, or max-int key scans;
- no raw archive tests or saved-path conformance tests defining production
  behavior.

The private substrate conformance suite covers only ordered-byte engine laws:
exact read/write, prefix delete, prefix scans, cursor-resumed scans,
transactions, rollback, nested savepoints, and read-your-writes behavior. Public
tests cover the typed tree-cell contract through `TreeStore::memory`,
`TreeStore::open`, and `TreeStore::open_read_only`.

## Integration Blockers

There is no active Lane 8 blocker in this worktree. Runtime, CLI, and serve no
longer import removed raw store modules. They use checked facts, checker-owned
source path parsing where text paths are still needed, and typed tree-cell
operations.

Lane 7 is integration-ready only after fresh store gates, affected runtime/CLI
tests, workspace compile, formatter/diff checks, and soundness plus idiom/spec
reviews pass with no production-reachable raw storage surface.

## Consumer Migration Contract

Production consumers must stop importing these prototype surfaces:

- `marrow_store::backend::{Backend, Presence, ScanPage}`;
- `marrow_store::mem::MemStore`;
- `marrow_store::redb::RedbStore`;
- `marrow_store::path::{PathSegment, ChildSegment, encode_path, decode_path,
  parse_path, display_path}`;
- `marrow_store::{archive, debug_admin}` and all raw archive read/write helpers.

The replacement production imports are:

- `marrow_store::StoreError` from the crate root;
- `marrow_store::key::{SavedKey, encode_identity_payload,
  decode_identity_payload_arity}` for typed identity/index key values and the
  canonical identity payload used by identity leaves and unique index entries;
- `marrow_store::cell::{CatalogId, DataPathSegment, SequencePosition}` for
  stable storage IDs, typed nested-data path segments, and sequence positions;
- `marrow_store::tree::{TreeStore, EngineProfile, CommitMetadata,
  TreeReference, TreeEnumMember}` plus exact index page/cursor types and
  reference/enum codecs;
- `marrow_store::value::{Scalar, SavedValue, ScalarType, encode_value,
  decode_value}` for canonical leaf payloads.

Checker-owned source path text remains a CLI/diagnostic convenience for
`marrow data get` and `marrow debug explain`. It is not a store replacement API and
must not be used to construct physical store keys or raw traversal.

Runtime callers must resolve source roots, fields, keyed layers, indexes, enum
members, and referenced stores through checked facts/catalog data before calling
the store. A production write uses typed tree-cell methods such as
`write_node`, `write_leaf`, `delete_leaf`, `write_sequence_position`,
`delete_sequence_position`, `write_data_value`, `delete_data_subtree`,
`write_index_entry`, and `delete_index_entry`; a production read uses
`node_exists`, `read_leaf`, `read_sequence_position`, `read_data_value`,
typed child/neighbor helpers, `read_index_entry`, and `scan_index_tuple`.
Transactions use `begin`/`commit`/`rollback` on `TreeStore`.

There is no replacement for public raw saved-path parsing, raw physical key
encoding, root/child/sibling traversal, raw prefix scans, raw max-int scans, or
raw archive replay. If a new store primitive is needed, it must be typed by
catalog IDs, `DataPathSegment`, and `SavedKey` values rather than source-shaped
path segments or raw backend bytes.

## Verification

Focused store gates use the isolated target dir explicitly:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-07-production-boundary-recovery \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-07-production-boundary-recovery/Cargo.toml \
    -p marrow-store --features native

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-07-production-boundary-recovery \
    cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-lane-07-production-boundary-recovery/Cargo.toml \
    -p marrow-store --all-targets --all-features -- -D warnings
```

Architecture scans must prove raw saved-path/archive APIs are absent from the
store crate public surface. Matches are acceptable only for private substrate
tests, downstream blocker references, or docs that explicitly exclude raw
storage from production semantics.

## Review Lenses

Soundness review attacks raw backend/path/archive reachability, source-name
physical identity, rollback, redb read-only and one-writer behavior, corrupt
metadata, catalog-backed reference/enum values, and typed tree-cell invariants.

Idiom/spec review checks that touched Rust stays small and direct, comments
explain durable rationale only, tests assert focused invariants, docs describe
the v0.1 typed contract, and no compatibility shim exists merely to keep old
tests green.
