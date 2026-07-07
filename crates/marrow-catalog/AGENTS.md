# marrow-catalog — Agent Notes

The accepted-catalog snapshot model: the committed record of durable identity (epoch, digest,
entries), its validation invariants, and structural-signature decode. Both store and check read it.

Enforce invariants in the constructor: `CatalogMetadata`/`CatalogLock` verify the digest and
self-consistency before a value can exist and fail closed on every corruption class without panicking.
`StructuralSignature` is the sole reader of the wire grammar (the encode side lives once in
marrow-check). `StoreRootEntry` derives one key set by construction rather than three hand-kept
filters. Precedent: rust-analyzer's constructor-enforced invariant.

Map: [docs/implementation/check/](../../docs/implementation/check/README.md).
