# Backend Contract

A Marrow store is a dumb ordered path/value backend. It maps encoded saved
paths to encoded value bytes, keeps them in one byte-lexicographic order, and
serves a fixed set of traversal and transaction operations over that order. It
does not parse `.mw`, resolve schemas, know fields from index keys, or maintain
indexes. Those are the checker's and runtime's job, above the store.

Two backends implement the same contract and pass the same conformance suite:

- the in-memory store (`memory`), a `BTreeMap` used for short runs, tests,
  and the default when a project pins no backend;
- the native store (`native`), persisted with [redb](https://docs.rs/redb)
  in a single `marrow.redb` file under the project's `dataDir`.

Because the store is path/value only, the same encoded stream restores
byte-for-byte into either backend (see [Archives And Portability](#archives-and-portability)).

## Encoded Paths And Marrow Order

A saved path is a sequence of segments — a root, identity record keys, named
members (fields, child layers, index names), and index keys inside a layer or
index. Each segment encodes to a self-delimiting byte run, and a path's bytes
are its segments concatenated. The encoding makes raw byte-lexicographic order
exactly Marrow order, so a backend that merely sorts bytes — a `BTreeMap`, or
redb's `&[u8]` keys — yields Marrow order with no custom comparator and
regardless of any host locale or collation.

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
- `max_int_record_key` / `max_int_index_key` returning the highest integer key,
  ignoring non-integer and named children, handling negatives, agreeing with the
  full child walk, and keeping record keys separate from index positions;
- ordered roots, deduped;
- bounded scans returning only the subtree, in order, truncating at the limit;
- dump and restore reproducing the store byte-for-byte;
- a corrupt path surfacing as a typed `store.corrupt_path` error;
- transaction laws: a committed transaction keeps its writes; a rolled-back one
  discards them; an unbalanced `commit`/`rollback` is a no-op; nested savepoints;
  inner-commit-then-outer-rollback discarding everything; three-level nesting
  with a middle commit and outer rollback; and a transaction seeing its writes in
  traversal.

Holding both stores to one suite is why a dump from one backend restores
faithfully into the other.

## Native-Store Responsibilities

The persistent backend can fail and corrupt where the in-memory store cannot, so
it carries extra duties. Each maps to a stable `store.*` code (see
[Errors](error-codes.md)).

- Format version. The native store records an on-disk format version
  (currently `1`) in a small metadata table. Opening a file that records a
  different version is refused as `store.format_version` — it is not auto-migrated
  or misread. A brand-new file is stamped with the version on creation.
- Lock. redb holds an OS lock on the file, so a second writer for an open
  store is refused as `store.locked` rather than racing it. A read-only inspecting
  open releases the lock when it drops, so it does not block a later read-write
  open.
- Corruption. A file that is not a Marrow store is rejected rather than
  adopted: an existing redb file with tables but no Marrow metadata is
  `store.corruption`, and a file that is not a valid database at all surfaces as
  `store.io` ("invalid data"). A read-only open never creates a missing file.

`store.corrupt_path` is reported by either backend when a *stored key* does not
decode as a valid segment sequence — the data is malformed, not the engine.
Backends enforce no key/value size limit, so `store.limit` comes only from
archive framing, never from a `read`/`write`.

## Archives And Portability

An archive is the store's whole-tree dump — the ordered `(path, value)`
pairs `scan` yields from the empty prefix — behind a small manifest (magic,
format version, record count). Paths and values are Marrow's canonical encoded
bytes, independent of any engine's files, so two archives of equal data are
byte-identical and an archive restores into either backend. Restore replays the
records inside one transaction, so a target either gains the whole archive or is
left unchanged.

`marrow restore` writes into an empty target only; a non-empty target fails
with `restore.not_empty`. Restoring over existing data — replace, merge, and
repair modes — is deferred (see [future/cli.md](future/cli.md)).

## Inspecting The Store From The CLI

`marrow data` exposes the backend's read operations over a project's saved tree.
Inspection is read-only and never creates the store — a project that has not
yet written reports no saved data and leaves no `marrow.redb` behind. All
subcommands accept `--format text|json|jsonl` (text is the default). The store
is path/value only, so these commands show raw encoded paths and bytes, with no
field or index interpretation. For each subcommand's full output shapes, see
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

`dump` is exactly the canonical ordered stream `backup` writes; record keys are
numeric-ordered (`^books(1)` before `^books(10)`), and the `jsonl`/`json` forms
add base64 `path_b64` and `value_b64` for the raw bytes. `data integrity`
verifies stored values against the project's checked schema, exiting `1` on a
finding (`data.decode`, `data.orphan`, or a `store.corrupt_path` key). `data
diff` and `data load` are deferred (see [future/data-tools.md](future/data-tools.md)).

These commands are raw backend inspection. Application access to saved data is
Marrow code over typed resources — see
[Resources And Storage](language/resources-and-storage.md) — never a
backend-specific API.
