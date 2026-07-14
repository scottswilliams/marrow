# Durable programming

This page is future direction. The current resource, managed-index, `nextId`,
and transaction behavior remains documented in the current reference while its
implementation is reachable.

## Goal

The beta should add durable declarations to one ordinary struct and algebraic-
data-type system rather than maintain a parallel database value language. Durable
state is a forest of typed sparse ordered trees.

A `store` root is either a singleton or a keyed tuple; named `branch` placements
are keyed. Singleton roots, keyed roots, branches, fields, group leaves, and
indexes are distinct graph nodes. Provision creates only control metadata; it
evaluates no application initializer. Initial application data is written
afterward through ordinary explicit exports and transactions, so no initial value
reruns on attach, restart, update, or restore. The beta topology graph is finite
and acyclic with explicit depth and width limits. Recursive ordinary values
remain local; recursive durable relationships use keys in a finite branch
topology rather than recursive schema expansion.

A present payload is one finite `resource` value. Resource fields are sparse by
default: an absent field is a distinct state from a present field, and `required`
fields are always present. Groups are static field-path namespaces with no value
or presence of their own. Keyed child branches are a declaration kind separate
from payload fields; they are not collection values materialized inside a payload.

Reading a durable place produces a durable-presence result distinct from ordinary
`Option`. A present payload and an absent place remain different states, and a
present `Option`-none field is distinct from an absent field. Store outage, denied
authority, corruption, budget exhaustion, and an indeterminate commit are faults,
not absence.

## Operations

The closed durable operations are presence, read, create, replace, and erase.
Creation is non-overwriting; replacement is update-only; whole-payload replacement
erases omitted sparse fields and preserves every keyed descendant; exact erasure
removes one finite payload while preserving keyed descendants. Field operations
observe or change one sparse leaf. Creation tests only the exact payload, so a
descendant-only node can receive a new payload without changing its descendants;
an application that needs a fresh subtree uses non-reused keys or an explicit
bounded cleanup rather than an unbounded check hidden in create. Broader subtree
removal is a distinct operation; work larger than one transaction is advanced by
application-owned typed progress over repeated bounded batches. Mutation outcomes
are closed payload-free values that are handled rather than discarded; code that
needs an old payload performs an explicit read. There is no upsert, optional-value
deletion coercion, cascading whole-value assignment, or implicit allocation.

## Indexes and secondary state

The beta provides narrow compiler-maintained indexes over a keyed sparse
resource: one nonunique ordered projection and one unique exact-lookup projection,
each maintained atomically with the primary payload in the same transaction.
Application code cannot write an index directly, and a unique collision is a
closed typed outcome on the source mutation. Applications may additionally
maintain monotonic allocation counters, secondary access trees, and histories as
ordinary durable data in the same transaction as primary state. The beta has no
`nextId`, predicate/computed/aggregate index, cross-root join, or child fan-out.

## Transactions and access

A mutating invocation has one explicit outer transaction. Only its lexical owner
can request commit or reject the business change; helpers join the transaction but
cannot begin or finish another. Writes outside that owner, nested transactions,
implicit fallthrough, control flow across its boundary, and host effects inside it
are rejected. A mutating invocation's entire transitive call graph is host-effect-
free from the transaction onward: any host work occurs before the first durable
access. The commit and rejection terminals both return the transaction
expression's ordinary result type, while only commit persists staged writes; this
permits an intentionally committed error value without giving `Result` special
transaction behavior. Ordinary typed propagation may occur inside helpers but
cannot cross the transaction owner; when the owner maps a result to its rejection
terminal, that deliberate rejection rolls back without poisoning. Runtime,
validation, and budget faults poison and roll back. The serialized beta performs
no automatic retry; an engine result whose commit outcome is unknown poisons the
attempt and requires identity-based reconciliation on reopen, which observes
either the complete old or the complete new state.

Read-only invocations observe one coherent snapshot. Exact presence is a point
operation. Potentially large branches are not iterable collection values and are
not exposed as pages, cursors, or resumable continuations. Traversal is ordinary
nested `for` iteration over a root, branch, or index with an explicit compile-time
`at most N` bound, an optional inclusive lower bound, and mandatory overflow
handling that runs exactly when more keys than the bound exist. The loop body
binds the immediate key tuple; a payload or presence fact requires an explicit
operation. A nonunique index is traversed by progressive prefix refinement; a
unique index supports only complete-key exact lookup. The runtime does not hide an
unbounded loop or retry behind local-looking syntax.

## Evidence target

A terminal-first lending application must exercise allocation counters, secondary
trees, one nonunique and one unique compiler-maintained index, exact
read/create/replace/erase, descendant preservation, bounded nested traversal with
overflow handling, transaction rejection and faults, interrupted-attempt
classification without automatic retry, ordinary typed state refresh after a lost
return value, a multi-batch business job whose progress survives that lost value,
restart, backup, and restore — through the same source functions over the memory
conformance model and one private native engine.
