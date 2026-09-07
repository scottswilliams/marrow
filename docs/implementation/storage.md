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

`marrow-kernel`'s `durable/audit.rs` owns the logical walk used by `marrow
doctor`. The walk first point-reads the empty key because `scan_after` excludes
its cursor. It then scans the remaining key space forward in bounded pages.
Together these cover every cell, including an empty key outside the schema and
data beneath absent parents. Each scanned cell is classified against fixed
tables derived from the admitted projection, using `physical.rs`'s constructors
and classifiers. Keys also pass the canonical scalar-domain validator, including
the supported ranges of dates and instants.

The cell stream follows physical order: each root's entry family, with markers,
own fields, group leaves, and branch descendants, followed by index families
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

The walk retains per-schema tables, a stack bounded by durable depth with
per-frame state proportional to declared node width, one scan page, and capped
findings. The engine cache is a separate memory population; the page witness
does not establish a native process residency bound. Large-store cache
residency and full peak-allocation qualification remain open.

`marrow-lifecycle`'s `audit.rs` uses the existing exact-binding admission gate
and selects `NativeOpenAccess::ReadOnly` through the native opening path. It
runs no repairing integrity check, changes no engine, head, or envelope bytes,
and does not discharge an inherited unclean-shutdown obligation. The lifecycle
maps findings to source names using the projection's schema and index
identities. The owner lock is released before the runner renders the report.

The digest is a hash chain over declared entry-family cells in key order. It
starts from the store-data digest of an empty payload; each step hashes the
previous state followed by the cell's key length, key, and value. Index and
metadata cells are excluded. Identical entry content has the same digest, and
a same-value commit need not change it. The head's data-digest slot remains
reserved: this digest is reported, not persisted, and grants no admission or
recovery permit.

Physical checksum verification is not part of logical inspection. A scalar
change that remains valid under its declared type can pass even when its
physical checksum is wrong. Full physical and image/schema/store validation
with fresh read-only admission before recovery resumes service remains
unimplemented. The existing service recovery path and its limitations are
unchanged ([operations](../operations/README.md#auditing-a-store)).
