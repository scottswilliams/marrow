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

The native backend exposes no raw engine constructor. Its lower opaque owner
derives `lock` and `store.redb` from one canonical store directory and keeps the
real advisory lock inseparable from the engine. Provisioning alone calls a
non-returning create-only operation that stamps the engine format. Ordinary
open and commit recovery use existing-only operations. A missing file remains absent; an empty, malformed,
unstamped, foreign, dangling, or unreadable file is refused rather than created
or adopted.

## Owner lock, in two phases

An existing-store open is split so that nothing above the storage layer has to
read a byte of the store directory to decide exclusion.

Acquisition canonicalizes the directory and takes an advisory lock on the
directory node itself, before it opens any name inside it. It then opens the
`lock` entry as that directory's own regular file — a link or another node kind
standing in for it is refused, and the opened node is compared against the entry
the directory names — and takes a second advisory lock on that entry. It makes no
engine call and is not told which store instance it is about to hold. It returns
an affine pending owner. Whether the entry is reachable under a second name is
admitted *after* the lock: a second link does not divide exclusion, so refusing
on it earlier would convert a contender's exclusion verdict into an I/O refusal.

Exclusion rests on the directory node because every name inside the directory can
be replaced. A writer there can unlink the `lock` entry and create another node
under that name, and can rename a fresh engine file over `store.redb`; each
replacement alone is refused by the other node's lock, but replacing both leaves
neither, which is also what a naive whole-directory restore over a live store
does. The directory node is the one node in the store that no replacement of its
own children changes, and canonicalization pinned it before the lock was asked
for.

What that establishes, and what it does not: while a holder is live, no second
owner of the same store directory node can be constructed, whatever a writer
inside that directory does to its children. It is not exclusion over a *path*. A
writer that replaces the store directory node itself — moving it aside and
publishing another directory under the same name — leaves two live owners of two
different directories that one path reaches in turn. Nothing here refuses that,
and no claim is made that it does: a process that can rewrite the store
directory's parent already holds the store's custody. Exclusion is cooperative
and advisory throughout; it binds processes that take it, not an actor with write
access to the directory or its parent.

Binding publishes the store instance the caller has since read, runs the
caller's zero-capability admission callback, and opens (and, when the prior
shutdown was unclean, fully audits) the existing engine under the same lock.
Dropping a pending owner instead releases the lock and preserves whatever unclean
obligation it inherited.

A contender that meets a live holder is told the store is locked, and the
holder's marker bytes only decide how precisely the holder is named. The marker
records a lock held before its store is known (`Pending`: magic, layout version,
state tag, pid, acquisition time) or one bound to it (`Bound`: the same fields
plus the 16-byte store instance); the bound layout this replaced, which carried
no state tag, is still read. Any other byte string names no owner and changes no
verdict, and the decoder reaches every byte through a checked lookup so that a
length it does not expect reaches a verdict rather than an abort.

The `lock` entry is not a completeness signal: provision does not write it, the
first open creates it, and from then on it persists — empty after a clean close,
carrying the crashed holder's descriptor after an unclean one, which is the
inherited obligation the next acquisition discharges only by a completed open
and clean close.

## Owner-held artifact admission

`marrow-lifecycle` reads the store directory's own `envelope` and `head` only
under that owner, from a directory descriptor retained across the whole
admission (`marrow-fs-journal`, the workspace's sole owner of descriptor-rooted
filesystem operations). Each child is opened from that descriptor without
following a link, must be a regular file reachable under exactly one name, and
is bounded before allocation by the exact ceiling the version in its own
five-byte prefix selects; identity, length, and the directory's mapping of the
name are rechecked before the bytes reach a decoder. A version this build does
not read is refused at the prefix, with no body read.

Two paths into the same directory are outside that protocol and are resolved by
path: `store.redb`, which the storage layer opens as part of holding the engine,
and the `lock` entry it owns. Because path resolution follows a link, the
completeness verdict decides each artifact's node kind through the retained
descriptor: a name that maps to a node which is not a regular file is refused as
itself, naming the entry. Without that, a store whose `store.redb` name mapped to
a link would open on engine bytes outside the directory the owner holds.

Two questions are settled before the lock, because acquiring it creates the
`lock` entry and writes a marker into it: whether this build can admit a store
directory on this platform at all, and — when the directory holds no lock entry —
whether the directory is a store rather than an ordinary directory. Neither reads
an artifact's bytes, and a store with a live holder always has the lock entry, so
neither can preempt the exclusion verdict a contender is owed.

Neither question resolves a failure to look into an observation. A directory this
process cannot examine is not a directory whose artifacts are missing: the
refusal reports the access denial and names the path, and `store.permission_denied`
is kept distinct from the absent, incomplete, and corrupt verdicts, which state
what the directory holds.

Because the descriptor-rooted operations are provided on macOS, and on Linux for
`x86_64` and `aarch64`, a store open on any other target refuses with a typed
unqualified-platform refusal naming the operating system and architecture, before
it creates anything in the store directory. The build itself is not narrowed.

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

The lower native owner quarantines its advisory lock before returning an
indeterminate commit verdict. The kernel's opaque native semantic owner resolves
the fact while retaining that same lower owner: it closes the indeterminate
engine, freshly opens the existing engine file at the retained path, performs a
full integrity audit, and privately consumes and compares the fact. Exact equality with
the proposed state is known new; equality with the captured before state is known
old. A third value, scope mismatch, malformed cell, failed read, failed open, or
failed audit is unknown. A known result returns a usable owner only in that same
dedicated process. Quarantine is irreversible: dropping a known owner, losing
the affine fact, or reaching unknown all retain the nonempty descriptor and
advisory lock until process exit. No public re-arm operation exists.
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
