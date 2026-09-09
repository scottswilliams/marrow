# Local applications

A local application is one program, one store, and one process owner on one
machine. The terminal and a desktop shell call the same exports.

## Today

```text
terminal:  marrow run <export> --store ./store  ->  marrow-runner  ->  store
desktop:   Electron main  ->  generated client  ->  marrow-runner  ->  store
```

`marrow run --store` runs one export against a store through the runner
([operations](../operations/README.md)). `marrow client typescript` writes a
client with one method per export; a Node supervisor starts the runner over a
private local channel, and the renderer stays isolated from the store
([TypeScript client](../tools/typescript-client.md)). A business function has
the same signature whether the terminal or the client calls it.

A lost reply is reported as outcome-unknown. A fresh session may read state for
application-specific reconciliation; this does not by itself resolve an unknown
outcome. Bounded traversal stays inside the program; no runtime cursor or page
token crosses the channel.

Two programs carry the evidence: a storeless graph report
(`fixtures/v01/conformance/graph_report`) and an equipment-lending desktop
application, Club Locker. Club Locker and the EMR change-set tool live in the
separate `marrow-acceptance` repository, which runs both against a built
toolchain through the public commands ([status](../status.md#applications)).

## Direction

A local bundle for one platform pins the image, the runner, the engine, the
generated client and renderer assets, the provisioning policy, and the
application identity. The end user installs neither Rust nor a database.
Install, first provision, start, code update, authority expansion, backup,
restore, uninstall, and data retention each have their own tested behavior.

The beta targets terminal and native-store operation on Linux and macOS, and
the existing macOS desktop shell. Linux desktop packaging is deferred. Clean
installation and actual desktop interaction need their own evidence; a native
terminal CI pass does not establish them.

The threat boundary is one trusted local owner and host filesystem. Qualify
accidental corruption, interrupted writes, identity/binding mistakes, ownership
and lost replies. Hostile filesystem substitution or rollback, authentication,
encryption and multi-principal policy remain outside this beta's assurance.
Checksums and logical digests must not be described as authentication.

## Evidence

One populated application keeps its state across code changes, contract
changes, crashes, lost replies, backup and restore, terminal and client calls,
and a clean install. Include a sparse-field update, exact erasure that preserves
children, bounded history removal after references are cleared, traversal with
overflow, and a lost reply followed by reconciliation without replay. A complete
logical backup includes every declared entry family, even beneath absent
ancestors, and restores into a validated fresh store before serving invocations.

The external suite must test application invariants on both sides of a
relationship, not merely whether commands succeed. Maintained use must span at
least four actual weeks, including a real update and backup/restore. Automated
time or generated traffic cannot replace it. Reusing business functions under a
later served profile is a hypothesis to test
([served execution](served-execution.md#promotion-test)), not a compatibility promise.
