# marrow-store Contributor Notes

On the beta line this is the raw ordered-byte engine: the private byte-oriented
engine contract (`ByteEngine`) with in-memory and redb backends under one
conformance suite. It orders opaque bytes and knows no `.mw` syntax and no
logical value meaning. The tree-cell key layer and the logical
key/value codecs it briefly hosted were relocated to the path kernel (calendar and temporal text semantics live in `marrow-temporal`)
(`marrow-kernel`) at T01; the prototype's logical tree facade, admission,
catalog, backup, `decimal`, the page-level recovery probe, and the
process-global panic-hook swap were deleted at B00. The kernel is the engine's
only consumer.

The store owns bytes, durability, snapshots, transactions, and physical
traversal. It does not own Marrow semantic paths, logical key or value
encoding, public URI identity, authorization, or evolution meaning. Redb is an
implementation, not product identity.

`StoreError` renders from a typed code. Store-wide internal traversal pages and
guards cursor progress rather than materializing unbounded data. The engine
must not become a separate source-language access model. Its public contract is
narrowed at the byte-engine lane (E00) and consumed by the read kernel (E01).
