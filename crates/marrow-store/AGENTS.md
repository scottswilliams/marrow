# marrow-store — Agent Notes

The storage layer: backend trait, redb and memory backends, tree-cell keys and
values, backup streaming, conformance. The byte contract below the language.
Map: [docs/implementation/store.md](../../docs/implementation/store.md).
Contract: [docs/backend-contract.md](../../docs/backend-contract.md).

**You MUST keep this map current.** On any high-level change here — a module,
type, codec, or invariant added, removed, renamed, or reshaped — review
store.md and update it IN PLACE in the same change, as concisely as possible:
rewrite the affected lines and delete what went stale. It is imperative the map
never accrues agentic sediment — no appended notes, history, or duplicate lines;
it is a thin map, not a changelog. Trivial edits that change nothing at the
map's altitude need no update.
