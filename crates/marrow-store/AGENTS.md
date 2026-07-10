# marrow-store Contributor Notes

This crate implements the current storage substrate boundary: ordered tree
operations, transactions, redb and memory backends, tree-cell encodings,
streaming backup, and backend conformance. It knows no `.mw` syntax.

The store owns bytes, durability, recovery, and physical traversal. It does not
own Marrow semantic paths, public URI identity, authorization, or evolution
meaning. Redb and the current tree-cell layout are implementations, not product
identity.

Use typed IDs and keys throughout. `StoreError` renders from a typed code.
`SealedStore` is the current handle-admission boundary; no caller may construct
a durable handle around validation. Store-wide internal and administrative
traversal pages and guards cursor progress rather than materializing unbounded
data. It must not become a separate source-language access model.

Physical substrate recovery is a private trusted component, not an application
data path. It may repair physical representation, but it does not establish that
the recovered bytes match the current program or its durable declarations.

Map: [docs/implementation/storage.md](../../docs/implementation/storage.md).
