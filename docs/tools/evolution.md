# Evolution Tools

Marrow compares checked source with the durable identity and data accepted by a
project's store. `evolve preview` reports the resulting obligations;
`evolve apply` recomputes and atomically applies an activatable witness against
the current store state.

The source-language `evolve` declarations and their exact semantics belong to
the [Language Evolution Reference](../language/evolution.md). This page covers
the command workflow.

## Syntax

```text
marrow evolve preview [--from-backup <artifact>] [--scaffold]
  [--format text|json|jsonl] <projectdir>
marrow evolve apply [--maintenance]
  [--approve-retire <field-path>:<count>]...
  [--backup <path> | --no-backup]
  [--format text|json|jsonl] <projectdir>
```

Preview is read-only. By default it checks obligations against the live store.
`--from-backup` validates and mounts archive data in memory. It still reads
`marrow.lock` and may open the live store read-only to report a catalog mismatch;
it does not use live data for the preview. `--scaffold` includes
formatter-produced source for supported evolution declarations in the report.

Apply checks the project, opens the native store write-capably, recomputes the
witness against that exact state, verifies its state binding inside the store
transaction, and publishes accepted identity, data changes, indexes, and commit
metadata together. After commit it regenerates `marrow.lock` from the store.

## Typical Workflow

1. Edit source and add any required `evolve` declaration.
2. Run `marrow check <projectdir>`.
3. Run `marrow evolve preview <projectdir>` and inspect every obligation and
   count.
4. When retirement or operational policy requires it, select a recovery backup.
5. Run `marrow evolve apply <projectdir>` with any approvals printed by preview.
6. Run `marrow data integrity <projectdir>` and commit the regenerated
   `marrow.lock` with the source change.

Preview exits successfully only when its witness is activatable. Apply refuses
if the source, accepted identity, store commit, engine profile, obligations, or
observed counts no longer match the recomputed witness.

## Current Change Classes

| Source change | Current handling over populated data |
|---|---|
| Add a sparse field or an unpopulated declaration | No record rewrite; `run` may auto-apply the change. |
| Add a required field | Provide a checked default or transform, then preview and apply. |
| Rename a resource, member, enum, or enum member | Use a source `evolve rename` so accepted identity follows the new spelling. |
| Add an index | Preview reports the rebuild; apply publishes rebuilt entries atomically. |
| Retype a populated leaf | Unsupported in place; add a new member, transform data, then retire the old member. |
| Change a populated key shape or keyed-layer structure | Unsupported in place; model a new location and move data with maintenance code. |
| Remove populated data or explicitly retire a declaration | Use `evolve retire` and explicit destructive approval. |

Empty locations do not require data migration. Changes that would orphan or
reinterpret populated data fail closed until source supplies a supported
transition or modeled maintenance has moved or removed the data.

## Destructive Retirement

A witness containing a destructive retirement requires all of the following,
even when preview reports an exact populated count of zero:

- `--maintenance`;
- one `--approve-retire <field-path>:<count>` for each target and exact count;
- either `--backup <path>` or the explicit decision `--no-backup`.

The command rejects configuration, source, test, and store-managed backup
paths. Its current guard does not include the legacy generated-client output,
which is refreshed after commit and can overwrite an archive at the same path.
Choose a recovery path outside the project tree. Apply writes and validates the
archive before mutating the store. A changed count or target invalidates the
approval and leaves the store unchanged.

`--backup` and `--no-backup` are apply-only and mutually exclusive.
`--from-backup` and `--scaffold` are preview-only. All reports support text,
JSON, and JSONL.
