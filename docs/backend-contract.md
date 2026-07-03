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
  exact index pages, opaque index cursors, opaque backup cells, and
  enum-member codecs;
- `marrow_store::value`: canonical scalar value codecs;
- `marrow_store::StoreError` and `marrow_store::Decimal`.

The private engine substrate stores byte keys and byte values in one
byte-lexicographic order. It provides exact read, exact value-prefix read,
exact write, prefix delete, bounded prefix scans, bounded lower/upper prefix
scans, cursor-resumed forward and reverse prefix scans, and flat joined
transactions. The exact value-prefix read copies at most the requested bytes
from one exact key and reports whether the stored value had more bytes; it has
the same transaction and pinned-snapshot visibility as exact read.
The in-memory engine uses `BTreeMap`; the native engine uses redb. The supported
production saved-data backend is the native redb engine. The in-memory engine is
for tests, development, REPLs, short runs, and conformance. It can exercise the
same tree-cell contract, but it is not a production `^` durability profile.
Neither engine parses `.mw`, resolves schemas, distinguishes fields from
indexes, owns catalog identity, or constructs Marrow physical keys.

Deterministic simulation grows by substituting the runtime nondeterminism
provider at host/tool boundaries, not by teaching backends to read clocks or
entropy. Tree-cell backends remain deterministic stores of typed facts; runtime
clock capture and store UID minting consume the provider before writing ordinary
typed metadata.

Physical tree-cell keys, prefix ranges, and ordered key byte codecs are private
store substrate. Public callers provide typed IDs and key values; the store
constructs physical bytes internally.

The private backend counting decorator is the canonical cost-conformance oracle:
tests assert operation shape through reads, writes, deletes, scan direction,
bytes moved, entries returned, commits, and commit fsync counts rather than by
timing a backend.

There is no public production raw saved-path API in v0.1 store. There is no
public production raw physical-cell replay API. Backup, tooling, and runtime
code consume the typed tree-cell surface or opaque backup cells owned by the
store. Public backup archive header and chunk helpers are bounded portable-format
framing helpers for the manifest, catalog section, and typed cell stream; they
do not expose physical engine keys or write raw cells.

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
| Nested-data value cell | node prefix + one or more (`30` + member ID or `40` + typed key value) segments + `00` | A typed value below a record node. Member and key path segments are structural; the trailing `00` marks the value cell. Empty nested-data paths use the node cell. |
| Index cell | index family + index ID + index-key tuple + `00` + record-key tuple + `00` | Sorts by exact index tuple, then record identity. |
| Commit metadata cell | meta family + `04` | Latest commit metadata. A store is stamped when `read_commit_metadata()` returns `Some`. |
| Store UID cell | meta family + `05` | `store_<32 lowercase hex>` physical store identity. |
| Structural-digest cell | meta family + `06` + store ID | 128-bit big-endian digest over every committed cell under that store root, restamped each commit that touches the root. The independent completeness anchor for the data family. |
| Catalog cells | catalog family + `00` header row, then `10` + stable catalog ID per entry | The accepted catalog snapshot: one header row (epoch and digest), one row per entry; the entry value carries its catalog ordinal. |
| Prefix ranges | `[prefix, successor(prefix))` | A prefix range includes exactly keys beginning with the prefix. Empty/all-`ff` prefixes have no upper bound. |

Index tuple scans use an exact tuple prefix. Scanning the exact tuple `["a"]`
excludes longer tuples such as `["a", false]`.

## Tree-Cell Operations

`TreeStore` exposes the production storage operations. `TreeStore::memory()`
uses the in-memory development/test engine; `SealedStore::open(path, AccessMode)`
is the only source of a native redb-backed handle.

- `begin`, `commit`, and `rollback`;
- read/replace the accepted catalog snapshot and read its digest;
- write/read commit metadata;
- write node markers and test node existence;
- write/read/delete leaves;
- write/read/delete sequence positions;
- write/read/delete typed nested-data values, including bounded exact value
  prefixes through `TreeStore::read_data_value_prefix`;
- typed record, nested-data, and index child/neighbor helpers using counts,
  first/next/last/previous navigation, or bounded pages rather than unbounded
  child-key lists;
