# Durable programming

This page is future direction. The current resource, managed-index, `nextId`,
and transaction behavior remains documented in the current reference while its
implementation is reachable.

## Goal

The beta should add durable declarations to one ordinary struct and algebraic-
data-type system rather than maintain a parallel database value language.
Durable state is a forest of typed sparse ordered trees. Fresh activation
initializes required singleton roots from verified deterministic pure initial
values; those initial values never rerun on attach, restart, update, or restore.
For the beta, an initial value is a bounded canonical expression made only from
literals and finite value constructors; it cannot call a function, loop,
recurse, or perform host or durable work. The compiler and verifier construct
the same finite image value without a general compile-time evaluator.
Keyed entries are optional; a present entry has one complete dense finite value.
Keyed child branches have an explicit declaration kind separate from payload
fields. They are not collection values or fields materialized inside that value.
The beta topology graph is finite and acyclic with explicit depth and width
limits. Recursive ordinary values remain local; recursive durable relationships
use keys in a finite Branch topology rather than recursive schema expansion.

Reading an optional keyed place produces a durable-presence result that is
distinct from ordinary `Option`. A present optional payload and an absent place
therefore remain different states. Store outage, denied authority, corruption,
and resource exhaustion are faults, not absence.

Exact creation is non-overwriting. Exact replacement is update-only. Exact
erasure removes one finite payload while preserving keyed descendants. Broader
subtree pruning is a separate bounded operation that returns a removal count
and whether work remains; it never materializes the removed values. A prune over
one selected keyed Branch may also return the last fully exhausted key of that
Branch. Its single beta order is semantic post-order: child Branch declarations in stable
semantic-identity order, keys within each Branch in forward language key order,
and the addressed payload last. Source reordering does not change this order;
reverse prune is not a beta operation. Mutation
outcomes are closed payload-free values that are handled rather than discarded;
code that needs an old payload performs an explicit read. There is no upsert,
optional-value deletion coercion, cascading whole-value assignment, implicit
allocation, or compiler-maintained index.

Creation tests only the exact payload; a descendant-only node can therefore
receive a new payload without changing its descendants. An application that
requires a fresh subtree uses non-reused keys or completes an explicit bounded
cleanup protocol before reuse rather than hiding an unbounded check in create.
An immediate Branch page enumerates present payload markers at that Branch,
not descendant-only prefixes. Those prefixes remain reachable by exact known-key
addresses and bounded subtree pruning. The physical layout must let a page seek
the payload-marker namespace directly, so a long run of descendant-only nodes
does not violate the stated page-work bound.

## Transactions and access

A mutating invocation should have one explicit outer transaction. Only its
lexical owner can request commit or reject the business change; helpers join
the transaction but cannot begin or finish another one. Writes outside that
owner, nested transactions, implicit fallthrough, control flow across its
boundary, and host effects are rejected. A mutating invocation's entire
transitive call graph is host-effect-free, including work before or after its
transaction. A read-only durable invocation may use an explicit bounded host
grant, although a detached sidecar profile may admit only host-effect-free
exports. Expected outcomes remain ordinary typed values: the commit and
rejection terminals both return the transaction
expression's ordinary result type, while only commit persists staged writes.
This permits an intentionally committed error value without giving `Result` or
propagation syntax special transaction behavior. Ordinary typed result
propagation may occur inside ordinary helpers, but it cannot cross the
transaction owner. When the owner explicitly maps such a result to its rejection
terminal, that
deliberate rejection rolls back without poisoning the transaction. Runtime,
validation, and budget faults poison and roll back the transaction. The
serialized beta performs no automatic retry, and an engine result whose commit
outcome is unknown requires identity-based reconciliation.

Read-only invocations observe one coherent snapshot. Exact presence is a point
operation. Potentially large branches are not iterable collection values. A
page returns and decodes at most its stated count and may examine one additional
key to report an accurate continuation. An exactly full terminal page has no
continuation when the lookahead reaches the end. A continuation is exclusive
live keyset progress across invocations: it is bound to one branch, parent
address, direction, key order, named export/result contract, and StoreId, but is
neither a repeatable snapshot nor authority. It does not bind a store revision,
so compatible later calls observe live changes; cross-store and post-restore
reuse rejects before engine access.

Every page and prune site has a compiler-known positive maximum. A dynamic
request is checked against that maximum, and the inferred effect carries the
finite maximum rather than a symbolic arithmetic expression. A page
byte ceiling must admit one maximum-sized entry of its detached result type;
aggregate byte pressure may shorten a page but does not add an ordinary
item-too-large branch to every export. A page
continuation has a source-visible nominal brand for its one page contract and a
sealed generic representation; separate page exports cannot exchange tokens,
and ordinary application code has no constructor. Wire bytes remain untrusted:
the host and kernel validate every embedded field, while a structurally valid
same-contract token may choose a different live start position within the page
export's already accepted keyed-layer demand. Branding improves type safety; it
is not authentication or authority.

Applications maintain monotonic allocation counters, secondary access trees,
and histories as ordinary durable data in the same transaction as primary
state. The compiler does not manufacture `nextId` or privileged index
maintenance.

A page's finite entries should use the ordinary collection and loop vocabulary.
Complex business logic may read and update many exact places inside one bounded
transaction without receiving a storage iterator or transaction object. A
transaction-local bounded page reports finite entries plus whether more keys
exist; it does not mint an exported live continuation. A job
larger than the transaction budget records an ordinary typed last key, cutoff,
or work queue in durable application data and advances it atomically with each
batch. Capturing a monotonic high-water key can give one live scan a finite
membership boundary; applications needing different inclusion semantics state
them in their progress type. The runtime does not hide an unbounded loop or
retry protocol behind local-looking syntax.

## Evidence target

A terminal-first lending application must exercise allocation, secondary trees,
exact read/create/replace/erase/prune, descendant preservation, forward and
reverse paging, transaction rejection and faults, interrupted-attempt
classification without automatic retry, ordinary typed state refresh after a
lost return value, a multi-batch business job whose progress survives that lost
value, restart, backup, and restore through the same source functions over the
memory conformance model and one private native engine.
