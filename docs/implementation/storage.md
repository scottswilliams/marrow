# Storage implementation

`marrow-store` orders opaque bytes. It defines a byte-engine contract, private
to the workspace because `marrow-kernel` is its only dependent, and the two
implementations behind it. Meaning comes from the kernel above it:
`marrow-kernel` owns the codecs that turn a durable path into key
bytes and a value into cell bytes, and every logical read and write reaches the
engine through the kernel's sessions.

## From a path to a cell

A durable read such as `^books[id].title` travels four layers. The compiler
resolves the path to a schema-stable operation in the program image. The
kernel's session turns that operation and the runtime key into an ordered byte
key (`durable/physical.rs`) and asks the engine for the cell or range under it.
The engine (`marrow-store`) returns bytes in key order. The kernel decodes them
back to a value (`codec/`) and hands it to the VM. Writes follow the same path
in reverse, staged inside one engine transaction that commits with the
`transaction` block.

## Layers

| Layer | Owner |
|---|---|
| Byte-engine contract (`ByteEngine`, `ReadView`, `WriteTxn`, `CommitOutcome`, `Cell`) | `engine.rs` |
| Errors (`StoreError`) | `error.rs` |
| In-memory engine (`MemoryEngine`) | `mem.rs` |
| Native redb engine (panic-contained adapter, integrity audit) | `redb.rs` |
| Native engine owner: store directory, advisory lock, two-phase open, quarantine | `native_owner.rs` |
| Bounded scan accumulation (`SCAN_MAX_RECORDS` cells per page) | `traversal.rs` |
| Shared engine conformance laws | `conformance.rs` (test-only) |
| Public surface and its compile-time audit | `lib.rs` |

`lib.rs` exports the engine contract, `StoreError`, `MemoryEngine`, and the
native owner's types; the redb adapter itself is private. A compile-time audit
in `lib.rs` fails if an exported name is removed or renamed. The conformance
suite runs the same byte-level traces over both engines: point reads, writes and
exact removal, the bounded forward scan at its boundary, consuming transactions,
batch limits, and the integrity audit. The filesystem durability envelope is
redb's own and is documented in `redb.rs`.

## One consumer

`marrow-kernel` is the engine's only dependent. The workspace tidy test in
`crates/marrow-codes/tests/tidy.rs` walks the crate graph and fails if any
other crate depends on `marrow-store`. Application code, the VM, and the CLI
therefore hold no raw key, engine handle, or transaction object.

## Native owner

`native_owner.rs` derives `lock` and `store.redb` from one canonical store
directory and keeps the advisory lock inseparable from the engine. Provisioning
calls a create-only operation that stamps the engine format. An open of an
existing store has two phases: acquire the lock on the directory node with no
engine call, then bind the store instance and open the engine under the same
lock. An indeterminate commit quarantines the lock until process exit; the
kernel classifies the outcome as known old, known new, or unknown
([interrupted commits](../operations/README.md#interrupted-commits)). The lock
excludes cooperating Marrow processes and does not authenticate the engine
file; recovery cannot detect an out-of-band substitution of `store.redb`. That
gap is recorded in [project status](../status.md#trust-boundaries); the
[audit](#auditing-a-store) reports what a substituted file's contents disagree
with, not where the file came from.

## Reading a whole entry

Reading a whole entry or group (`marrow-kernel`'s `read_record_leaves`, the
single owner shared by the root entry and every group) does engine work
proportional to the entry's populated field count, never its declared width.
The read is a structural-tag-bounded range scan over the node's own contiguous
field-leaf cells (`physical::field_leaf_range`, the marker stem followed by the
field tag), so it visits only present leaves and stops at the next node
boundary. The counted unit is engine scan calls: one page per `SCAN_MAX_RECORDS`
present leaves plus one boundary read. A per-declared-field probe would make
this `O(declared)`; a counting-engine test fails if that returns.

The value the read produces is `O(declared)`: `EntryValue.fields` is a dense
schema-aligned `Vec<Option<_>>` with one slot per declared field, and the
per-read name map that places each scanned leaf costs the same. A sparse sorted
`(field-index, value)` representation, which the leaf scan already yields in
order, would make both `O(populated)`; the dense shape stays because the
create, read, replace, and index-maintenance contract is written against it.

## Auditing a store

`marrow-kernel`'s `durable/audit.rs` owns the read-only logical walk `marrow
doctor` runs. It is one forward scan over the whole key space, one engine page
at a time, that parses the cell stream in the order `physical.rs` sorts it: each
root's entry family (an entry's marker, its own field leaves, its group leaves,
then its branch descendants to any depth), then the managed-index families, then
the commit witness. Every cell is classified once against fixed tables built
from the projection; a cell that belongs to no declared node or index is a
typed finding. The walk's retained state is a stack of open nodes, one frame per
nesting level and so bounded by the durable depth cap, plus a presence bitmap per
open node's declared fields and a capped finding list, so its memory does not
grow with the store. It builds every key it compares through `physical.rs`'s
constructors and classifiers and spells no layout byte of its own.

A required leaf that is absent is found when its node closes. An index row is
checked against its source entry by point reads (the marker, then each
projected field leaf), and a present entry with a complete projection is
checked for its row; the engine work is therefore one scan per page of
populated cells plus a bounded number of point reads per index row and per
indexed entry, never a read per declared field (`tests/audit_walk_work.rs`
holds this flat across declared widths, and holds the largest batch the walk
ever receives at the engine's page bound over 10,000 entries).

`marrow-lifecycle`'s `audit.rs` composes the audit: it opens the store under
the owner lock through the exact-binding gate the importer uses, runs the
engine's own integrity audit (redb's whole-file checksum walk, the same pass an
unclean open runs), then the kernel walk, and renders each finding in source
vocabulary. The digest is a hash chain over the walk's cell stream — starting
from the store data digest of the empty payload, each cell in key order folds
its key length, key, and value into the next state — so it is computed in the
same pass with the same bounded memory. The head's data-digest slot stays
reserved; the digest is reported, not persisted.
