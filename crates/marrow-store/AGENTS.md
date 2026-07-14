# marrow-store Contributor Notes

On the beta line this is the stripped ordered-byte engine: the private
byte-oriented engine contract with in-memory and redb backends under one
conformance suite, plus the tree-cell key layer and the logical
key/value/civil-date codecs it temporarily hosts. It knows no `.mw` syntax. The
prototype's logical tree facade, admission, catalog, backup, `decimal`, the
page-level recovery probe, and the process-global panic-hook swap were deleted
at B00; the engine has no source-language consumer until the runtime lanes wire
it up.

The store owns bytes, durability, and physical traversal. It does not own Marrow
semantic paths, public URI identity, authorization, or evolution meaning. Redb
and the tree-cell layout are implementations, not product identity.

Use typed IDs and keys throughout. `StoreError` renders from a typed code.
Store-wide internal traversal pages and guards cursor progress rather than
materializing unbounded data. The engine must not become a separate
source-language access model. Its public contract is narrowed at the byte-engine
lane (E00) and consumed by the read kernel (E01).
