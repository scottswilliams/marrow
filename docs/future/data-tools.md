# Data Tools

Future counterpart of [`../data-tools.md`](../data-tools.md).

Current data tools inspect typed Marrow data. They do not expose raw store keys,
engine bytes, or backend-specific dump formats as production APIs. Future data
movement tools keep that boundary.

## `marrow data diff`

`marrow data diff` is a deferred typed comparison command. It compares two
explicit Marrow data sources after each source is bound to checked source and
accepted catalog facts.

A diff reports modeled data differences: saved roots, record identities, keyed
entries, members, and values. It does not compare generated index cells, engine
metadata, physical key encodings, or file bytes. If the two inputs cannot be
decoded under compatible source and catalog facts, the command refuses instead
of falling back to a raw byte diff.

The command is a read-only inspection tool. It must not create a store, apply
evolution, regenerate `marrow.lock`, repair data, or write either input.

## `marrow data load`

`marrow data load` is a deferred typed import command. It loads modeled data
through checked Marrow shapes rather than through raw tree-cell keys.

A load input must name saved paths and values in a format whose types can be
checked before commit. The command must validate required fields, key shapes,
identity values, enum members, and orphaned managed cells before publishing a
target state. Generated indexes are derived from loaded data, not imported as
authority.

Loading is a write-capable maintenance operation. It has an explicit conflict
mode and commits atomically, or leaves the target unchanged.

## Restore merge and repair

v0.1 restore supports empty-target restore and counted replace. Restore merge
and repair modes are deferred.

A restore merge mode states how archive records match live records, how
conflicts are detected, which side wins when a value differs, and how required
fields and dangling references are validated. A restore repair mode names which
invalid states it may correct and which states remain operator-authored
maintenance work.

Both modes must preserve the backup contract: source/catalog validation,
typed-cell decoding, generated-index rebuild, and all-or-nothing commit.

## Cross-engine restore

Restoring an archive written under a different engine, layout, or value codec
currently reports `restore.engine_recompile_required`.

A future cross-engine restore is a typed recompile. It decodes the archive under
the source manifest and accepted catalog, validates the modeled data, then
encodes it under the target backend profile. It must not copy engine-private
bytes across profiles.

## Branch data movement

Native store files are not merge artifacts. Branch workflows move data through
typed backup, restore, and future data diff/load/merge tools.

A future cross-branch merge must compare modeled records by stable catalog and
record identity. It must report conflicts as typed data findings and leave the
native store format private.
