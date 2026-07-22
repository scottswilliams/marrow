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

The native backend has separate create-capable and existing-only constructors.
Provisioning is the sole lifecycle path that uses creation and stamps the engine
format. Ordinary open and commit recovery use the same write-capable
existing-only operation. A missing file remains absent; an empty, malformed,
unstamped, foreign, dangling, or unreadable file is refused rather than created
or adopted.

## Commit witness and recovery

The path kernel owns one bounded witness cell. A new witness is a version tag
followed by a checked big-endian `u128` generation. Absence and every exact
16-byte legacy token migrate to generation zero in the disjoint 17-byte tagged
domain; a tagged generation increments without wrap, and exhaustion is a typed
store limit before a write transaction begins. Any other witness encoding is
corruption.

Before beginning a mutating engine transaction, the kernel captures the exact
current witness bytes and derives the proposed next bytes. It stages the proposed
witness in the same transaction as the application writes. A confirmed commit
installs both; a confirmed abort or a failure during commit reconciliation before
the engine commit is known old and leaves the handle usable. An indeterminate engine result poisons the handle and returns one
opaque affine recovery fact owning the exact before state, proposed-after state,
and the persistent lifecycle's store-instance/path scope. The fact has no public
constructor, clone, byte accessor, or serialization.

The native lifecycle resolves that fact while retaining the same advisory owner
lock. It first revokes clean-on-drop, closes the indeterminate engine, freshly
opens the existing engine file at the retained path, performs a full integrity
audit, and asks the kernel to consume and compare the fact. Exact equality with
the proposed state is known new; equality with the captured before state is known
old. A third value, scope mismatch, malformed cell, failed read, failed open, or
failed audit is unknown. Only a known result re-arms clean close and returns the
fresh store owner. Unknown retires it, leaves the descriptor unclean, and retains
the advisory lock until process exit so no later session in the same process can
cross the unresolved boundary. Losing the affine fact and dropping its poisoned
owner takes the same quarantine path.
`OpenStore` keeps that engine and its owner lock private and implements only the
session-opening capability, so safe callers cannot detach a raw engine handle
from the lock. Classification never replays application bytecode and never
creates a return value.

The owner lock excludes cooperating Marrow processes; it does not authenticate
the engine file. The redb open API exposes no durable handle identity, and this
ledger-free recovery cannot distinguish an out-of-band substitution of a
structurally valid foreign store or an exact prior snapshot. It deliberately
does not approximate identity with paths, inode metadata, timestamps, lengths,
entropy, or sampled content. Recovery correctness therefore assumes that no such
substitution occurs while the owner lock is held. Exact substitution and rollback
detection remains an explicit pre-release safety veto for the adversarial QA
campaign; the current implementation is not evidence for that property.

No public whole-store raw-cell visitor or insertion seam exists. Logical backup
and restore remain future lifecycle work and must re-enter through their own
verified canonical format rather than copying this witness lineage.

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
is already `O(populated)`. The decode-side per-read cost rides the same seam: resolving
each scanned leaf to its declared position uses a per-read name map that is `O(declared)`
CPU and allocation, so it too becomes `O(populated)` only under the sparse-slot
representation the seam defers.

## What was deleted at B00

The prototype's logical tree facade (`TreeStore`/`SealedStore`), admission
metadata, catalog rows, structural digests, backup framing, and the `decimal`
value type were deleted with their owners; each returns through its refounding
lane. Inside the redb adapter, the page-level recovery probe and the
process-global panic-hook swap were deleted as release-veto families: a
malformed or torn store now surfaces redb's own open error through the typed
`StoreError` mapping, with no engine-page parsing above the backend.
