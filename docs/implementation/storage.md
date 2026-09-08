# Storage implementation

`marrow-store` orders opaque bytes. It defines a byte-engine contract, private
to the workspace because `marrow-kernel` is its only dependent, and the two
implementations behind it. Meaning comes from the kernel above it:
`marrow-kernel` owns the codecs that turn a durable path into key
bytes and a value into cell bytes. Program invocations reach the engine through
kernel sessions; privileged logical inspection uses the kernel's audit walk
under the native owner.

## From a path to a cell

A durable read such as `^books[id].title` travels four layers. The compiler
resolves the path to a schema-stable operation in the program image. The
kernel's session turns that operation and the runtime key into an ordered byte
key (`durable/physical.rs`) and asks the engine for the cell or range under it.
The engine (`marrow-store`) returns bytes in key order. The kernel decodes them
back to a value (`codec/`) and hands it to the VM. Writes follow the same path
in reverse, staged inside one engine transaction that commits with the
`transaction` block.

`number_store` assigns globally unique store-local numbers to roots, branches,
groups, and fields. Each root or branch declaration uses its number as a static
entry-family prefix. An entry marker appends the encoded full ancestor-and-own
key tuple and a terminator; its field and group leaves extend that marker.
The selected family's schema fixes every key component's kind and position.
Child families occupy separate ranges, so a parent's replacement or deletion
leaves its children in place. The complete path includes every component of
each declared key tuple; the existing 4096-byte key-write limit still applies.

`codec::value::scalar_key_matches_type` owns scalar key kind and domain checks.
The address helper applies it to each supplied root and branch key column in its
existing column walk, then encodes the validated full tuple once. Traversal
validates supplied ancestors before encoding its range and carries the resolved
immediate key kind beside that range. It validates a decoded key before
returning it. Index operations validate supplied and decoded components;
unique lookup also checks every decoded source column against the root's key
schema. These checks allocate no collection and add no engine access. Invalid supplied keys
refuse before operation reads or writes, after the session's separate setup.

## Layers

