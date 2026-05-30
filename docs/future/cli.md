# CLI Reference

Future counterpart of [`../cli.md`](../cli.md).

## Non-empty `marrow restore` (replace, merge, repair)

`marrow restore` writes into an empty target only; a non-empty target fails with
`restore.not_empty`. Restoring over existing data — the replace, merge, and
repair modes — is an explicit maintenance action routed through the maintenance
capability, not a relaxation of the empty-target guard. Until they ship, empty
the target first with a maintenance run, then restore into the empty store.

See [`../cli.md`](../cli.md) for the restore command as it works today.
