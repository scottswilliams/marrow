# Backend Contract

A Marrow v0.1 store is a typed tree-cell store over a private ordered-byte
engine. Production callers use tree-cell operations. They do not receive raw
engine keys, saved-path segment encoders, backend traversal traits, or raw
archive replay APIs. Leaf and index payloads are canonical store value bytes
whose meaning is supplied by checked facts; they are not backend records or raw
saved-path entries.

The public store crate surface is:

- `marrow_store::cell`: catalog IDs, typed nested-data path segments, and
  sequence positions;
- `marrow_store::key`: typed saved key values and the canonical identity
  payload codec;
- `marrow_store::tree`: `TreeStore`, engine profile metadata, commit metadata,
  exact index pages, opaque index cursors, opaque backup cells, typed
  references, and enum-member codecs;
- `marrow_store::value`: canonical scalar value codecs;
- `marrow_store::StoreError` and `marrow_store::Decimal`.

The private engine substrate stores byte keys and byte values in one
byte-lexicographic order. It provides exact read, exact write, prefix delete,
bounded prefix scans, cursor-resumed prefix scans, and savepoint transactions.
The in-memory engine uses `BTreeMap`; the native engine uses redb. The supported
production saved-data backend is the native redb engine. The in-memory engine is
for tests, development, REPLs, short runs, and conformance. It can exercise the
same tree-cell contract, but it is not a production `^` durability profile.
Neither engine parses `.mw`, resolves schemas, distinguishes fields from
indexes, owns catalog identity, or constructs Marrow physical keys.

Backend profiles may grow future residency, tiering, and durability fields, but
those facts remain engine-profile facts. Source still declares `^` saved roots;
a memory-resident or tiered durable backend does not get a separate source
sigil.

Physical tree-cell keys, prefix ranges, and ordered key byte codecs are private
store substrate. Public callers provide typed IDs and key values; the store
constructs physical bytes internally.

There is no public production raw saved-path API in v0.1 store. There is no
public production raw archive API. Backup, tooling, and runtime code consume
the typed tree-cell surface or opaque backup cells owned by the store.

## Tree-Cell Keys

Tree-cell keys are byte-ordered by construction, so memory and redb use ordinary
byte ordering. Physical keys derive from stable catalog IDs, typed key values,
and the reserved empty placement prefix. They never derive from source root
names, member names, index names, enum member spelling, or declaration order.

| Component | Bytes | Meaning |
|---|---|---|
| Placement prefix | `00` | Reserved empty/default placement prefix for v0. |
| Profile byte | `01` | Tree-cell key profile v0. |
| Family tags | `10`, `20`, `30`, `40` | Meta, data, index, and catalog families. Other family tags are reserved. |
| Catalog IDs | `cat_` + 32 lowercase hex | Opaque 128-bit storage ID shape. |
| ID bytes | escaped bytes + `00 00` | IDs use the same escaped byte-run terminator as typed string keys. |
| Node cell | data family + store ID + record-key tuple + `00` | Node marker and prefix for the record's leaf and sequence cells. |
| Leaf cell | node prefix + `10` + member ID | A typed leaf under a node. |
| Sequence cell | node prefix + `20` + member ID + `u64_be(position)` | A sequence element under a node/member, ordered by position. |
| Index cell | index family + index ID + index-key tuple + `00` + record-key tuple + `00` | Sorts by exact index tuple, then record identity. |
| Meta cells | meta family + `01`, `02`, `03`, or `04` | Catalog epoch, layout epoch, engine profile digest, or latest commit metadata. |
| Catalog cells | catalog family + `00` header row, then `10` + `u64_be(ordinal)` per entry | The accepted catalog snapshot: one header row (epoch and digest), one row per entry in catalog order. |
| Prefix ranges | `[prefix, successor(prefix))` | A prefix range includes exactly keys beginning with the prefix. Empty/all-`ff` prefixes have no upper bound. |

