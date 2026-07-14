# Local applications

This page is future direction. Marrow does not currently ship a supported local
runner, generated named-function boundary, contained desktop renderer, or
distributable application bundle.

## Goal

The first durable product profile should be a terminal application using one
compiled image, one exact bound store, and one process owner, so the durable
model is proven terminal-first. The v0.1 release gate for the personal local application is
invocation through a generated strict TypeScript client supervised by an
Electron/Node application. The generated client is the release gate rather than a
later addition, and the end user installs neither Rust nor a database.

Named typed exports are the boundary. Terminal and generated TypeScript clients
call the same stable named business exports over one domain type graph. Marrow
source receives no transport envelope, raw path, transaction object, authority
token, or handwritten host data-transfer object. Adding the generated boundary
must not change a business function.

The generated TypeScript client and the local wire run as an exact matched
release pair. The transport is a supervised private Unix-domain-socket channel
between a trusted supervisor — the Node main process or a command-line owner —
and a no-shell child runner, established through a bounded handshake. Standard
streams are drained byte logs, never protocol.

Durable presence and domain results belong to the shared detached type graph.
Potentially large branches are not exchanged as pages, cursors, or resumable
continuations over the wire; bounded ordered traversal remains a runner-side
language operation with an explicit compile-time bound and overflow handling. No
page token, continuation, or cursor crosses the boundary.

The desktop renderer is contained: context isolation, a sandboxed process, no
remote content, and named preload methods only. It receives no filesystem path,
store handle, raw durable address, transaction object, ceiling, or maintenance
authority. Invocation authority is verified demand intersected with a deployment
ceiling and export attenuation.

The intended development loop compiles and verifies a candidate, shows package,
API, effect, contract, and binding consequences, and performs any approved
activation while the single writer is quiesced. Source changes never silently
reset a persistent store.

## Acceptance applications

Graph Report is the storeless acceptance program. Club Locker is an offline
equipment-lending application with members, assets, unique tags, checkout and
return history, application-owned counters and secondary trees, bounded ordered
traversal, restart, backup, and restore.

Replies are honest about interruption. A lost reply after a mutating invocation
or a host handoff is reported as outcome-unknown and reconciled by an ordinary
domain read against store-side truth; the observed state is either the complete
prior state or the complete new state. There is no automatic replay, delivery
ledger, exactly-once claim, or durable delivery record. Read-only exact exports
remain available while an outcome is unknown, so the user performs an ordinary
typed state refresh before continuing. The host does not replay a business action
on the user's behalf.

Club Locker should work from the terminal before TypeScript generation or UI
framework work begins, and the generated TypeScript client supervised by the
Electron shell is the profile's release gate. The desktop shell exists to test
the host seam, not to make Marrow a UI framework.

## Distribution

A release bundle for one qualified beta platform pins the program image, the
runner, the selected private engine, the generated client and renderer assets,
the provisioning policy, and the application identity. There is no separately
installed database or daemon. Install, first provision, start, code update,
explicit authority expansion, backup, restore, uninstall, and data retention
each need separate tested behavior. End users install neither Rust nor a
database.

## Evidence target

One populated application must retain state across supported code and contract
changes, crashes, lost replies, backup and restore, terminal and TypeScript
calls, and clean-machine installation. A lost reply is reconciled by an ordinary
domain read rather than by replay or a delivery record. The same business
functions and durable model should later run under a served profile without being
rewritten around transport or CRUD.

Fresh application-developer walkthroughs must exercise checkout, exact erase
versus broader subtree removal advanced by application-owned typed progress
over repeated bounded batches, bounded traversal with overflow handling, effect
broadening, and lost-reply reconciliation through an ordinary domain read —
including a normal reply followed by a further mutation, an interrupted attempt
whose outcome is unknown, and a read-only refresh before continuing — before the
host protocol freezes.

This page states direction. See the [vision](../vision.md) for product
progression and [durable programming](durable-programming.md) for the durable
model these applications exercise.
