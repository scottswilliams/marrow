# Data Inspection And Repair Tools

## `marrow data diff` and `marrow data load`

`marrow data diff` compares two store states and reports typed differences.
The baseline form compares states at an equal catalog epoch; comparison across
epochs is the growth direction. `marrow data load` applies typed records to a
store through the maintenance capability. Neither command changes the
read-only contract of the current `marrow data` inspection subcommands or the
narrow repair-open role of `marrow data recover`; typed backup/restore keeps
ownership of bulk data movement.

See [`../data-tools.md`](../data-tools.md) for the `marrow data` commands
available today.
