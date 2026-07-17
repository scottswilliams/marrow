# Storage implementation

`marrow-store` is the stripped ordered-byte storage engine retained at lane
B00. It defines a crate-private byte-oriented engine contract and the two
implementors that back it. It orders opaque bytes: it does not parse `.mw`
source, resolve schemas, assign language identity, or interpret key or value
bytes. The logical key/value codecs that give those bytes meaning
were relocated to the path kernel (`marrow-kernel`), which is now the engine's
sole consumer: every logical read and write reaches these bytes through the
kernel's typed sessions.

## Layers

| Layer | Owner |
|---|---|
| Byte-engine contract (`ByteEngine`/`ReadView`/`WriteTxn`, `CommitOutcome`) | `engine.rs` |
| Typed owner-local errors (`StoreError`) | `error.rs` |
| In-memory backend | `mem.rs` |
| Native redb backend (panic-contained adapter, integrity audit) | `redb.rs` |
| Shared backend conformance laws | `conformance.rs` (test-only) |
| Bounded scan accumulation | `traversal.rs` |

The public API is the narrow whitelist in `lib.rs` (the engine trait, the two
backends, `CommitOutcome`, `Cell`, `StoreError`), frozen by a compile-time
surface audit; `marrow-kernel` is the only production dependent (enforced by
the workspace DAG gate). The conformance suite keeps the memory and redb
implementations aligned on the same byte-level laws, including the documented
filesystem envelope (fsync-based durability; see the crate docs).

## Whole-entry materialization law

Materializing a whole entry or group (`marrow-kernel`'s `read_record_leaves`, the
single owner shared by the root entry and every group) obeys a bounded-work law: its
engine work is proportional to the entry's *populated* field count, never its
*declared* field width. The read is a structural-tag-bounded range scan over the
node's own contiguous field-leaf cells (`physical::field_leaf_range` — the marker stem
followed by the field tag), so it visits only present leaves and stops at the group,
branch, or next-node boundary. The counted unit is engine scan calls: `O(populated /
page + 1)` — one page per `SCAN_MAX_RECORDS` present leaves plus one boundary read —
flat across declared widths at a fixed present count. A regression to a
per-declared-field probe (one read per declared field, `O(declared)`) is a
release-veto defect for wide sparse resources: it is pinned red-to-green by a
counting-engine law test.

The *value size* of the materialized result is an accepted, measured `O(declared)`:
`EntryValue.fields` is a dense schema-aligned `Vec<Option<_>>` with one slot per
declared field, so its length tracks the declared width, not the present count (also
pinned by a law test). This is a named, deferred representation seam — sparse sorted
`(field-index, value)` slots, which the field-leaf scan already yields in order,
versus an `Rc`-COW record backing — carried at kernel↔VM boundary width because the
dense positional shape is woven through the create/read/replace and index-maintenance
contract; the durable engine-work law above is the release-veto-critical property and
is already `O(populated)`.

## What was deleted at B00

The prototype's logical tree facade (`TreeStore`/`SealedStore`), admission
metadata, catalog rows, structural digests, backup framing, and the `decimal`
value type were deleted with their owners; each returns through its refounding
lane. Inside the redb adapter, the page-level recovery probe and the
process-global panic-hook swap were deleted as release-veto families: a
malformed or torn store now surfaces redb's own open error through the typed
`StoreError` mapping, with no engine-page parsing above the backend.
