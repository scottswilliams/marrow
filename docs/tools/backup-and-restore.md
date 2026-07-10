# Backup And Restore

Backup archives are the current typed data-movement boundary. They contain a
manifest, accepted durable-identity rows, and canonical data cells. They are not
copies of the private native-store file.

## Create A Backup

```text
marrow backup <projectdir> <output-file>
```

Backup checks the project, opens the live store read-only, verifies structural
digests and declared-index completeness, and reads one stable snapshot. It does
not replace the full modeled-data checks in `marrow data integrity`. A project
with no store file produces a valid empty archive. The command writes an
adjacent owner-only temporary file, syncs and reopens it for validation, then
renames it to the requested output path.

The final rename replaces an existing destination on supported Unix platforms.
The standalone command does not reject project-managed destinations. Never use
`marrow.json`, `marrow.lock`, a source or test path, the legacy client output,
`store.dataDir`, or the native store file as the backup path. Run `marrow data
integrity` first when the archive is intended as a recovery point.

Success reports the saved-entity count:

```console
$ marrow backup ./shelf ./shelf.mwbackup
ok: backed up 2 record(s) to ./shelf.mwbackup
```

One user-facing record is one saved entity such as `^books(1)`. The archive
manifest also carries an internal physical cell-frame count; that value is not
the record count printed by backup or required by counted restore.

## Restore Into An Empty Target

```text
marrow restore <projectdir> <backup-file>
```

The project must select a native backend. Restore validates archive framing,
checksum, source and accepted identity, engine profile, layout, key profile,
value codec, and typed data before commit. The target must contain no data
cells, generated index cells, or accepted catalog. Catalog rows and data cells
are replayed in one transaction, generated indexes are rebuilt, and a fresh
store UID and `marrow.lock` projection are written.

```console
$ marrow restore ./shelf ./shelf.mwbackup
ok: restored 2 record(s) from ./shelf.mwbackup
```

The success line contains only the restored entity count and archive path.

## Replace A Nonempty Target

```text
marrow restore --replace --count N <projectdir> <backup-file>
```

Replace mode first validates the live target against its accepted shape and
counts its saved entities. This check does not report dangling identity references; run `marrow
data integrity` for the full modeled-data pass. The exact count must equal `N`;
otherwise restore refuses before clearing data. Clearing and replay occur in the
same transaction, so a replay or verification failure rolls back to the prior
target state.

```console
$ marrow restore --replace --count 2 ./shelf ./shelf.mwbackup
ok: restored 2 record(s) from ./shelf.mwbackup
```

`--replace` requires `--count`, and `--count` is invalid without `--replace`.

## Inspect Before Restoring

The read-only data commands and evolution preview can mount an archive in
memory:

```sh
marrow data stats --backup ./shelf.mwbackup ./shelf
marrow data integrity --backup ./shelf.mwbackup ./shelf
marrow evolve preview --from-backup ./shelf.mwbackup ./shelf
```

The data commands validate the archive without opening the live store.
Evolution preview also reads `marrow.lock` and may open the live store read-only
to describe a catalog mismatch; its data comparison still uses the archive.

## Compatibility Boundary

The native redb store is the only current persistent storage substrate. The
archive is independent of raw redb file bytes, but current restore still
requires the recorded engine, layout, key, and value-codec profile to match the
running implementation. The repository therefore does not yet claim tested
portability between multiple persistent backends. See
[Compatibility](../compatibility.md).
