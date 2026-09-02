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

A lost reply is reported as outcome-unknown. After an unknown outcome the
application opens a fresh session and reads the state it needs before
continuing. Bounded traversal stays inside the program; no cursor or page token
crosses the channel.

Two programs carry the evidence: a storeless graph report
(`fixtures/v01/conformance/graph_report`) and an equipment-lending desktop
application (`apps/club-locker`).

## Direction

A release bundle for one platform pins the image, the runner, the engine, the
generated client and renderer assets, the provisioning policy, and the
application identity. The end user installs neither Rust nor a database.
Install, first provision, start, code update, authority expansion, backup,
restore, uninstall, and data retention each have their own tested behavior.

## Evidence

One populated application keeps its state across code changes, contract
changes, crashes, lost replies, backup and restore, terminal and client calls,
and a clean install. Walkthroughs cover four journeys: checkout; exact erase
beside batched removal of a subtree; bounded traversal with overflow; and a
lost reply followed by a read. The same functions later run under a served
profile without a rewrite ([served execution](served-execution.md)).
