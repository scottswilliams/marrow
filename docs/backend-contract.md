# Backend Contract

A Marrow store is an ordered-byte engine plus a Marrow-owned tree-cell key
profile above it. The engine stores opaque byte keys and byte values in one
byte-lexicographic order and provides traversal, scan, and transaction
operations over that order. It does not parse `.mw`, resolve schemas, know
fields from indexes, or construct physical Marrow keys.

Two backends implement the same contract and pass the same conformance suite:

- the in-memory store (`memory`), a `BTreeMap` used for short runs, tests,
  and the default when a project pins no backend;
- the native store (`native`), persisted with [redb](https://docs.rs/redb)
  in a single `marrow.redb` file under the project's `dataDir`.

Marrow tree-cell key construction lives above those engines. The v0 physical
key-construction substrate defines data, index, sequence, catalog/meta, and blob
cell addresses. `Backend` traversal uses the saved-path operations below;
current runtime callers and CLI inspection still consume that raw surface until
their owning lanes replace it. Raw archive access is debug/admin only. The typed
tree-cell store facade is the production boundary for tree-cell writes: it
constructs physical keys through `CellKey` and exposes narrow operations for
nodes, leaves, sequence positions, exact index entries, exact index tuple scans,
and metadata.
Tree-cell data/index/sequence keys derive from stable catalog IDs and typed key
values; no physical tree-cell key derives from source root names, member names,
index names, enum member spelling, or source order. Typed record and index
identity uses the same `SavedKey` ordering as Marrow saved keys. Catalog/meta
and blob families occupy disjoint byte ranges from data cells.

## Tree-Cell Keys And Marrow Order

Tree-cell keys are byte-ordered by construction, so redb and memory only need
ordinary byte ordering. A node key is the prefix for its leaf and sequence cells.
An index key sorts by the declared index key tuple first and by record identity
as the tie-breaker. Sequence cells sort by their unsigned position. The
tree-cell API exposes ranges from typed cell keys and exact index tuple prefixes
without letting callers manufacture arbitrary raw byte ranges.

The empty placement prefix is reserved in v0 even though only the default
placement exists today. Future placement profiles must allocate a different
prefix instead of changing the meaning of existing v0 keys.

### V0 Byte Layout

| Component | Bytes | Meaning |
|---|---|---|
| Placement prefix | `00` | Reserved empty/default placement prefix for v0. |
| Profile byte | `01` | Tree-cell key profile v0. |
| Family tags | `10`, `11`, `20`, `30`, `40` | Meta, catalog, data, index, and blob families. Other family tags are reserved. |
| Catalog/blob IDs | `cat_` + 16 lowercase hex + optional `_<n>` | Tree-cell storage ID shape. `n` is positive decimal with no leading zero. Source-like names are rejected before encoding. |
| ID bytes | escaped bytes + `00 00` | IDs use the same escaped byte-run terminator as typed string keys; the accepted shape makes escaping a no-op except for the terminator. |
| Node cell | data family + store ID + record-key tuple + `00` | Node key and prefix for the record's leaf and sequence cells. |
| Leaf cell | node prefix + `10` + member ID | A typed leaf under a node. Other data subcell tags are reserved. |
| Sequence cell | node prefix + `20` + member ID + `u64_be(position)` | A sequence element under a node/member, ordered by position. |
| Index cell | index family + index ID + index-key tuple + `00` + record-key tuple + `00` | Sorts by exact index tuple, then record identity. The first `00` delimiter marks the start of identity keys; the final `00` terminates the entry so exact deletes cannot remove longer identities. |
| Catalog/meta cells | catalog family + storage catalog ID; meta family + `01`, `02`, `03`, or `04` | Catalog entry state and catalog epoch, layout epoch, engine profile digest, or latest commit metadata. Other meta tags are reserved. |
| Blob chunk | blob family + blob ID + `u64_be(chunk)` | Chunked blob storage, ordered by chunk number. |
| Prefix ranges | `[prefix, successor(prefix))` | A prefix range includes exactly keys beginning with the prefix. Empty/all-`ff` prefixes have no upper bound. |

Index tuple scans use an exact tuple prefix: the API appends the identity
delimiter before deriving the range, so scanning the exact tuple `["a"]`
excludes longer tuples such as `["a", false]`.

## Tree-Cell Facade And Metadata

The typed tree-cell facade wraps any `Backend` and keeps tree-cell callers away
from raw physical keys. Node writes create a node marker at the typed node key.
Leaf, sequence, and index methods read, write, and delete only their exact
`CellKey` addresses. An absent leaf reads as absent even when the node marker
exists. Exact index tuple scans derive the range from
`CellKey::index_tuple_prefix(...).range()` and return only entries under that
typed tuple prefix. They are paged: a caller supplies a limit and receives an
opaque cursor only when more entries remain.

Store-level metadata is written through typed meta cells:

| Meta cell | Tag | Value |
|---|---|---|
| Catalog epoch | `01` | `u64_be(catalog_epoch)` |
| Layout epoch | `02` | `u64_be(layout_epoch)` |
| Engine profile digest | `03` | 8 bytes, the stable v0 engine-profile digest |
| Commit metadata | `04` | commit id, catalog epoch, layout epoch, profile digest, changed root catalog IDs, and changed index catalog IDs |

The v0 engine profile records the layout epoch and key profile version `0`. Its
digest is a deterministic FNV-1a 64-bit digest over a fixed profile label, the
key profile version, and the big-endian layout epoch. The digest is stored as
big-endian bytes and can also be rendered as a fixed 16-character lowercase hex
string.

Commit metadata stores the commit id, catalog epoch, and layout epoch as
big-endian `u64` values. The engine profile digest and catalog ID lists are
length-prefixed with big-endian `u32` counts or byte lengths. Catalog IDs remain
opaque `cat_<16 lowercase hex>[_n]` values inside the metadata value; they do
not become source spellings or physical saved paths.

Malformed tree-cell metadata or index cells report `store.corruption`. The
saved-path decoder's `store.corrupt_path` code is reserved for malformed saved
path keys.

## Tree-Cell Value Codecs

Scalar leaves still use the canonical scalar value codec described in
`marrow_store::value`: typed reads know the scalar type from checked facts, so
those bytes carry no type tag.

Tree-cell references and enum-member values use catalog-backed codecs because
their durable meaning is not a scalar spelling:

| Value | Bytes |
|---|---|
| Store reference | version `00`, referenced store catalog ID, identity key count, then each identity key as a length-prefixed `SavedKey` byte run. Lengths and counts are big-endian `u32` values. |
| Enum member | version `00`, enum catalog ID, member catalog ID. |

Each catalog ID is the big-endian length-prefixed opaque `cat_<16 lowercase hex>[_n]`
string from the accepted catalog. Reference bytes therefore distinguish two
stores with identical key values, and enum bytes distinguish members by stable
catalog identity instead of declaration order. Source root names, member names,
enum member spelling, and source order are not inputs to these codecs.

## Current Saved-Path Operations

A saved path is a sequence of segments — a root, identity record keys, named
members (fields, child layers, index names), and index keys inside a layer or
index. This text-shaped path encoding is the `Backend` traversal surface and is
also what CLI inspection and debug/admin raw archive operations expose. It is not the
tree-cell physical key identity. Each segment encodes to a self-delimiting byte
run, and a path's bytes are its segments concatenated. The encoding makes raw
byte-lexicographic order exactly Marrow order, so a backend that merely sorts
bytes yields Marrow order with no custom comparator and regardless of any host
locale or collation.

The order the bytes encode:

- at one tree level, record keys sort before named members (a record-key
  segment carries a lower kind tag than a named segment);
- keys sort by type then value: booleans (false before true), then
  integers, then dates, instants, and durations, then strings, then bytes;
- integer keys sort by numeric value, not text — `^books(1)` precedes
  `^books(10)`, and negative keys precede positive ones — because integers
  encode as sign-flipped big-endian bytes;
- names sort by UTF-8 byte order;
- a shorter string/bytes value sorts before a longer one that extends it.

An encoded ancestor is a byte-prefix of every descendant, and segment
terminators stop unrelated paths from sharing a prefix, so "the subtree at
`path`" is exactly "the stored keys that start with `path`'s bytes."

The store decodes an immediate child segment to a `ChildSegment` — either a
`Key` (a record or index key) or a `Name`. It cannot tell a field, child layer,
or index name apart from the bytes alone; all three are one `Name`. Telling
them apart is the schema's job.

## Operations

Every read returns owned bytes (so a persistent backend can copy them out of a
transaction guard), and every operation is fallible (so a persistent backend
can report I/O and corruption the in-memory store never meets).

| Operation | Returns | Behavior |
|---|---|---|
| `read(path)` | `Option<Vec<u8>>` | The exact value at `path`, or `None` when nothing is stored there. Absence is never a stored sentinel — an unpopulated path simply has no entry. |
| `write(path, value)` | — | Store `value` at `path`, replacing any value already there. |
| `delete(path)` | — | Remove the value at `path` and every value below it (the whole subtree). Deleting an absent path is a no-op. |
| `presence(path)` | `Presence` | Whether `path` holds a value, children, both, or neither (see below). |
| `child_keys(path)` | `Vec<ChildSegment>` | The distinct immediate children directly below `path`, in Marrow order. |
| `child_keys_rev(path)` | `Vec<ChildSegment>` | The same children in reverse Marrow order — the exact reverse of `child_keys`, run backward over a double-ended range, not reversed after the fact. |
| `next_sibling(parent, after)` | `Option<ChildSegment>` | The immediate key child of `parent` directly following the child segment `after`, or `None` when `after` is the last key child. Skips gaps and steps over `after`'s whole subtree. |
| `prev_sibling(parent, before)` | `Option<ChildSegment>` | The mirror of `next_sibling`: the immediate key child directly preceding `before`, or `None` when `before` is the first key child. |
| `first_child(parent)` | `Option<ChildSegment>` | The lowest immediate key child of `parent`, or `None` when it has none. The bare-layer entry point for forward navigation. |
| `last_child(parent)` | `Option<ChildSegment>` | The highest immediate key child of `parent`, or `None` when it has none. The bare-layer entry point for reverse navigation. |
| `scan(path, limit)` | `ScanPage` | Up to `limit` `(path, value)` pairs in the subtree at `path`, in Marrow order, including the value at `path` itself when present. |
| `roots()` | `Vec<String>` | The distinct saved root names, in Marrow order. |
| `max_int_record_key(prefix)` | `Option<i64>` | The highest integer record key among the immediate children of `prefix`, or `None` when none decodes to one. |
| `max_int_index_key(prefix)` | `Option<i64>` | The highest integer index key among the immediate children of `prefix` (positions inside a keyed layer), or `None`. |
| `begin` / `commit` / `rollback` | — | Savepoint control (see [Transactions](#transactions-as-a-savepoint-stack)). |

### Presence

`presence` answers from whether `path` has its own value and whether any key
lies strictly below it, giving four states:

| State | Value at `path` | Children below `path` |
|---|---|---|
| `Absent` | no | no |
| `ValueOnly` | yes | no |
| `ChildrenOnly` | no | yes |
| `ValueAndChildren` | yes | yes |

A record written whole is `ValueOnly`; once a field is written under it, the
record becomes `ValueAndChildren`; a record that only ever received fields is
`ChildrenOnly`.

### Child-key ordering and dedup

`child_keys` walks the subtree once and returns one entry per immediate child,
in Marrow order. Several descendants under the same immediate child collapse to
a single entry — `^seq(1).a` and `^seq(1).b` make `^seq` report `1` once. The
result interleaves keys and names in their encoded order, so record keys come
before named members at that level, integer keys are numeric-ordered, and names
are UTF-8 ordered.

### Ordered navigation

`child_keys_rev`, `next_sibling`, `prev_sibling`, `first_child`, and `last_child`
serve the runtime's ordered navigation (`reversed(...)`, `next()`, `prev()`).
They navigate one *key* level: each returns key children — record keys under a
keyed root, index keys under a keyed layer — and skips any named member (a
declared index, field, or child layer), which sorts after the key children. So
stepping past the last key child reports `None`, the catchable edge, rather than
landing on a trailing index name. The sibling seeks step over the bound child's
whole subtree in one stop (a child with its own descendants is never returned as
a grandchild) and skip deleted holes, returning the nearest stored key neighbor.
Each runs as an `O(k)` walk over a double-ended range — `child_keys_rev` and the
reverse seeks run it backward — so a backend reads only as far as the answer
needs, not the whole subtree.

### Bounded `max` lookups

Integer keys of one kind under a prefix form a single contiguous numeric-ordered
byte band. `max_int_record_key` and `max_int_index_key` range over that band and
take its last entry, so they answer in `O(log n)` without materializing every
child. They ignore non-integer and string keys, and they distinguish the two
key positions: a root has record keys, a keyed child layer has index positions,
and one never bleeds into the other's answer. These back the runtime's
`nextId`-style allocation.

### Bounded scans

`scan(path, limit)` returns a `ScanPage`:

```rust
struct ScanPage {
    entries: Vec<(Vec<u8>, Vec<u8>)>, // (encoded path, value), in Marrow order
    truncated: bool,                  // true iff more remained past the limit
}
```

It walks the subtree at `path` in order and stops at `limit` entries, setting
`truncated` when more remained. A `limit` at or above the subtree's size returns
everything with `truncated == false`; scanning from the empty prefix walks the
whole store. Backends enforce no key or value size limit, so a scan is bounded
only by its `limit` argument.

## Transactions As A Savepoint Stack

`begin` opens a savepoint; `commit` and `rollback` close the innermost one.
Nested `begin`s stack, so nesting is savepoints: an inner `rollback` undoes
only the inner level, and an inner `commit` merely folds the inner level's
writes outward onto the still-open outer transaction. Only the outermost
`commit` makes writes durable, and the outermost `rollback` discards the whole
transaction. Within an open transaction, reads see their own writes
(read-your-writes), and that applies to traversal too — `presence`,
`child_keys`, and `scan` all reflect staged writes before the commit.

A `commit` or `rollback` with no open savepoint is a no-op, not an error:
callers pair `begin` with `commit`/`rollback`, so a stray one is harmless misuse.

The two backends reach the same behavior differently:

- The in-memory store snapshots the whole map on `begin`, drops the snapshot on
  `commit`, and restores it on `rollback`. Cloning the map per savepoint is
  intentional for a small reference store.
- The native store holds one redb write transaction for the life of the
  outermost `begin`, and keeps a per-level undo journal of pre-images.
  An inner `rollback` replays its journal in reverse against the open
  transaction; an inner `commit` moves its journal outward; the outermost
  `commit` commits the redb transaction; the outermost `rollback` aborts it.
  (redb savepoints cannot be created once a transaction has written, hence the
  journal.) Outside any transaction, each `write` and `delete` is its own short,
  immediately durable redb transaction.

## Presence-Equal Backends: The Conformance Suite

A single reusable suite (`conformance::run_all`) drives the same laws against
fresh stores from a factory, and both backends run it as a test
(`mem_store_passes_the_conformance_suite`, `redb_store_passes_the_conformance_suite`).
The laws cover:

- value round-trips, and `write` replacing an existing value;
- the four presence states;
- subtree delete, and delete of an absent path as a no-op;
- child-key ordering and dedup, for integer, string, field, and mixed children;
- ordered navigation — `child_keys_rev` as the exact reverse of `child_keys`,
  and `next_sibling`, `prev_sibling`, `first_child`, `last_child` returning the
  adjacent or edge *key* child while skipping gaps, stepping over subtrees, and
  passing over trailing named members (a declared index) to the catchable edge;
- `max_int_record_key` / `max_int_index_key` returning the highest integer key,
  ignoring non-integer and named children, handling negatives, agreeing with the
  full child walk, and keeping record keys separate from index positions;
- ordered roots, deduped;
- bounded scans returning only the subtree, in order, truncating at the limit;
- a corrupt path surfacing as a typed `store.corrupt_path` error;
- transaction laws: a committed transaction keeps its writes; a rolled-back one
  discards them; an unbalanced `commit`/`rollback` is a no-op; nested savepoints;
  inner-commit-then-outer-rollback discarding everything; three-level nesting
  with a middle commit and outer rollback; and a transaction seeing its writes in
  traversal.
## Native-Store Responsibilities

The persistent backend can fail and corrupt where the in-memory store cannot, so
it carries extra duties. Each maps to a stable `store.*` code (see
[Errors](error-codes.md)).

- Format version. The native store records an on-disk format version
  (currently `1`) in a small metadata table. Opening a file that records a
  different version is refused as `store.format_version` — it is not
  auto-converted or misread. A brand-new file is stamped with the version on
  creation.
- Lock. redb holds an OS lock on the file, so a second writer for an open
  store is refused as `store.locked` rather than racing it. Read-only inspecting
  opens use redb's read-only handle, may coexist with other read-only opens, and
  release their shared read access when dropped so they do not block a subsequent
  read-write open. Write-capability operations through a read-only handle are
  refused as `store.read_only`.
- Corruption. A file that is not a Marrow store is rejected rather than
  adopted: an existing redb file with tables but no Marrow metadata is
  `store.corruption`, and a file that is not a valid database at all surfaces as
  `store.io` ("invalid data"). A read-only open never creates a missing file.

`store.corrupt_path` is reported by either backend when a *stored key* does not
decode as a valid segment sequence — the data is malformed, not the engine.
Malformed tree-cell metadata and malformed tree-cell index identity suffixes
report `store.corruption`, because they are not saved-path keys.
Backends enforce no key/value size limit, so `store.limit` comes only from
Marrow framing layers such as archive chunks or tree-cell metadata, never from a
backend `read`/`write`.

## Archives And Backup Boundary

The raw archive is an ordered-byte saved-path stream behind a small manifest
(magic, format version, record count). It is portable between the memory and redb
engines for debug/admin saved-path transfer, but it is not the typed backup
contract for tree-cell Marrow data.

Portable backup/restore belongs at the tree-cell boundary: catalog IDs, typed
keys, typed values, index cells, sequence cells, catalog epochs, and blob chunks
must be interpreted through the Marrow storage profile rather than by copying a
raw saved-path stream. The portable backup format over this profile is a
separate backup-format contract.

## Inspecting The Store From The CLI

`marrow data` exposes the backend's read operations over a project's saved tree.
Inspection is read-only and never creates the store — a project that has not
yet written reports no saved data and leaves no `marrow.redb` behind. All
subcommands accept `--format text|json|jsonl` (text is the default). These
commands currently expose raw encoded saved paths and bytes, with no field or
index interpretation. For each subcommand's full output shapes, see
[data-tools.md](data-tools.md).

```console
$ marrow data roots ./proj
^books

$ marrow data stats ./proj
roots: 1
records: 4

$ marrow data dump ./proj
^books(1).author	Herbert
^books(1).title	Dune
^books(2).author	Simmons
^books(2).title	Hyperion

$ marrow data get ./proj '^books(1).title'
Dune

$ marrow data get ./proj '^books(99).title'
(absent)
```

`dump` is the raw ordered stream; record keys are numeric-ordered (`^books(1)`
before `^books(10)`), and the `jsonl`/`json` forms add base64 `path_b64` and
`value_b64` for the raw bytes. `data integrity`
verifies stored values against the project's checked schema, exiting `1` on a
finding (`data.decode`, `data.orphan`, or a `store.corrupt_path` key). `data
diff` and `data load` are outside this backend contract.

These commands are raw backend inspection. Application access to saved data is
Marrow code over typed resources — see
[Resources And Storage](language/resources-and-storage.md) — never a
backend-specific API.