- write/read/delete exact index entries;
- scan an exact index tuple with an opaque cursor;
- scan a non-unique index exact prefix plus one trailing ordered key range with
  an opaque cursor, forward or reverse;
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

A bounded index range scan starts from an exact index-key prefix and constrains
only the next key component. The lower bound is inclusive when present; the upper
bound is exclusive for `..` and inclusive for `..=` by scanning to the successor
of the upper key prefix. Inverted bounds normalize to an empty page. Returned
rows stay ordered by encoded index key and identity suffix. Cursors are bound to
the exact prefix and normalized lower/upper byte bounds, not to public path text,
so resuming one bounded range against another reports `store.cursor`.

Store-root keyspace and keyed-layer range traversal use the same typed child
navigation contract: exact leading key components are lowered to the record or
nested-data prefix, the first child seek starts at the lower or upper bound for
the requested direction, and each resumed first/next/previous step stops at the
opposite bound. These scans return stored child keys only; they do not expose raw
engine cursors or materialize unbounded child lists.

`TreeStore::read_data_value_prefix` takes the same typed store id, identity, and
nested-data path as `read_data_value`. It returns no physical key bytes. When a
value exists it copies at most the effective limit bytes and reports whether the
stored value bytes were truncated by that limit. It observes the same
transaction and pinned-snapshot version as exact tree-cell reads.

Backup traversal returns `TreeBackupCell`, an opaque borrowed data-family cell.
Callers can read its typed data-cell identity, fold its framed checksum, and
write its length-prefixed typed frame, but they cannot read or provide physical
tree-cell key bytes. `TreeBackupCellBuf` reads the same typed frame back. Restore
replays those cells through ordinary typed tree writes after manifest validation;
there is no public physical-cell replay method. The public archive header and
chunk helpers frame bounded portable sections; they do not decode, expose, or
replay physical tree-cell keys.

## Metadata

Store-level metadata is written through typed meta cells:

| Meta cell | Tag | Value |
|---|---|---|
| Commit metadata | `04` | Commit id, catalog epoch, layout epoch, source digest, profile digest, changed root/index catalog IDs |
| Store UID | `05` | `store_<32 lowercase hex>` physical store identity |
| Structural digest | `06` + store ID | 128-bit big-endian digest over every committed cell under that store root |

Each commit re-seals one **commit record** binding `{uid, epoch, catalog digest,
active roots, per-root digests}` under a single content seal. Each per-root digest
is the wrapping sum of one 128-bit hash per cell, each hash taken over the cell's
full physical key — its root, record identity, and field path — together with its
stored value bytes. The combiner is commutative and associative, so the digest is
order-independent and a write maintains it in constant time: a write adds the new
cell's hash and, on overwrite or delete, subtracts the prior one, never rescanning
the root. The digests are the independent completeness anchor for the data family:
because the data cells are their own derivation, a backend page that silently drops
a cell, truncates a record range, or rewrites a stored value with bytes that still
decode shifts every enumeration with no structural fault. The record is recomputed
from the replayed cells, not carried, on restore, so a restored store re-verifies.

The **store-open** path validates the commit record in O(1): decoding recomputes
its content seal, so a flip of any bound field fails closed, and the record is
cross-checked against the store's own uid, commit stamp, and catalog snapshot, so a
flip in one of those auxiliary cells fails closed too. The `run` and `serve`
admission opens and the point-read inspection share this O(1) witness; an
enumerating read additionally reconciles the root it walks against that root's
sealed digest (`verify_root_digest_once`), so a btree-corrupt root fails closed as
`store.corruption` while untouched roots stay unscanned. The full O(N)
re-derivation — re-deriving every root's digest from a data-family scan, reconciling
the scan against the point-lookup descent typed reads use (so an interior-separator
flip that misroutes a lookup past a committed cell the scan still covers fails
closed), and the same reconciliation over the derived index family (each committed
index entry reachable by an index seek, and each entry's stored identity matching
its redundant copy) — is the deep pass `data integrity`, `backup`, and `data
recover` run, never the admission open. `data stats` and `data dump` traverse the
whole store and share that deep pass through the inspection open.

