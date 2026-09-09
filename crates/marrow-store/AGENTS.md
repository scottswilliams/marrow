# marrow-store Contributor Notes

This crate owns the private ordered-byte engine contract (`ByteEngine`) and its
in-memory and redb implementations under one conformance suite. It orders opaque
bytes and knows no `.mw` syntax or logical value meaning. The
[storage map](../../docs/implementation/storage.md) describes its current callers
and the kernel-owned key/value representation.

The store owns bytes, durability, snapshots, transactions, and physical
traversal. It does not own Marrow semantic paths, logical key or value
encoding, public URI identity, authorization, or evolution meaning. Redb is an
implementation, not product identity.

`StoreError` renders from a typed code. Store-wide internal traversal pages and
guards cursor progress rather than materializing unbounded data. The engine
must not become a separate source-language access model. Changes to its contract,
ordering, snapshots, transactions or failure semantics require both backend
conformance and the affected kernel/lifecycle evidence.
