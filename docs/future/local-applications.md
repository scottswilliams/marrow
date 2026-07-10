# Local applications

This page is future direction. Marrow does not currently ship a supported local
sidecar, generated named-function boundary, or distributable application bundle.

## Goal

The first durable product profile should be a terminal application using one
compiled image, one exact bound store, and one process owner. A later local
sidecar should expose only explicitly exported typed functions to a desktop
renderer. The renderer receives no filesystem path, store handle, raw durable
address, transaction object, ceiling, or maintenance authority.

The intended development loop compiles and verifies a candidate, shows package/
API/effect/contract/binding consequences, and performs any approved activation
while the single writer is quiesced. Source changes never silently reset a
persistent store.

## Acceptance applications

Graph Report is the storeless acceptance program. Club Locker is an offline
equipment-lending application with members, assets, unique tags, checkout/return
history, idempotent commands, application-owned counters and secondary trees,
bounded pages, restart, backup, and restore.

Club Locker should work from the terminal before TypeScript generation or UI
framework work begins. The desktop shell exists to test the host seam, not to
make Marrow a UI framework.

## Distribution

A release bundle for one qualified beta platform should pin the image,
runtime/sidecar, selected private engine, generated client and renderer assets,
provisioning policy, and application identity. Install, first provision, start,
code update, explicit authority expansion, backup, restore, uninstall, and data
retention need separate tested behavior. End users should not install Rust or a
database.

## Evidence target

One populated application must retain state across supported code and contract
changes, crashes, backup/restore, terminal and TypeScript calls, and clean-machine
installation. Its business functions and durable model should later run under a
served profile without being rewritten around transport or CRUD.
