# Data Inspection And Repair Tools

## Multi-Reader Live Inspection

A future app-server or local tooling host may own the write-capable store handle
and issue multiple read-only inspection handles inside that host. Each reader
pins a stable snapshot and exposes only typed, bounded or pageable facts from
checked source and catalog metadata. Readers do not expose raw saved paths,
physical keys, or an engine data-access surface, and writes still enter the single
writer queue.

This is not the v0.1 native file-open contract. v0.1 ships no `Sync`
`TreeStore`: separate read-only opens may coexist with each other, but any
read-only/read-write overlap across processes remains `store.locked`.

## `marrow data diff` and `marrow data load`

`marrow data diff` is a state-vs-state tool. The equal-epoch baseline
compares states at an equal catalog epoch, and cross-epoch comparison is the
growth direction. `marrow data load` applies typed records to a store through
the maintenance capability. Neither command changes the
read-only contract of the current `marrow data` inspection subcommands or the
narrow repair-open role of `marrow data recover`; typed backup/restore keeps
ownership of bulk data movement.

See [`../data-tools.md`](../data-tools.md) for the `marrow data` commands
available today.