The record cannot witness a corruption that drops the record itself: a flip that
rolls the store back to its empty initial commit presents zero records and zero
digests, so the per-root cross-check visits nothing and passes vacuously. The independent witness is the committed
`marrow.lock`, a separate durable file recording the epoch high-water, the
accepted catalog roots, and the epoch each root became active at. `backup`, `data
integrity`, `data stats`, and `data recover` cross-check the roots a **present**
store presents against the roots the lock records, judging each committed root by
its recorded activation epoch: a store whose own epoch has reached a root's
activation must present it — missing means the store lost durable identity and
fails closed as `store.corruption`, whatever the lock's high-water — while a store
still below the activation legitimately never held the root and is the
store-behind case the advance paths resolve by activating the store. A root with
no recorded activation always reads as a loss when missing, the fail-closed
default. An **absent** store body is the disposable-store case, not a loss: the
write paths (`run`, `evolve apply`, `serve --write`) seed an empty store from the
committed identity, so an absent store is never `store.corruption`. With no
committed lock the store has no recorded baseline to contradict, the separate
missing-lock case. Neither witness can see a rollback that resets the whole
store body to an old epoch: the sealed commit record reverts along with the data,
so such a store is locally indistinguishable from a checkout that never advanced
past that epoch, and a root activated later reads as legitimately absent even when
the rollback destroyed its records. This residual is inherently undecidable from
local state — no store-internal record can witness its own wholesale reversion — so
it stays documented behavior, not corruption: the store reads `store_behind`, the
advance paths resolve it, and `doctor`'s epoch-mismatch advisory names a behind
store honestly (advance or restore; never regenerate the lock from it). The
mitigation is backup/restore plus that advisory. The commit record instead closes
every store-**internal** corruption — a partial rollback, torn value, dropped cell,
re-sealed field disagreeing with an auxiliary cell, or a covered-root drop the
store's own epoch covers all fail closed.

A store is stamped exactly when `TreeStore::read_commit_metadata()` returns `Some`.
The commit stamp is the single durable owner of the stamped catalog epoch,
layout epoch, source digest, and engine-profile digest. The accepted catalog
snapshot remains in the catalog family; its epoch and digest must agree with a
stamped commit when both are present.

The v0 engine profile records layout epoch and key profile version `0`. Its
profile fingerprint is deterministic, non-cryptographic, and scoped only to
engine-profile equality; catalog, source, evolution, and commit fences use
`sha256:<hex>` digests instead.

Commit metadata stores the commit id, catalog epoch, and layout epoch as
big-endian `u64` values. Strings, the engine profile digest, and catalog ID
lists are length-prefixed with big-endian `u32` counts or byte lengths. Catalog
IDs remain opaque storage IDs inside metadata values. Operator receipt counts
from an evolution apply are rendered from the in-memory apply result and are not
persisted in this meta cell.

Commit IDs are dense over the committed sequence. The catalog baseline is
commit `0` on an unstamped store; every later stamped write reads the
predecessor commit metadata inside the same write transaction that writes the
new stamp and records `prior + 1`. A rollback consumes no commit ID, so after
`N` post-baseline committed writes the high-water mark has advanced exactly `N`.

The top-level changed root/index catalog ID lists are per-commit stamp facts:
they describe the data roots and indexes this commit itself touched. An
evolution apply stamps the activation commit's changed IDs there, but a later
managed write replaces them with that write's own changed IDs and does not carry
the activation commit's changed IDs forward. If a stamped commit has no changed
roots or indexes, the lists are empty.
The change-signal fields are `changed_root_catalog_ids` and
`changed_index_catalog_ids`; hosts invalidate cached root and index views from
those catalog-id lists, never from source spellings.

An evolution apply advances the accepted catalog rows, data/index effects, and
commit metadata in one transaction. Replay suppression uses the slim stamp
facts (catalog epoch, source digest, engine profile, and catalog snapshot
digest) plus a recomputed witness gate; the store byte surface does not carry
per-effect default, transform, retire, or index counts or digests. A failure
before commit rolls every effect back together. The committed store catalog
family is the sole write-time authority for accepted identity; after a
successful CLI apply, the project-root `marrow.lock` projection is regenerated
from these committed catalog rows as a one-way, store-to-lock projection.

