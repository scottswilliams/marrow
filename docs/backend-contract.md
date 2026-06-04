# Backend Contract

A Marrow v0.1 store is a typed tree-cell store over a private ordered-byte
engine. Production callers use tree-cell operations. They do not receive raw
engine keys, saved-path segment encoders, backend traversal traits, or archive
replay APIs. Leaf and index payloads are canonical store value bytes whose
meaning is supplied by checked facts; they are not backend records or raw
saved-path entries.

The public store crate surface is:

- `marrow_store::cell`: catalog IDs, typed nested-data path segments, and
  sequence positions;
- `marrow_store::key`: typed saved key values and the canonical identity
  payload codec;
- `marrow_store::tree`: `TreeStore`, engine profile metadata, commit metadata,
  exact index pages, opaque index cursors, typed references, and enum-member
  codecs;
- `marrow_store::value`: canonical scalar value codecs;
- `marrow_store::StoreError` and `marrow_store::Decimal`.

The private engine substrate stores byte keys and byte values in one
byte-lexicographic order. It provides exact read, exact write, prefix delete,
bounded prefix scans, cursor-resumed prefix scans, and savepoint transactions.
The in-memory engine uses `BTreeMap`; the native engine uses redb. Neither
engine parses `.mw`, resolves schemas, distinguishes fields from indexes, owns
catalog identity, or constructs Marrow physical keys.

Physical tree-cell keys, prefix ranges, and ordered key byte codecs are private
store substrate. Public callers provide typed IDs and key values; the store
constructs physical bytes internally.

There is no public production raw saved-path API in v0.1 store. There is no
public production raw archive API. Backup, tooling, and runtime code consume
the typed tree-cell surface or define their own typed contracts above it.

## Tree-Cell Keys

Tree-cell keys are byte-ordered by construction, so memory and redb use ordinary
byte ordering. Physical keys derive from stable catalog IDs, typed key values,
and the reserved empty placement prefix. They never derive from source root
names, member names, index names, enum member spelling, or declaration order.

| Component | Bytes | Meaning |
|---|---|---|
| Placement prefix | `00` | Reserved empty/default placement prefix for v0. |
| Profile byte | `01` | Tree-cell key profile v0. |
| Family tags | `10`, `20`, `30` | Meta, data, and index families. Other family tags are reserved. |
| Catalog IDs | `cat_` + 16 lowercase hex + optional `_<n>` | Opaque storage ID shape. `n` is positive decimal with no leading zero. |
| ID bytes | escaped bytes + `00 00` | IDs use the same escaped byte-run terminator as typed string keys. |
| Node cell | data family + store ID + record-key tuple + `00` | Node marker and prefix for the record's leaf and sequence cells. |
| Leaf cell | node prefix + `10` + member ID | A typed leaf under a node. |
| Sequence cell | node prefix + `20` + member ID + `u64_be(position)` | A sequence element under a node/member, ordered by position. |
| Index cell | index family + index ID + index-key tuple + `00` + record-key tuple + `00` | Sorts by exact index tuple, then record identity. |
| Meta cells | meta family + `01`, `02`, `03`, or `04` | Catalog epoch, layout epoch, engine profile digest, or latest commit metadata. |
| Prefix ranges | `[prefix, successor(prefix))` | A prefix range includes exactly keys beginning with the prefix. Empty/all-`ff` prefixes have no upper bound. |

Index tuple scans use an exact tuple prefix. Scanning the exact tuple `["a"]`
excludes longer tuples such as `["a", false]`.

## Tree-Cell Operations

`TreeStore` exposes the production storage operations. `TreeStore::memory()`
uses the in-memory engine; `TreeStore::open(path)` and
`TreeStore::open_read_only(path)` use the native redb engine.

- `begin`, `commit`, and `rollback`;
- write/read catalog epoch and layout epoch;
- write/read the engine profile digest;
- write/read commit metadata;
- write node markers and test node existence;
- write/read/delete leaves;
- write/read/delete sequence positions;
- write/read/delete typed nested-data values;
- typed record, nested-data, and index child/neighbor helpers;
- write/read/delete exact index entries;
- scan an exact index tuple with an opaque cursor.

