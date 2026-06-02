# Data Inspection And Repair Tools

## `marrow data diff` and `marrow data load`

`marrow data diff` and `marrow data load` move bulk data between a store and a
typed source. They route through the maintenance capability and depend on typed
source-fingerprinting, and they do not loosen the read-only guarantee of the
`marrow data` inspection group. Typed backup/restore owns bulk data movement.

See [`../data-tools.md`](../data-tools.md) for the read-only `marrow data`
commands available today.
