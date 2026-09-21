# Operations

A durable program keeps its data in a store: a directory on disk bound to one
program. This page covers creating a store, running against it, changing the
program, interrupted commits, auditing a store, explicit recovery, and logical backup/restore.

Today, a store runs on one machine under one process at a time. Served
execution and schema evolution beyond explicit sparse-field apply are future work
([status](../status.md#not-yet-available)).

## Logical backup and fresh restore

`marrow backup` writes a complete logical backup under the source store's retained
owner. It compiles the current project, which must match the store's exact active
image; it does not rebind a code edit. Restore uses the backup's embedded image
and needs neither the original project nor current-source compilation.

```sh
marrow backup --store ./store --out ./complete.backup
marrow restore --from ./complete.backup --store ./restored
```

The artifact carries the exact executable image, accepted head and ceiling, and
all canonical entry and managed-index cells, including descendants beneath
absent ancestors. Commit witnesses, engine files and operational store identity
are not transferred. Restore creates a fresh store instance, retaining the
image, logical data and accepted facts. It executes no embedded export. It
does not merge stores, evolve a schema or replace an occupied destination.

Backup leaves source artifacts unchanged, including ownership-marker bytes or
absence. It performs logical audit and export in one coherent read view. A finding
prevents completion. It does not verify physical source checksums. The bounded
stream checks order, lengths, count, a completion digest and exact end of input;
the digest detects altered bytes but does not authenticate their producer.
Only the supported image and logical-layout generations are admitted
([compatibility](../compatibility.md)).

Backup checks unbuffered writes and file synchronization, releases the file,
publishes without replacement and synchronizes the parent. File release uses
Rust Drop; its close result is unobserved. These operations use the existing
filesystem synchronization contract, not a guarantee against every filesystem
or hardware failure. A failure after publication preserves the file and reports
`store.publication_uncertain` with its location.

Restore constructs a private sibling store in bounded confirmed batches. Head
remains absent until complete input, physical validation and full logical/index
audit pass. Only then does it install Head, publish without replacement, complete
activation barriers and reread the exact active metadata under the retained
owner. An aborted or indeterminate batch stops construction without retry. Earlier
confirmed batches may remain in the reported unpublished stage; missing Head
makes both ordinary admission and explicit recovery refuse it.

Failures retain possible unpublished stages or report a published instance when
known. Preserve those paths: restore performs no automatic resume, removal or
replacement. A failed final barrier may leave the destination present and Active
visible; failure is not proof that no work occurred, and neither is a missing
receipt ([command receipts](../tools/cli.md#marrow-backup-and-restore)).

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

Provisioning never replaces an existing destination entry, including an empty
directory, file or symbolic link. The destination basename must be a normal
UTF-8 entry of at most 255 bytes, without control characters, backslashes or a
drive-style prefix such as `a:`. The immediate parent must be an existing
directory, not a symbolic link, with owner read, write and search permissions.
These constraints are checked before staging begins. A single-component
relative destination uses the current directory as its parent.

The runner writes and flushes the provisioning report before publishing the
store. If the destination no longer names the retained store or synchronizing
the parent directory fails after rename, the runner
reports `store.publication_uncertain` with the published instance identity and
leaves the destination in place. Import stops before reading the corpus. The
supervisor exposes a fully delivered uncertainty record as `ProvisionUncertainError`.
A failed success-receipt delivery also leaves the published store in place.
The supervisor waits for child and stream closure and validates the complete
record; missing or invalid delivery has an unknown outcome. Neither result
authorizes automatic reprovisioning.

Provisioning records a pending state before publishing the directory. Ordinary
attach, import and audit refuse a pending store with `store.activation_required`.
After the publication barrier, a failure to confirm final activation is
`store.activation_uncertain`. Explicit [recovery](#recovering-a-store) validates
the stored program and establishes fresh barriers. It does not reconstruct a
lost acknowledgment.

If construction or publication fails before the directory is published, the
runner attempts to remove its owned staging directory. After successful
construction, failed publication checks that the stage name still identifies
the retained directory before removal. Construction-failure cleanup assumes
cooperating parent paths. A cleanup failure retains
both the original error and the actual stage location; removal may have deleted
some contents already. The supervisor exposes a complete cleanup-failure record
as `ProvisionFailedError`. Its stage name is a sibling of the requested store,
not a child. No automatic cleanup retry is performed.

A store provisioned by an older toolchain cannot be converted; its layout
requires fresh provisioning
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

A returned value does not mean the companion has finished closing the store. The
terminal waits for the companion to exit on its own within a separate cleanup
bound and never kills a native companion, since a signal during close leaves the
engine unclean and the next attach refuses it. Unconfirmed cleanup leaves the
invocation result intact and reports the observed PID and retained staging path;
resolve that ownership before another operation on the store, and do not retry
the invocation because cleanup failed. Reaping a companion does not establish
physical or logical integrity.

Generated Node clients report native cleanup the same way:
[`close()`](../tools/typescript-client.md#launching) observes exit without
signalling and rejects when it is unconfirmed. `terminate()` and parent-process
exit remain abrupt and can interrupt a native close.

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

- An identical program opens an active store without changing its program binding.
- A program whose code changed, and whose resources, store roots, indexes, and
  exported functions are unchanged, rebinds the store to the new code. Every
  stored value stays in place, and the next run uses the new code. The transition
  first checks logical contents read-only, then prepares writable service under
  the same owner lock. An inconsistent store is refused before preparation.
  Preparation errors stop before binding metadata changes; physical recovery
  may already have changed engine bookkeeping. The transition records the exact
  old and new heads before replacing the head; activation is confirmed only
  after the metadata barriers and final Head/envelope verification.
- A program whose durable contract or exported interface changed is
  `store.contract_changed`, decided before engine opening even if that engine
  would fail to open. The prior program remains the accepted binding.
  The refusal preserves the owner marker as well as the binding and engine.
- A program whose durable demand exceeds the current standing ceiling is
  `store.demand_exceeds_ceiling`. The refusal names the export,
  the place, and the access. The store is untouched.

An interrupted rebind may leave either recorded head in place. Ordinary access
refuses a pending transition. Recovery requires the program matching the head
actually present; it does not replay the missing update. A third head is refused.

The durable contract is the set of resources, store roots, keys, fields, and
indexes the program declares. No current transition rewrites stored data.
Explicit [`marrow apply`](../tools/cli.md#marrow-apply) accepts verified OLD and NEW
artifacts, preserves all old representations and adds sparse scalar fields.
New fields start absent; ordinary NEW exports can subsequently write them.
The operation verifies OLD against the actual accepted Head, audits its logical
contents read-only, and publishes NEW through the same Pending/Head/Active owner.
It retains the existing ceiling or requires explicit acceptance of its exact
union with NEW demand. Unsupported changes preserve the prior binding;
publication failure may leave an interrupted or uncertain transition.
Broader evolution remains [future work](../future/admission-and-activation.md).

The accepted Head defines each durable identity's physical address. Opening,
recovery and fresh restore retain those addresses, including valid gaps below
the lifetime allocation high-water. Identity coverage, node kinds, unique
numbers and bounds are checked against the presented image and its projection.
Tools that require fresh preorder numbering refuse a different accepted map;
an equivalent preorder map does not require a format fence.

Head digests detect damaged bytes but do not authenticate deliberately rewritten
and resealed metadata. Logical audit cannot detect every same-type field swap
in such a map. Accepted address metadata is part of the trusted local store;
these checks do not establish its historical authorship.

`marrow import` into an existing store never rebinds. It fills the store only
when the compiled program is exactly the active binding; a code-only change is
`store.image_not_active` until a `run --store` rebinds the store, and the
other refusals above apply unchanged. Every refusal is decided before the
store's engine opens, so a refused import writes nothing.

A store whose layout or active image belongs to another generation is
`store.format_version`, refused before engine open — for rebind, audit and
import alike, and even when the newly compiled program has the same durable
contract. The refusal preserves the engine file, head and envelope; lock and
owner-marker bookkeeping may still occur. No command converts such a store in
place, so keep its source, identity ledger, image and matching tools together
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
`run.outcome_unknown`: the call may have run, wholly or in part. For this
invocation-delivery uncertainty, run a read-only export to observe the store
before acting again. This observation does not settle publication durability.

## Auditing a store

`marrow doctor --store <dir>` checks a store's logical contents against its
active program without running an export
([marrow doctor](../tools/cli.md#marrow-doctor)). The project at the working
directory must match that program. The audit admits the image and inspects the
store under its owner lock, then releases the lock before printing its findings
and a digest over the entries. It leaves the engine file, head, envelope and
ownership marker unchanged, including marker absence:

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
same-value commit need not change it. [Project
status](../status.md#trust-boundaries) records the digest's persistence and
authentication limits.

Physical integrity is not checked: a changed scalar can remain valid under its
declared type and pass, including when the stored checksum no longer matches.
Inspection does not repair the engine or clear the unclean-shutdown status
inherited from a prior owner, so a logical audit report is not a recovery result
([status](../status.md#trust-boundaries)). The report's fields, records and exit
codes are in [marrow doctor](../tools/cli.md#marrow-doctor).

## Recovering a store

`marrow recover --store <dir>` validates and activates the store at its current
location without running an export. An explicit `--image` selects an immutable
artifact without compiling a project. Otherwise, the working project's source
and identity ledger must compile to the exact image named by the stored head:

```sh
marrow recover --store ./store
marrow recover --store ./store --format jsonl
marrow recover --store ./store --image ./deployment/program.image
```

After an interrupted apply, select the retained OLD or NEW artifact matching the
head actually present. Recovery refuses the other artifact and does not replay
the missing update.

Recovery acquires the exclusive owner lock, checks the envelope and head before
opening the engine, forces physical integrity checking, and inspects logical
contents against the admitted image. A pending transition permits only its
recorded head or heads. Failure of these initial checks prevents activation. Physical recovery
uses a read-write engine open and is not a read-only inspection.

After successful validation, recovery preserves regular single-link files in the
two metadata replacement slots under generated names rather than replaying or
deleting their bytes. Other slot shapes are refused. At
most one envelope slot and one head slot are moved per attempt. It then
synchronizes the stored artifacts, directory and current parent, writes the
active envelope, and rereads the exact envelope and head for final admission.
The command releases ownership before returning its result; it does not start an
application session.

Recovery leaves logical data and the selected head in place. A legacy version-0
envelope is explicitly upgraded to version 1 through a pending state; supported
head, image and engine stamps are still required. It preserves the instance
identity and does not migrate the data layout or a changed program contract.
Version-0 tools refuse the upgraded envelope. Keep matching source, ledger,
image and tools with stored data ([compatibility](../compatibility.md#versioning)).

The result names preservation moves known to this attempt, including on failure.
Final activation uncertainty is `store.activation_uncertain`. The Active record
may already be visible when its final barrier or reread fails; not every uncertain
result leaves a pending admission veto. Missing result delivery does not prove
that no work occurred. A later recovery performs fresh
validation and cannot reconstruct a previous attempt's lost receipt. [Project
status](../status.md#trust-boundaries) records the remaining trust boundaries.

## Durability

A confirmed commit is written with `fsync` before it is reported. It survives a
process exit and an operating-system crash. Survival of sudden power loss or a
drive-cache reset is not established: the commit path issues `fsync`, not
`F_FULLFSYNC`, so a drive's write cache may still hold the last write.

## Locks

One process owns a store at a time. The runner takes the store's lock when it
attaches and releases it when it exits. An audit holds it through admission and
inspection and releases it before printing. A second process opening a held
store is `store.locked`. Inspection does not publish an owner identity; a
contention diagnostic's marker record may belong to an earlier mutable holder.
[Status](../status.md#trust-boundaries) lists the lock's trust boundaries.
