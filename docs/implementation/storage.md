# Storage implementation

`marrow-store` is the stripped ordered-byte storage engine retained at lane
B00. It defines a crate-private byte-oriented engine contract and the two
implementors that back it. It orders opaque bytes: it does not parse `.mw`
source, resolve schemas, assign language identity, or interpret key or value
bytes. The logical key/value/civil-date codecs that give those bytes meaning
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

## What was deleted at B00

The prototype's logical tree facade (`TreeStore`/`SealedStore`), admission
metadata, catalog rows, structural digests, backup framing, and the `decimal`
value type were deleted with their owners; each returns through its refounding
lane. Inside the redb adapter, the page-level recovery probe and the
process-global panic-hook swap were deleted as release-veto families: a
malformed or torn store now surfaces redb's own open error through the typed
`StoreError` mapping, with no engine-page parsing above the backend.
