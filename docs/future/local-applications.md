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

Terminal and generated TypeScript clients should call the same stable named
business exports and one domain type graph. Marrow source should not receive a
transport envelope, raw path, transaction object, retry token, authority token,
or handwritten host DTO. Adding the generated boundary must not change a
business function.

Durable presence, domain results, finite pages, and a page export's bounded
opaque continuation should be part of that shared detached type graph. The
continuation uses a source-visible nominal brand for one named
branch/direction/result contract and is validated by generated code. Its sealed
generic representation has no public constructor; different page exports have
different brands. Generated-code branding does not authenticate hostile wire
bytes; runtime validation contains any structurally valid token within the
named export's already accepted page region. This does not make general places
serializable or carry read authority.

## Acceptance applications

Graph Report is the storeless acceptance program. Club Locker is an offline
equipment-lending application with members, assets, unique tags, checkout/return
history, application-owned counters and secondary trees, bounded pages, restart,
backup, and restore. A generated client should acknowledge a normally decoded
mutation reply through the host protocol before resolving its promise. A direct
terminal instead acknowledges immediately after successful rendering and flush,
then exits successfully only after that acknowledgement. Both are transport
progress, not application ceremony. If the reply or acknowledgement is lost,
the host classifies the single interrupted attempt against store-side truth and
exposes its durable status. Read-only exact and paged exports remain available
while that status is unaccepted, so the user can perform a typed state refresh
before explicitly accepting the observation. The host must not replay the
business action automatically. A later mutation or maintenance change waits
for that acceptance. Status and acceptance controls carry no
application arguments, result bytes, retry token, read authority, or executable
refresh, and they never replay the business action.

Missing or malformed delivery state must fail closed without making a valid
store permanently unreadable. A recovery-only owner may inspect and logically
back up current typed state after explicitly abandoning delivery knowledge, but
mutation resumes only in a freshly restored deployment and store with a fresh
empty delivery record.

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
changes, crashes, lost replies, backup/restore, terminal and TypeScript calls,
and clean-machine installation. Its business functions and durable model should
later run under a served profile without being rewritten around transport or
CRUD. Fresh application-developer walkthroughs must exercise checkout, exact
erase versus bounded prune, live pages, effect broadening, and lost-reply
reconciliation, including normal reply acknowledgement and a following
mutation, lost acknowledgement, read-only refresh, and client death after
acknowledgement, before the host protocol freezes.
