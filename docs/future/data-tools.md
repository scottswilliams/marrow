# Data Inspection And Repair Tools

## `marrow data diff` and `marrow data load`

`marrow data diff` compares two store states and reports typed differences.
The baseline form compares states at an equal catalog epoch; comparison across
epochs is the growth direction. `marrow data load` applies typed records to a
store through the maintenance capability. Neither command loosens the read-only
guarantee of the `marrow data` inspection group, and typed backup/restore keeps
ownership of bulk data movement.

See [`../data-tools.md`](../data-tools.md) for the read-only `marrow data`
commands available today.
