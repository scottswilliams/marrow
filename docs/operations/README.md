# Operations

A durable program keeps its data in a store: a directory on disk bound to one
program. This page covers creating a store, running against it, changing the
program, what an interrupted commit leaves behind, and auditing a store.

Today, a store runs on one machine under one process at a time. Served
execution, backup, restore, and schema evolution are future work
([status](../status.md#not-yet-available)).

## A store on disk

A store is created by `marrow import`, which compiles and verifies the project,
provisions the directory, and binds the new store to that program ([marrow
import](../tools/cli.md#marrow-import)). The corpus is read and committed in
bounded batches, so a corpus larger than memory imports the same way.

Within the `marrow` project CLI, `import` is the only command that creates a store.
It mints no identity: the ledger `.marrow/ids` comes from one storeless
`marrow run` before the import
([identity ledger](../tools/projects.md#identity-ledger)).

The generated TypeScript supervisor also provides
[`provision(options)`](../tools/typescript-client.md#launching), which creates an
empty store bound to a compiled image without importing a corpus.

Current tools provision stores with logical-head generation 2. The entry layout
requires fresh provisioning; there is no automatic conversion of older stores
([compatibility](../compatibility.md#versioning)).

Both `import` and `run --store` run the program in a separate runner process,
`marrow-runner`, installed beside `marrow` together with the `marrow-companions`
manifest. The `marrow` process itself opens no store. A missing or altered
runner is `cli.installation_damaged`, and nothing runs.
[Install](../install.md#running-against-a-store) names the layout and the
platforms that open a store; no current command assembles it.

## Running an export against a store

`marrow run <export> --store <dir>` runs one exported function against the store
and prints its result. The exports below are the notes program from the
[quickstart](../quickstart.md):

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
compiles the project and, once the store's format is admitted, compares the
result with that binding:

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

`marrow import` into an existing store never rebinds. It fills the store only
when the compiled program is exactly the active binding; a code-only change is
`store.image_not_active` until a `run --store` rebinds the store, and the
other refusals above apply unchanged. Every refusal is decided before the
store's engine opens, so a refused import writes nothing.

A generation-1 store requires its matching older toolchain. Current tools
refuse it with `store.format_version` before engine open, including for
code-only rebind, audit, and import. Generation-1 tools likewise refuse
generation-2 stores. These refusals preserve the engine file, head, and envelope; lock and
owner-marker bookkeeping may still occur. Using the matching toolchain leaves
the store usable. Rebind and import never migrate its layout.

The active binding must also name image generation 1. Attach, import and audit
refuse other image generations with `store.format_version` before engine open,
even when a newly compiled program has the same durable contract. This refusal
preserves the engine file, head and envelope; owner-marker bookkeeping may occur.
Keep the old source, identity ledger, image and tools with the old store for data
extraction. No command converts it in place
([compatibility](../compatibility.md#versioning)).

## Interrupted commits

Whether an invocation returned and whether its commit happened are two separate
facts. An invocation that faults before its block commits rolls back and reports
the fault; the store is as it was. One whose commit is confirmed and then faults
later reports `incomplete` with durable state `known_new`: the commit stands.
One whose commit is aborted reports `incomplete` with `known_old`: nothing
changed. `marrow run` prints the outcome and, for an interrupted invocation, the
durable state; with `--format jsonl` they are the `outcome` and `durable`
members of the run record.

When the store cannot say whether a commit completed, the runner reopens the
store file and audits it. The proposed state on disk is `known_new`, the prior
state is `known_old`, and anything else is `unknown`. No application code runs
again, and no commit is retried. The runner then holds the store's lock until it
exits, and the next command starts a fresh runner.

If no reply from the runner reaches `marrow`, the outcome is
`run.outcome_unknown`: the call may have run, wholly or in part. In every
uncertain case, run a read-only export to observe the store before acting again.

## Auditing a store

`marrow doctor --store <dir>` checks a store's logical contents against its
active program without running an export
([marrow doctor](../tools/cli.md#marrow-doctor)). The project at the working
directory must match that program. The audit admits the image and inspects the
store under its owner lock, then releases the lock before printing its findings
and a digest over the entries. It leaves the engine file, head, and envelope
unchanged:

```sh
marrow doctor --store ./store
```

A finding names a stable code and the place it concerns: an invalid key or
value, a cell outside the program's shape, missing required data, a malformed
presence marker, or a mismatch between an index and its source entry. Data
beneath absent parents is inspected too. An entry with only sparse fields may
have no populated fields. All findings are counted; at most 256 are listed in
deterministic scan and node-closure order.

The digest is computed from declared entry-family keys and values, including
malformed cells in those families and data under absent parents. Index,
metadata, and undeclared-family cells are excluded; undeclared cells are still
findings. Runs over unchanged entry content produce the same digest, and a
same-value commit need not change it. The digest is reported, not stored; a recorded digest can be compared with
a later inspection under the same program. It does not authenticate the engine
file or establish its origin. Substitution and rollback remain unqualified;
an index under an identity absent from the active program is a logical finding,
but a file from another store need not contain such a mismatch.

The report explicitly states that physical integrity was not checked. A changed
scalar can remain valid under its declared type and pass, including when the
stored checksum no longer matches. Exit `0` therefore means no logical
inconsistency was found; findings, engine errors, and refusals exit `1`.
Inspection does not repair the engine or clear the unclean-shutdown status
inherited from a prior owner. Complete physical and image/schema/store
validation, followed by fresh read-only admission before recovery resumes
service, remains future work. A logical audit report does not establish that
recovery is safe ([status](../status.md#trust-boundaries)).

## Durability

A confirmed commit is written with `fsync` before it is reported. It survives a
process exit and an operating-system crash. Survival of sudden power loss or a
drive-cache reset is not established: the commit path issues `fsync`, not
`F_FULLFSYNC`, so a drive's write cache may still hold the last write.

## Locks

One process owns a store at a time. The runner takes the store's lock when it
attaches and releases it when it exits. An audit holds it through admission and
inspection and releases it before printing. A second process opening a held
store is `store.locked`. The lock excludes other Marrow processes; it does not
detect a store file replaced or rolled back underneath it by another program.
[Status](../status.md#trust-boundaries) lists the trust boundaries.
