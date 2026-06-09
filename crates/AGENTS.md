# crates/ — Agent Notes

Seven crates form the Marrow pipeline, source text to committed saved data.
Start at the map and crate table:
[docs/implementation/](../docs/implementation/README.md).

**You MUST keep the map current.** On any high-level change to a crate — a
module, type, pass, or invariant added, removed, renamed, or reshaped — review
that crate's page under `docs/implementation/` and update it IN PLACE in the
same change, as concisely as possible: rewrite the affected lines and delete
what went stale. It is imperative the map never accrues agentic sediment — no
appended notes, history, or duplicate lines; it is a thin map, not a changelog.
Trivial edits that change nothing at the map's altitude need no update.
