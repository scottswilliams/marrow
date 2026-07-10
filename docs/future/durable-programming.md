# Durable programming

This page is future direction. The current resource, managed-index, `nextId`,
and transaction behavior remains documented in the current reference while its
implementation is reachable.

## Goal

The beta should add durable declarations to one ordinary struct and algebraic-
data-type system rather than maintain a parallel database value language.
Durable state is a forest of typed sparse ordered trees. Roots and keyed entries
can be absent; a present entry has one complete dense finite value. Keyed child
layers are declarations, not fields materialized inside that value.

Reading a finite value and assigning it back must preserve every keyed
descendant that was not materialized. Deleting an entry and deleting a broader
subtree are explicit operations with different effects and authority.

## Transactions and access

A mutating invocation should have one explicit outer transaction. Only its
lexical owner can commit or abort; helpers join the transaction but cannot
finish it. Host effects are unavailable inside. A deliberate typed abort by the
owner, including typed result propagation, rolls back without poisoning the
transaction. Runtime, validation, and budget faults poison and roll back the
transaction. An engine result whose commit outcome is unknown requires
identity-based reconciliation rather than an unsafe retry.

Read-only invocations observe one coherent snapshot. Exact presence is a point
operation. Potentially large collections use bounded ordered traversal whose
cursor cannot escape its snapshot or transaction.

Applications maintain monotonic allocation counters, secondary access trees,
and histories as ordinary durable data in the same transaction as primary
state. The compiler does not manufacture `nextId` or privileged index
maintenance.

## Evidence target

A terminal-first lending application must exercise idempotent commands,
allocation, secondary trees, exact point access, bounded paging, transaction
failure, restart, backup, and restore through the same source functions over the
memory conformance model and one private native engine.
