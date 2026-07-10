# Native Store Operations

The native backend is Marrow's current persistent store. A project selects it
in `marrow.json`:

```json
{
  "sourceRoots": ["src"],
  "store": { "backend": "native", "dataDir": ".marrow/data" }
}
```

The private store file is `marrow.redb` below `dataDir`. There is no command-line
storage override. The in-memory backend is used for tests and nondurable
development; it is not a production durability profile.

## Process Ownership

The native store permits multiple concurrent read-only opens. A write-capable
open excludes every other reader and writer, and an existing reader excludes a
writer. Conflicts report `store.locked`; stop the process holding the store and
retry.

Typical access modes are:

| Mode | Commands |
|---|---|
| Read-only | `check` when a store is readable, `doctor`, `run --dry-run`, `evolve preview`, `data roots|stats|dump|get|integrity`, `backup` |
| Write-capable | An ordinary native `run`, `evolve apply`, `data recover`, `restore` |

`check` uses a lenient read-only open and does not repair an unreadable store.
Inspection commands hold their read handle or pinned snapshot for the duration
of the command, so a full scan can delay a writer.

## Store Creation And Absence

Read-only tools do not create a missing store. They report empty data where an
empty result is meaningful. An absent store body is treated as disposable local
state, not automatically as corruption.

A write-capable run or evolution apply can create a native store. When a
committed `marrow.lock` exists, that artifact supplies accepted durable identity
for the empty store; the write path reports that it seeded the store. Restore
can instead create a target from a validated archive. A present store that has
lost roots recorded by its accepted identity is different: it fails closed as
corruption rather than being silently reseeded.

The live store is authoritative for accepted durable identity. After a
successful write-capable command changes that identity, the CLI projects it to
`marrow.lock`.
Commit the resulting lock with the source change; do not hand-edit it or use it
to overwrite a valid live store.

## Applying A Source Change

Use an explicit maintenance window for a change that affects durable shape:

1. Stop the current writer.
2. Create a typed backup from the currently accepted source and store.
3. Deploy the new source and binary together.
4. Run `marrow check <projectdir>`.
5. Run `marrow evolve preview <projectdir>` and review every obligation.
6. Run `marrow evolve apply <projectdir>` with any required maintenance,
   retirement approvals, and recovery-point decision.
7. Run `marrow data integrity <projectdir>`.
8. Commit the regenerated `marrow.lock`, then start the new writer.

`run` can apply changes that rewrite no saved records, but preview/apply is the
operator-visible path for reviewed store changes. The current evolve-apply flow
is local and single-owner; rolling mixed-version deployment and concurrent
multi-writer transitions are not implemented.

## Full-Store Work

These commands intentionally scan a complete typed snapshot:

- `data stats` counts roots, entities, and values;
- `data dump` visits every checked value;
- `data integrity` checks modeled data and physical traversal witnesses;
- `backup` visits the canonical data-cell stream.

Schedule them accordingly for a large store. `data get` is the point-bounded
inspection command. Marrow does not currently provide online vacuum or
compaction. To repack, back up the accepted store, restore into an empty target,
and run integrity checking before replacing the old target operationally.

## Branch And Copy Discipline

Treat development stores as disposable and branch-local. Do not merge or copy
native store files as source artifacts. Move realistic data through a typed
backup created under the source that owns it, restore into an empty branch-local
target, and discard that target with the branch.

The archive is independent of raw redb file bytes, but the current restore path
still requires matching engine, layout, key, and value-codec facts. Because
redb is the only persistent substrate, this is not yet a cross-backend
portability claim.

## Security And Durability Assumptions

On Unix, newly created native store files and backup archives use owner-only
`0600` modes. Parent-directory permissions, filesystem durability, backups,
encryption at rest, process identity, and host access remain operator
responsibilities.

Checksums and structural digests detect selected accidental corruption; they
are not authentication or tamper protection. Marrow currently provides no
database users-and-roles system, compiler-enforced path authorization,
replication, failover, or TLS boundary. See [Project Status](../status.md) for
the complete current trust boundary.
