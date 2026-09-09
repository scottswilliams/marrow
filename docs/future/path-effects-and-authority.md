# Path effects and authority

Demand describes which durable operations an export may perform. It grants
nothing.

## Today

Each export's demand includes durable operations through its acyclic call graph,
including managed-index maintenance
([access demand](../language/durable-places.md#access-demand)).
The image carries a deployment ceiling accepted by the owner, and the store
retains that ceiling. An invocation demanding more is refused before durable
work ([changing the program](../operations/README.md#changing-the-program)).
The current profile has one local owner; fine-grained principals and grants
are future work.

## Direction

Effective authority must intersect verified program demand, exact accepted
image, the store's separately accepted maximum ceiling and invocation
attenuation. An authenticated principal may narrow that intersection, never
expand it. Exact entry access does not imply permission to traverse its family.

The compiler owns resolved source effects. The independent verifier
reconstructs them from image bytes. Presence checking, writer classification,
tools and runtime consume their owners' typed facts rather than inventing
another effect declaration or path classifier. A presence proof or address
alias grants no access.

Maintenance, activation, backup, restore and physical recovery have distinct
trusted authority unavailable to application code. Stored users or credentials
are data, not an authentication trust anchor.

The beta preserves the current single-owner ceiling boundary. Principal policy,
route publication, indirect-call demand and key-provenance analysis are
deferred with [served execution](served-execution.md). No policy language or
authorization framework is a beta prerequisite.

## Evidence

A changed program that widens demand needs explicit ceiling acceptance.
Wrong images, paths and grants refuse before application engine access. Later
principal enforcement must demonstrate narrowing, revocation and traversal
distinctions using the same semantic path owner, with no downstream re-parsing.
