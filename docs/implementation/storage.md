# Storage implementation

`marrow-store` is the stripped ordered-byte storage engine retained at lane
B00. It defines a crate-private byte-oriented engine contract and the two
implementors that back it. It orders opaque bytes: it does not parse `.mw`
source, resolve schemas, assign language identity, or interpret key or value
bytes. The logical key/value/civil-date codecs that give those bytes meaning
were relocated to the path kernel (`marrow-kernel`) at lane K.5. The engine
currently has no source-language consumer: the read kernel and runtime that
will drive it are refounded in later lanes.

## Layers

| Layer | Owner |
|---|---|
| Private byte-engine contract and `StoreError` | `backend.rs` |
| In-memory backend | `mem.rs` |
| Native redb backend | `redb.rs` |
| Shared backend conformance laws | `conformance.rs` (test-only) |
| Bounded scan accumulation | `traversal.rs` |

The crate's only public API is the `StoreError` re-export. The engine trait and
both backends are crate-private; the in-crate conformance suite keeps the memory
and redb implementations aligned on the same byte-level laws.

## What was deleted at B00

The prototype's logical tree facade (`TreeStore`/`SealedStore`), admission
metadata, catalog rows, structural digests, backup framing, and the `decimal`
value type were deleted with their owners; each returns through its refounding
lane. Inside the redb adapter, the page-level recovery probe and the
process-global panic-hook swap were deleted as release-veto families: a
malformed or torn store now surfaces redb's own open error through the typed
`StoreError` mapping, with no engine-page parsing above the backend.
