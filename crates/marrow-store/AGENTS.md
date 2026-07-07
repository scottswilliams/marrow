# marrow-store — Agent Notes

The byte contract below the language: the backend trait, redb and memory engines, tree-cell keys and
values, backup streaming, and a backend-agnostic conformance suite. Knows no `.mw`.

Identity is typed everywhere (`CatalogId`, `StoreUid`, `SavedKey`, `SequencePosition`) — no stringly
keys. `StoreError` is render-only with a typed `code()`. `SealedStore` is the sole mint of a durable
`TreeStore` handle, so a handle cannot be created around the integrity ladder — that is the crate's
enforcement artifact. Every store-wide walk pages and resumes with a fail-closed non-advancing-cursor
guard; never materialize unbounded. State a durable invariant in an assertion message, not the history
that added it. Precedent: redb / BurntSushi.

Contract: [docs/backend-contract.md](../../docs/backend-contract.md).
Map: [docs/implementation/store.md](../../docs/implementation/store.md).
