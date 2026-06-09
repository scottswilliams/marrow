# marrow-run — Agent Notes

The runtime: evaluates the checked program, plans managed writes, reads and
iterates saved data, applies data evolution, and hosts the stdlib boundary.
Map: [docs/implementation/runtime/](../../docs/implementation/runtime/README.md).

**You MUST keep this map current.** On any high-level change here — a module,
type, pass, or invariant added, removed, renamed, or reshaped — review the
matching page under `docs/implementation/runtime/` and update it IN PLACE in the
same change, as concisely as possible: rewrite the affected lines and delete
what went stale. It is imperative the map never accrues agentic sediment — no
appended notes, history, or duplicate lines; it is a thin map, not a changelog.
Trivial edits that change nothing at the map's altitude need no update.
