# Tools

Marrow tools operate on a project directory containing `marrow.json`. They
consume parser, checker, runtime, catalog, and store facts; they do not define a
second language or saved-data model.

- [Project File](project-file.md) defines `marrow.json` fields and validation.
- [CLI Reference](cli.md) covers project creation, formatting, execution, and
  tests, and indexes the remaining commands.
- [Data Tools](data.md) covers typed inspection and the `data` command family.
- [Evolution Tools](evolution.md) covers preview and apply over accepted durable
  identity.
- [Backup And Restore](backup-and-restore.md) covers the typed archive boundary.
- [Diagnostics](diagnostics.md) covers `check`, `doctor`, report formats, and
  dotted codes.

The command-line tools use checked source and typed store APIs. Raw native-store
keys and files are not command output contracts. Commands that intentionally
scan a whole store say so; application code does not acquire a general data API
through the administrative commands.

The prototype surface/client/serve commands were deleted at B00; see
[Project status](../status.md).
