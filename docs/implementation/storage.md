# Storage implementation

`marrow-store` implements the current logical tree-cell and transaction
contract beneath the language. It knows catalog-shaped identifiers and typed
key/value encodings used by the current stack, but it does not parse `.mw`
source.

## Layers

| Layer | Owner |
|---|---|
| Public typed facade | `tree.rs`, `sealed.rs` |
| Key and value codecs | `key.rs`, `value.rs`, `decimal.rs`, `codec.rs` |
| Logical cell families | `cell.rs`, `catalog.rs`, `metadata.rs` |
| Transactions and traversal | `tree.rs`, `traversal.rs` |
| Backup cell framing | `backup.rs` |
| Private backend contract | `backend.rs` |
| In-memory backend | `mem.rs` |
| Native redb backend | `redb.rs` |
| Shared backend laws | `conformance.rs` |

`TreeStore` is the current typed facade. Native handles are opened through
`SealedStore` with an `AccessMode`; raw backend and physical key APIs are
private to the crate.

## Transactions and durability

The backend contract provides snapshot reads and atomic write transactions.
The facade keeps data, derived index cells, catalog rows, and commit metadata in
one commit bracket. Memory and redb run the same private substrate conformance
laws; only redb is the current persistent substrate.

The native store seals each committed state with commit metadata and a commit
record containing per-root structural digests. Current metadata family tags are
`04` for engine metadata, `05` for commit metadata, and `07` for the sealed
commit record. There is no separate `06 + store ID` structural-digest cell.

## Recovery and backup

Normal open performs bounded structural checks. Explicit integrity and recovery
paths perform deeper validation. Physical recovery is below application
semantics and cannot by itself establish that saved data matches the current
program.

Backup streams accepted catalog rows and logical data cells while excluding
derived indexes; restore rebuilds indexes. One persistent substrate means
cross-backend portability is not yet an earned conformance claim.
