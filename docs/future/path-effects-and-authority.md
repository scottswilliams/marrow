# Path effects and authority

An export's authority over durable data is the intersection of what its code
demands, what its store accepted, and what the invocation is granted. Demand
describes need and grants nothing.

## Today

Every export has a demand: the durable places it reads and writes, through
every function it calls ([access demand](../language/durable-places.md#access-demand)).
Demand is the union along the call graph, which has no cycles. Index
maintenance is part of a write's demand
([traversal and indexes](../language/traversal-and-indexes.md)).

A program image carries its demand as its deployment ceiling, and `marrow
image` writes the image once that ceiling's id is accepted
([`marrow image`](../tools/cli.md#marrow-image)). A store keeps the ceiling it
was provisioned with. An export that demands a place outside that ceiling is
`store.demand_exceeds_ceiling` before any durable work begins
([changing the program](../operations/README.md#changing-the-program)). This is
the whole of authority today: one local owner, one store, and read and write as
the two kinds of access. Grants finer than read and write are future work
([status](../status.md#not-yet-available)).

## Direction

A grant names one image, one export, and one store together. It holds the
places the export demands, intersected with the store's ceiling, and nothing
more. A later authenticated principal can only further intersect an address or
a context predicate. It can narrow an already-granted reach; it can never add a
place the grant did not carry.

Demand describes operations a program may execute over semantic paths. The
compiler and verifier compose it once from resolved operations and callee
summaries. Presence checking and writer classification consume those facts;
they do not introduce another source declaration, authority grant or scheduling
envelope. A grant covers only its permitted operations and region, so reading
one entry does not authorize walking its root.

An address alias neither grants authority nor reserves its target. A presence
proof establishes a condition about an entry in the invocation's view, not
permission to access it. Refactoring to a checked address or whole-entry
assignment can change demand and must still pass ordinary store admission.

Stored users, credentials, and rotation records are inert data. They cannot be
decoded into an authenticated context or a grant; the trust anchor for
authentication stays outside application durable state. Maintenance,
activation, backup, restore, and physical recovery use authority that
application code cannot hold.

Three things are deferred. Closures and recursion would require a separate
decision about indirect-call demand. Key provenance is not needed for serial
writer admission or conservative entry-family invalidation. Principals, roles,
and served enforcement belong to
[served execution](served-execution.md).

## Evidence

Hover and the change review show, for each export, the operation and place it
demands, the traversal bound, and the call that carries the demand. A refactor
that changes no demand changes no authority. An edit that widens demand runs
only after the store's ceiling and the invocation's grant cover it. A forged
image, path, or grant reaches zero engine calls.