`TreeStore` methods take a shared reference and serialize access through the
store facade. A native read-only open can read existing tree cells and rejects
write-capability operations as `store.read_only`.

Exact index scans return only entries in the requested typed tuple range. The
cursor contains private engine key bytes, but callers can only receive and
return it through the typed index-scan API; a cursor from another exact tuple is
rejected as `store.cursor`.

## Metadata

Store-level metadata is written through typed meta cells:

| Meta cell | Tag | Value |
|---|---|---|
| Catalog epoch | `01` | `u64_be(catalog_epoch)` |
| Layout epoch | `02` | `u64_be(layout_epoch)` |
| Engine profile digest | `03` | 8 bytes, the stable v0 engine-profile digest |
| Commit metadata | `04` | Commit id, catalog epoch, layout epoch, profile digest, changed root catalog IDs, and changed index catalog IDs |

The v0 engine profile records layout epoch and key profile version `0`. Its
digest is deterministic FNV-1a 64-bit over a fixed profile label, the key
profile version, and the big-endian layout epoch.

Commit metadata stores the commit id, catalog epoch, and layout epoch as
big-endian `u64` values. The engine profile digest and catalog ID lists are
length-prefixed with big-endian `u32` counts or byte lengths. Catalog IDs remain
opaque storage IDs inside metadata values.

Future online activation needs commit metadata to describe the durable commit
boundary, not the last write plan that happened to run inside it. The metadata
surface may grow runtime-generation, activation-job, source/catalog digest, and
adapter/window evidence fields. Those fields remain typed Marrow evidence above
the engine; they do not make raw engine keys, migration ledgers, or backend
history part of the production API.

Malformed tree-cell metadata, malformed node markers, malformed tree-cell
reference/enum values, and malformed index identity suffixes report
`store.corruption`.

## Value Codecs

Scalar leaves use the canonical scalar value codec in `marrow_store::value`.
Typed reads know the scalar type from checked facts, so scalar bytes carry no
type tag. Identity leaves and unique index entries use
`marrow_store::key::{encode_identity_payload, decode_identity_payload_arity}`.
These payload bytes are part of the typed value contract, not raw backend
records.

Tree-cell references and enum-member values use catalog-backed codecs:

| Value | Bytes |
|---|---|
| Store reference | Version `00`, referenced store catalog ID, identity key count, then each identity key as a length-prefixed `SavedKey` byte run. |
| Enum member | Version `00`, enum catalog ID, member catalog ID. |

Reference bytes distinguish stores with identical key values. Enum bytes
distinguish members by stable catalog identity instead of source order.

## Transactions

`begin` opens a savepoint; `commit` and `rollback` close the innermost
savepoint. Nested transactions are savepoints: an inner rollback undoes only
the inner level, and an inner commit keeps its writes but leaves them undoable
by an outer rollback. Only the outermost commit makes native writes durable.
Unbalanced `commit` and `rollback` are no-ops.

The in-memory engine snapshots the whole map at each savepoint. The native
engine holds one redb write transaction while the outermost savepoint is open
and records per-level undo journals for nested rollback.

## Native Store Duties

The native store records format version `1` in a metadata table. Opening a file
with another version is refused as `store.format_version`. A store file already
held by another writer is refused as `store.locked`. Read-only opens never
create a missing file and use a redb read-only handle.

Corrupt or foreign redb files are rejected as `store.corruption` or `store.io`
depending on whether redb can open the file and Marrow metadata can be read.

## Conformance

The private substrate conformance suite keeps memory and redb aligned on:

- value round trips and replacement writes;
- prefix delete and absent delete;
- prefix scan order, bounded scans, and cursor-resumed scans;
- transaction commit, rollback, nested savepoints, and read-your-writes scans.

Public tree-cell tests assert the production contract: stable catalog-ID
physical keys, typed leaves, sequence cells, exact index entries and scans,
metadata round trips, read-only native behavior, rollback, corruption handling,
catalog-backed references, and catalog-backed enum member values.