Index tuple scans use an exact tuple prefix. Scanning the exact tuple `["a"]`
excludes longer tuples such as `["a", false]`.

## Tree-Cell Operations

`TreeStore` exposes the production storage operations. `TreeStore::memory()`
uses the in-memory development/test engine; `TreeStore::open(path)` and
`TreeStore::open_read_only(path)` use the native redb engine.

- `begin`, `commit`, and `rollback`;
- write/read catalog epoch and layout epoch;
- read/replace the accepted catalog snapshot and read its digest;
- write/read the engine profile digest;
- write/read commit metadata;
- write node markers and test node existence;
- write/read/delete leaves;
- write/read/delete sequence positions;
- write/read/delete typed nested-data values;
- typed record, nested-data, and index child/neighbor helpers using counts,
  first/next/last/previous navigation, or bounded pages rather than unbounded
  child-key lists;
- write/read/delete exact index entries;
- scan an exact index tuple with an opaque cursor;
- visit validated backup cells.

`TreeStore` methods take a shared reference and serialize access through the
store facade. A native read-only open can read existing tree cells and rejects
write-capability operations as `store.read_only`.

An exact index scan matches only the exact tuple it is given: scanning a tuple
prefix returns entries whose index-key tuple equals it exactly and excludes any
longer tuple that extends it. The cursor contains private engine key bytes, but
callers can only receive and return it through the typed index-scan API, and a
cursor is bound to the exact tuple it was issued for; resuming it against a
different tuple prefix is rejected as `store.cursor`, so a paged scan cannot drift
onto another tuple's rows.

Backup traversal returns `TreeBackupCell`, an opaque borrowed data-family cell.
Callers can read its typed data-cell identity, fold its framed checksum, and
write its length-prefixed typed frame, but they cannot read or provide physical
tree-cell key bytes. `TreeBackupCellBuf` reads the same typed frame back. Restore
replays those cells through ordinary typed tree writes after manifest validation;
there is no public physical-cell replay method.

## Metadata

Store-level metadata is written through typed meta cells:

| Meta cell | Tag | Value |
|---|---|---|
| Catalog epoch | `01` | `u64_be(catalog_epoch)` |
| Layout epoch | `02` | `u64_be(layout_epoch)` |
| Engine profile digest | `03` | 8 bytes, the stable v0 engine-profile digest |
| Commit metadata | `04` | Commit id, catalog epoch, layout epoch, source digest, profile digest, changed root/index catalog IDs, and activation evidence |

The v0 engine profile records layout epoch and key profile version `0`. Its
profile fingerprint is deterministic, non-cryptographic, and scoped only to
engine-profile equality; catalog, source, evolution, and commit fences use
`sha256:<hex>` digests instead.

Commit metadata stores the commit id, catalog epoch, layout epoch, activation
counts, and target counts as big-endian `u64` values. Strings, the engine
profile digest, catalog ID lists, per-default activation counts, and per-retire
approval counts are length-prefixed with big-endian `u32` counts or byte
lengths. Catalog IDs remain opaque storage IDs inside metadata values.

Activation evidence binds the durable commit boundary: source digest, evolution
digest, proposal catalog digest, changed root/index IDs, per-default bounded
effect digests plus counts, rebuilt-index count, retire count digest plus per-id
counts, and transform count. These fields are receipts over the committed
activation, not executable migration history and not proposal catalog bodies.
The accepted catalog rows advance in the same transaction as this metadata and
the data and index cells they describe, so there is no post-commit publish step
to resume: a failure before commit rolls every effect back together.

The catalog family is private engine metadata, not language data. No source
declaration, runtime expression, standard-library call, data CLI operation, or
user transaction can address, scan, or mutate catalog rows; they are reached only
through the typed snapshot read/replace operations. A read rebuilds the snapshot
from its rows and recomputes the digest against the stored header, so a tampered
catalog row — even one that decodes into a structurally valid entry — fails closed
as `store.corruption`.