The catalog family is private engine metadata, not language data, and the live
store is its authority. The project-root `marrow.lock` is a committed projection
of the catalog family, never an input to it: no path rewrites the catalog rows
from the file. No source declaration, runtime expression, standard-library call,
data CLI operation, or user transaction can address, scan, or mutate catalog
rows; they are reached only through the typed snapshot read/replace operations. A
read
rebuilds the snapshot from its rows and verifies the stored header against the
decoded entries. The canonical catalog digest sorts entries by declaration kind
tag, canonical path, stable ID, aliases, lifecycle tag, accepted store-key shape,
accepted store-index shape, and accepted structural signature before hashing, so
declaration order does not change the digest. The accepted contract is the
canonical digest only. A stale row-order header digest or a tampered catalog row
— even one that decodes into a structurally valid entry — fails closed as
`store.corruption`.

Malformed tree-cell metadata, malformed node markers, malformed tree-cell
enum values, malformed index identity suffixes, and a catalog snapshot
whose header digest does not match the decoded entries report `store.corruption`.

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
records. Index components derived from `Id(^store)` fields use the same
order-preserving identity payload with the referenced store's stable catalog ID
as a prefix, so same-shaped identities from different stores cannot collide.

Tree-cell enum-member values use a catalog-backed codec:

| Value | Bytes |
|---|---|
| Enum member | Version `00`, enum catalog ID, member catalog ID. |

Enum bytes distinguish members by stable catalog identity instead of source
order.

## Transactions

`begin` opens a flat write transaction or joins the already-open one. `commit`
closes one joined level, and only the outermost commit makes writes durable.
`rollback` at any joined level aborts the whole open transaction. Unbalanced
`commit` and `rollback` are no-ops.

The in-memory engine snapshots the whole map once when the flat transaction
opens. The native engine holds one redb write transaction while any joined level
is open; rollback aborts that transaction instead of replaying per-level undo
state.

A pinned read snapshot and an open write transaction are mutually exclusive on
one store handle. `begin` fails with `store.transaction` while a read snapshot is
pinned, and `read_snapshot` fails with `store.transaction` while a write
transaction is open. A second pinned read snapshot on the same handle also
fails with `store.transaction`, so dropping one guard cannot unpin another.
Backups and long inspections pin snapshots outside write transactions; source
transactions use the write transaction's read-your-writes view.

## Concurrency

The native redb backend enforces one cross-process store-file lock. Read-only
opens coexist with each other. A read-only open and a read-write open mutually
exclude, in both directions: whichever process opens second is refused with
`store.locked`.

`store.locked` means "The store file is held open by another process (a writer
or a read-only inspection)." Close the other process, then retry. Marrow does
not try to identify whether the holder is a writer or a reader; redb does not
expose that distinction through the lock error.

Within one `TreeStore` handle, a pinned read snapshot and an open write
transaction are also mutually exclusive, but that is an invalid handle state
reported as `store.transaction`. `store.locked` is reserved for the
cross-process file-open contract.

A read-only native handle is an inspection handle: read calls work, while
write-capability operations fail with `store.read_only`. Same-handle
snapshot/write conflicts remain `store.transaction`, not `store.locked`.

## Open-Mode Downgrade

An entry whose checked transitive effect closure proves
`write_effects_reachable = false`, has no reachable transaction block, and has
no pending catalog proposal may execute against a native store opened read-only.
The proof is over lowered direct-effect facts and resolved `CheckedFunctionRef`
callees, so duplicate-name error recovery cannot hide a callee write. A closure
with any reachable saved write or transaction requires a write-capable open.

A first run against a store with no frozen catalog identity is always
write-capable, even when the selected entry is otherwise read-only, because the
catalog baseline writes accepted identity and a commit stamp. A check that binds
an accepted catalog but still carries a pending proposal also remains
write-capable until activation freezes that proposal. After identity is frozen,
no proposal is pending, and the entry closure is read-only and transaction-free,
the host may choose a read-only open under the native lock contract; read-only
opens coexist with other read-only opens and still exclude writers.

