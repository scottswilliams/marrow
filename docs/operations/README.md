# Operations

Current operations target one local native store owned through ordinary Marrow
commands. Marrow does not install a daemon, service manager, replication layer,
or high-availability control plane.

- [Native Store Operations](native-store.md) covers storage selection, process
  ownership, deployment, integrity checks, backups, and security assumptions.
- [Recovery](recovery.md) covers `doctor`, unclean-open recovery, corruption,
  lock repair, and restore decisions.
- [Backup And Restore](../tools/backup-and-restore.md) is the command reference
  for typed archives.

The native redb implementation is the only current persistent substrate. Raw
store files are private implementation data. Operator inspection uses the typed
[Data Tools](../tools/data.md), and durable data movement uses backup and
restore.