Malformed tree-cell metadata, malformed node markers, malformed tree-cell
reference/enum values, malformed index identity suffixes, and a catalog snapshot
whose recomputed digest does not match its header report `store.corruption`.

## Value Codecs

Scalar leaves use the canonical scalar value codec in `marrow_store::value`.
The Rust API intentionally accepts and returns `Vec<u8>` for leaf, nested-data,
sequence, index, and backup-cell payloads because checked facts supply the type
at the caller boundary. Those bytes are the explicit canonical typed payload
contract, not raw backend records or physical path/value entries. Typed reads
know the scalar type from checked facts, so scalar bytes carry no type tag.
Identity leaves and unique index entries use
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
savepoint. Nested transactions are savepoints: an inner rollback undoes only the
writes made since the inner `begin`, leaving the outer transaction's still-open
writes intact, and an inner commit keeps its writes but leaves them undoable by an
outer rollback. Only the outermost commit makes native writes durable. Unbalanced
`commit` and `rollback` are no-ops.

The in-memory engine snapshots the whole map at each savepoint. The native
engine holds one redb write transaction while the outermost savepoint is open
and records per-level undo journals for nested rollback.

A pinned read snapshot and an open write transaction are mutually exclusive on
one store handle. `begin` fails with `store.transaction` while a read snapshot is
pinned, and `read_snapshot` fails with `store.transaction` while a write
transaction is open. A second pinned read snapshot on the same handle also
fails with `store.transaction`, so dropping one guard cannot unpin another.
Backups and long inspections pin snapshots outside write transactions; source
transactions use the write transaction's read-your-writes view.

## Native Store Duties

The native store records format version `1` in a metadata table. Opening a file
with another version is refused as `store.format_version`. A store file already
held by another writer is refused as `store.locked`. Read-only opens never
create a missing file and use a redb read-only handle.

Opening a damaged native store fails closed with a typed code, never a process
crash. A truncated or torn body — including a file whose damage drives the engine
into a panic during its open-and-repair path — is rejected as `store.corruption`;
a foreign redb file with no Marrow metadata is `store.corruption`; a transient
I/O fault is `store.io`. A store left needing repair by an unclean shutdown is
reported on a read-only open as `store.recovery_required`: a write-capable open
attempts to replay the interrupted commit and reports whether the store opened,
so a store damaged beyond replay still surfaces `store.corruption`.

## Conformance

The private substrate conformance suite keeps memory and redb aligned on:

- value round trips and replacement writes;
- prefix delete and absent delete;
- prefix scan order, bounded scans, and cursor-resumed scans;
- transaction commit, rollback, nested savepoints, read-your-writes scans,
  rejection of overlapping pinned read snapshots and write transactions on one handle, and
  rejection of nested read snapshots on one handle.

Public tree-cell tests assert the production contract: stable catalog-ID
physical keys, typed leaves, sequence cells, exact index entries and scans,
metadata round trips, read-only native behavior, rollback, corruption handling,
catalog-backed references, and catalog-backed enum member values.

## Adapters And Portability

The native redb engine is the only production saved-data backend in v0.1. Any
other storage engine is an adapter that must implement this same tree-cell
contract over its own ordered-byte storage; it may map the ordered tree onto
tables, documents, or engine records internally, but it adds no language
features and exposes no engine query surface. Adapters and other extensions such
as import/export or host bridges ship as separate packages, never in the default
install.

Portability is the typed tree-cell data plus the source and catalog facts needed
to interpret it, not a raw engine byte stream. A backup carries the accepted
catalog rows, catalog IDs, typed values, sequence state, and engine-profile
metadata; generated indexes are derived and rebuilt on restore rather than
trusted as bytes. Restore reconstructs the data, metadata, and catalog rows in
one transaction, so a restored store opens at its accepted catalog with no
file-publish step to resume. Backups are portable across conforming backends at
the same layout and value-codec version.