Marrow v0.1 does not include a built-in durable outbox engine. When application
code needs an external side effect to follow saved-data changes, write an
ordinary saved outbox record in the same Marrow transaction as the state change,
commit, and have a separate worker read, send, and mark that record idempotently.
The backend makes the saved outbox record durable with the transaction; it does
not infer messages or deliver effects.

## Durability And Recovery

Native redb write transactions explicitly pin `Durability::Immediate` when the
transaction begins. An unbracketed single tree-cell write is still its own redb
write transaction, so its engine-relative cost includes one durable commit fsync;
a source `transaction` brackets many tree-cell writes into one outer commit.

Marrow uses redb's one-phase commit posture: the store does not call
`WriteTransaction::set_two_phase_commit`, expose a posture flag, or add a
configuration switch. A successful native commit has redb's immediate-durability
guarantee for the database file. When a missing native store file is created and
stamped as a Marrow store, Marrow also fsyncs the containing directory after the
format-stamp commit so the new directory entry is durable.

The residual one-phase risk is a crash that tears the active commit slot and is
misread as committed only if the torn bytes collide with redb's
non-cryptographic slot checksum. The named escalation is enabling
`WriteTransaction::set_two_phase_commit(true)`, which doubles commit fsyncs; that
would be a runtime durability posture change, not a tree-cell format break.

The crash/recovery harness asserts both-or-invisible transaction recovery at
deterministic kill points and with a bounded commit-race soak. Torn or truncated
native store bodies fail closed as `store.corruption`. A store left needing
repair by an unclean shutdown is reported on a read-only open as
`store.recovery_required`; a write-capable open attempts the replay and reports
whether the store opened, so damage beyond replay still surfaces
`store.corruption`.

## Native Store Duties

The native store records format version `1` in a metadata table. Opening a file
with another version is refused as `store.format_version`. A store file already
held by another process (a writer or a read-only inspection) is refused as
`store.locked`. Read-only opens never create a missing file and use a redb
read-only handle.

Opening a damaged native store fails closed with a typed code, never a process
crash. A truncated or torn body — including a file whose damage drives the engine
into a panic during its open-and-repair path — is rejected as `store.corruption`;
a foreign redb file with no Marrow metadata is `store.corruption`; a transient
I/O fault is `store.io`.

## Conformance

The private substrate conformance suite keeps memory and redb aligned on:

- value round trips and replacement writes;
- value-prefix read limits, truncation flags, and snapshot visibility;
- prefix delete and absent delete;
- prefix scan order, bounded scans, and cursor-resumed scans;
- transaction commit, rollback, flat joined nesting, read-your-writes scans,
  rejection of overlapping pinned read snapshots and write transactions on one handle, and
  rejection of nested read snapshots on one handle.

Public tree-cell tests assert the production contract: stable catalog-ID
physical keys, typed leaves, sequence cells, exact index entries and scans,
metadata round trips, read-only native behavior, the native reader/writer lock
matrix, rollback, corruption handling, and catalog-backed enum member values.

## Adapters And Portability

The native redb engine is the only production saved-data backend in v0.1. Any
other storage engine is an adapter that must implement this same tree-cell
contract over its own ordered-byte storage; it may map the ordered tree onto
tables, documents, or engine records internally, but it adds no language
features and exposes no engine data-access surface. Adapters and other extensions such
as import/export or host bridges ship as separate packages, never in the default
install.

Portability is the typed tree-cell data plus the source and catalog facts needed
to interpret it, not a raw engine byte stream. A backup manifest carries
`source_digest`, `catalog_epoch`, `catalog_digest`, `state_digest`,
`store_uid`, the reserved empty `parent_snapshot_digest` sentinel, `engine`,
`commit`, `record_count`, and `archive_checksum`; generated indexes are derived
and rebuilt on restore rather than trusted as bytes. Restore reconstructs the
data, metadata, and catalog rows in one transaction with a fresh store UID, so a
restored store opens at its accepted catalog, and a later CLI command can
regenerate `marrow.lock` as a one-way projection of that snapshot.
Backups are portable across conforming backends at the same layout and
value-codec version.
