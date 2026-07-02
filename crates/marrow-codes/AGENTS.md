# marrow-codes — Agent Notes

The diagnostic code registry: one `Code` per dotted error code, the single owner
of every code string, its family/kind, severity class, catchability, and
documented meaning. Generates `docs/error-codes.md`.
Map: [docs/implementation/codes.md](../../docs/implementation/codes.md).

Add or change a code in the `codes!` table in `src/lib.rs` only, then regenerate
the reference page with `MARROW_UPDATE_ERROR_CODES=1` and review the diff. Never
spell a code string as an inline literal elsewhere; an absence test enforces it.

**You MUST keep the map current.** On any high-level change here — a field, type,
or invariant added, removed, renamed, or reshaped — review the matching page under
`docs/implementation/` and update it IN PLACE in the same change. It is a thin
map, not a changelog; no appended notes or history.