| Layer | Owner |
|---|---|
| Byte-engine contract (`ByteEngine`, `ReadView`, `WriteTxn`, `CommitOutcome`, `Cell`) | `engine.rs` |
| Errors (`StoreError`) | `error.rs` |
| In-memory engine (`MemoryEngine`) | `mem.rs` |
| Native redb engine (panic-contained adapter, read-only access, service integrity audit) | `redb.rs` |
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
lock. `NativeOpenAccess` selects service read/write access or read-only
inspection through that same opening path. Inspection cannot write or invoke
the repairing integrity operation; it preserves any inherited unclean-shutdown
obligation. An indeterminate commit quarantines the lock until process exit; the
kernel classifies the outcome as known old, known new, or unknown
([interrupted commits](../operations/README.md#interrupted-commits)). The lock
excludes cooperating Marrow processes and does not authenticate the engine
file; recovery cannot detect an out-of-band substitution of `store.redb`. That
gap is recorded in [project status](../status.md#trust-boundaries); the
[audit](#auditing-a-store) reports what a substituted file's contents disagree
with, not where the file came from.

The lifecycle's existing logical-head generation selects the entry layout.
Generation 2 is written by fresh provisioning; generation 1 and every other
unsupported generation are refused with `store.format_version`. Attach,
code-only rebind, logical audit, and import read that fence before opening the
engine. Refusal preserves the engine file, head, and envelope; owner-marker and
lock bookkeeping can precede it. There is no automatic rewrite or migration
reader. Generation 2 changes branch-entry keys; root, index, and metadata
encodings retain their generation-1 shapes. Image, envelope, and engine formats
are unchanged, and the head's sequencing and data-digest fields retain their
reserved-zero meanings
([compatibility](../compatibility.md#versioning)).

## Reading a field

`durable/store/address.rs` prepares each field site with one private immutable
`FieldPayload` behind `Arc`. Cloning field tokens from read or transaction
sessions shares the containing record and selected value shape. Session setup
still resolves and copies metadata; path/key cloning and stored-value decoding
remain separate work. `read_ops.rs` reads a field with one point read and no
scan or write after setup. A preceding entry-presence guard performs its own
marker read. Group reads retain the materialization cost below.

## Reading a whole entry

Reading a whole entry or group (`marrow-kernel`'s `read_record_leaves`, the
single owner shared by the root entry and every group) does engine work
proportional to the entry's populated field count, never its declared width.
The read is a structural-tag-bounded range scan over the node's own contiguous
field-leaf cells (`physical::field_leaf_range`, the marker stem followed by the
field tag), so it visits only present leaves and stops at the next node
boundary. The counted unit is engine scan calls: one per returned page plus
the terminal scan, with page limits described below. A per-declared-field probe
would make this `O(declared)`; a counting-engine test fails if that returns.

The value the read produces is `O(declared)`: `EntryValue.fields` is a dense
schema-aligned `Vec<Option<_>>` with one slot per declared field, and the
per-read name map that places each scanned leaf costs the same. A sparse sorted
`(field-index, value)` representation, which the leaf scan already yields in
order, would make both `O(populated)`; the dense shape stays because the
create, read, replace, and index-maintenance contract is written against it.

## Maintaining indexes

`durable/plan.rs` owns the projection from an entry mutation to ordered index
cell operations. Its old and new states each distinguish an absent entry from
a present entry's projected fields. Entry absence produces no projection;
present empty or all-sparse payloads still produce key-only projections.
An unchanged projection emits no operation. Changed projections remove the
old cell before putting the new, in declaration order.

The transaction session supplies presence already established by the entry
operation. Creation needs no old projected-field reads; erasure retains its
old projected reads, including refusal of malformed projected orphan leaves,
and allocates no synthetic absent-field vector. Erasing an absent entry
cleans up its decodable own payload without removing a sibling's unique cell.
The session applies every source and index operation in the same transaction;
a unique put checks existing ownership and faults on a different source
identity. The VM rolls back that transaction through its ordinary runtime
fault path. Maintenance preserves initially coherent indexes; it does not
repair pre-existing inconsistencies.

## Navigating entries

`durable/store/traverse.rs` resolves one static root or branch family and the
supplied ancestor keys into a range. Each `layer_step` makes one `scan_after`
call and classifies the first returned cell. A marker key yields the next
immediate key; own payload where a marker is required faults as corruption;
the end of the range yields no key. The next step starts after that entry's
own payload. Other families are outside the range, including children of
absent parents. The operation checks marker-key structure and scalar domain;
the complete audit owns marker-value and payload validation.

Bounded acquisition uses at most `N + 1` steps to freeze `N` keys and decide
`more`. The extra step has the same orphan check and stops when its marker key
establishes `more`. Family presence uses one step. These bounds count operation
scan calls after setup, independently of child populations and declared field
width. They do not count loop-body operations, setup, or commit work.

The byte-engine collector returns at most 64 cells per page with a soft 1 MiB
sum of key and value bytes. An oversized first cell is returned to make progress.
A navigation step can therefore copy payload and later-entry cells that it
does not classify. Frozen keys, decoded values, page allocation and disposal,
native cache residency, and backend seek time remain separate costs. The scan
bound is neither a byte bound on hostile input nor a latency guarantee.

## Auditing a store

`marrow-kernel`'s `durable/audit.rs` owns the logical walk used by `marrow
doctor`. The walk first point-reads the empty key because `scan_after` excludes
its cursor. It then scans the remaining key space forward in bounded pages.
Together these cover every cell, including an empty key outside the schema and
data beneath absent parents. Each scanned cell is classified against fixed
tables derived from the admitted projection, using `physical.rs`'s constructors
and classifiers. One family table follows store-number order. When opening an
entry, the walk decodes all ancestor and own key components against borrowed
schema slices, including the supported ranges of dates and instants. Later
payload cells share that validated marker stem and decoded tuple. No ancestor
marker is required or point-read.

The cell stream follows physical order: each root or branch entry family, with
markers, own fields, and group leaves, followed by index families
and metadata. The walk reports malformed markers, undecodable values, cells
outside the schema, and orphaned leaves. A missing required field is reported
when its node closes. A marker with no populated leaves is valid when all the
entry's fields are sparse. Findings therefore follow deterministic scan and
node-closure order, rather than sorted place order. Every finding is counted;
only the first 256 in that order are retained.

Index correspondence is checked in both directions. Each index cell's source
marker and projected fields are point-read and compared with the projection.
Each present entry with a complete projection must have an index cell whose
source payload names that exact entry. The engine work is one initial empty-key
read, one scan per page plus the terminal empty scan, and a bounded number of
point reads per index cell and indexed entry. It performs no point read per
declared field. `tests/audit_walk_work.rs` compares engine-call counts across
declared widths and populations and checks the largest returned page over
10,000 entries. The page witness measures returned batch size, not peak
allocation or how many pages remain retained.

The walk retains per-schema tables and one current entry. For `F` families and
at most `H = 17` root-plus-branch segments per family, the key-kind paths hold
`O(F × H)` borrowed slice descriptors; scalar kinds stay in the projection.
The current entry holds flags proportional to its declared own/group width,
its encoded and decoded keys, and cached top-level scalar values for indexed
roots, including fields unused by an index. Add
one scan page and at most 256 finding sites. No state grows with the number of
stored entries. These are logical retention bounds, not peak-allocation
measurements. Closing an entry checks its declared field/group width, and group
suffix lookup remains linear in the number of declared groups. The engine
cache is a separate memory population; the page witness does not establish a
native process residency bound. Large-store cache
residency and full peak-allocation qualification remain open.

`marrow-lifecycle`'s `audit.rs` uses the existing exact-binding admission gate
and selects `NativeOpenAccess::ReadOnly` through the native opening path. It
runs no repairing integrity check, changes no engine, head, or envelope bytes,
and does not discharge an inherited unclean-shutdown obligation. The lifecycle
maps findings to source names using the projection's schema and index
identities. The owner lock is released before the runner renders the report.

The digest is a hash chain over declared static entry-family cells in key
order. It starts from the store-data digest of an empty payload; each step
hashes the previous state followed by the cell's key length, key, and value. Malformed
cells in a declared family and children beneath absent parents are included.
Index, metadata, and undeclared-family cells are excluded; undeclared cells
still produce findings. Identical entry-family content has the same digest, and
a same-value commit need not change it. The head's data-digest slot remains
reserved: this digest is reported, not persisted, and grants no admission or
recovery permit.

Physical checksum verification is not part of logical inspection. A scalar
change that remains valid under its declared type can pass even when its
physical checksum is wrong. Full physical and image/schema/store validation
with fresh read-only admission before recovery resumes service remains
unimplemented. The existing service recovery path and its limitations are
unchanged ([operations](../operations/README.md#auditing-a-store)).
