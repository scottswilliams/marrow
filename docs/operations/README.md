# Operations

A durable program keeps its data in a store: a directory on disk bound to one
program. This page covers creating a store, running against it, changing the
program, and what an interrupted commit leaves behind.

Today, a store runs on one machine under one process at a time. Served
execution, backup, restore, and schema evolution are future work
([status](../status.md#not-yet-available)).

## A store on disk

`marrow import` creates a store and fills it from a file of JSON objects, one
entry per line. Each member is a scalar named for a key or a field of the root.
The program is the notes program from the [quickstart](../quickstart.md): `store
^notes[id: int]: Note`, whose `text` field is required.

```sh
printf '{"id": 1, "text": "imported note"}\n{"id": 2, "text": "second"}\n' > seed.jsonl
marrow import --store ./store --jsonl seed.jsonl --root notes --keys id
```

```text
provisioned a fresh store at ./store
{"batches_committed":1,"rows_imported":2}
```

The first line reports that `./store` was created. The second counts what was
committed. The file is read and committed in bounded batches, so a corpus larger
than memory imports the same way.

`import` is the only command that creates a store. It compiles and verifies the
project first and binds the new store to that program. It mints no identity: the
ledger `.marrow/ids` comes from one storeless `marrow run` before the import
([identity ledger](../tools/projects.md#identity-ledger)).

Both `import` and `run --store` run the program in a separate runner process,
`marrow-runner`, installed beside `marrow` together with the `marrow-companions`
manifest. The `marrow` process itself opens no store. A missing or altered
runner is `cli.installation_damaged`, and nothing runs.
[Install](../install.md#running-against-a-store) states how to have that layout
and which platforms open a store.

## Running an export against a store

`marrow run <export> --store <dir>` runs one exported function against the store
and prints its result:

```sh
marrow run textOf --store ./store -- 1      # imported note
marrow run add --store ./store -- 3 "added via run"   # true
marrow run textOf --store ./store -- 3      # added via run
```

Each invocation is its own commit boundary. `add` commits its `transaction`
block, and the next `textOf` reads what it wrote. A read-only export runs the
same way, since the values it reads live in the store.

A durable export run without `--store` has nothing to act on:

```sh
marrow run add -- 1 x
```

```text
cli.durable_unsupported
```

The one exception is the first such run on a project with no ledger, which
writes `.marrow/ids` before reporting this. `marrow run --store` never mints.

## Changing the program

A store is bound to the program that provisioned it. Every `run --store`
compiles the project and compares the result with that binding:

- An identical program opens the store with no write.
- A program whose code changed, and whose resources, store roots, indexes, and
  exported functions are unchanged, rebinds the store to the new code. Every
  stored value stays in place, and the next run uses the new code.
- A program whose durable contract or exported interface changed is
  `store.contract_changed`. The store is untouched, and the prior program still
  runs against it.
- A program that touches more durable places than the store accepted at
  provisioning is `store.demand_exceeds_ceiling`. The refusal names the export,
  the place, and the access. The store is untouched.

The durable contract is the set of resources, store roots, keys, fields, and
indexes the program declares. No transition rewrites stored data. Accepting a
changed contract, with stored data carried across, is future work ([data
coexistence](../future/data-coexistence.md)).

## Interrupted commits

Whether an invocation returned and whether its commit happened are two separate
facts. An invocation that faults before its block commits rolls back and reports
the fault; the store is as it was. One whose commit is confirmed and then faults
later reports `incomplete` with durable state `known_new`: the commit stands.
One whose commit is aborted reports `incomplete` with `known_old`: nothing
changed.

When the store cannot say whether a commit completed, the runner reopens the
store file and audits it. The proposed state on disk is `known_new`, the prior
state is `known_old`, and anything else is `unknown`. No application code runs
again, and no commit is retried. The runner then holds the store's lock until it
exits, and the next command starts a fresh runner.

If no reply from the runner reaches `marrow`, the outcome is
`run.outcome_unknown`: the call may have run, wholly or in part. In every
uncertain case, run a read-only export to observe the store before acting again.

## Durability

A confirmed commit is written with `fsync` before it is reported. It survives a
process exit and an operating-system crash. Survival of sudden power loss or a
drive-cache reset is not established: the commit path issues `fsync`, not
`F_FULLFSYNC`, so a drive's write cache may still hold the last write.

## Locks

One process owns a store at a time. The runner takes the store's lock when it
attaches and releases it when it exits. A second process opening a held store is
`store.locked`. The lock excludes other Marrow processes; it does not detect a
store file replaced or rolled back underneath it by another program.
[Status](../status.md#trust-boundaries) lists the trust boundaries.
