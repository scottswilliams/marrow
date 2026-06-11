# Data Inspection And Repair Tools

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
