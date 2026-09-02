# Durable programming

Durable declarations use the same struct and enum types as local values.
Durable state is a forest of typed sparse ordered trees.

## Today

Resources, store roots, transactions, indexes, and bounded traversal are
current and defined in the reference:
[resources](../language/resources.md),
[durable places](../language/durable-places.md),
[errors and transactions](../language/errors-and-transactions.md), and
[traversal and indexes](../language/traversal-and-indexes.md).

A present payload is one finite `resource` value. Fields are sparse by
default: an absent field is a distinct state from a present field, and
`required` fields are always present. Absence is a value (`T?`); outage,
denied authority, corruption, and an exhausted budget are faults. A write does
not return the old value; a program reads it first when it needs it. An
interrupted commit reopens as `known_old`, `known_new`, or `unknown`
([operations](../operations/README.md#interrupted-commits)).

## Direction

Provisioning creates control metadata and evaluates no application
initializer. Initial data is written afterward through ordinary exports, so no
initial value reruns on attach, restart, update, or restore.

A mutating invocation's whole call graph performs its host work before its
first durable access. No host effect exists today
([path effects and authority](path-effects-and-authority.md)).

Work larger than one transaction advances by application-owned progress over
repeated bounded batches. Removing a whole subtree is one such job; today it is
a bounded traversal plus a `delete` per entry.

The store serializes writers and performs no automatic retry. Concurrent
writers are [served execution](served-execution.md). Backup and restore are
[local applications](local-applications.md).

An index is built from one root's own keys and fields. A computed or aggregate
index is not planned.

## Open forks

These designs are undecided. Each states what the language does today.

- Whether a transaction has an explicit rejection exit. Today every `return`
  inside the block commits, and a deliberate failure is returned before the
  first write.
- Whether creating an entry is distinct from replacing one. Today whole-entry
  assignment does both.
- Whether a durable read distinguishes an absent entry from an absent field.
  Today both read as `absent`.
- Whether a traversal binds the whole key tuple. Today the loop variable binds
  one key component, and a composite-keyed layer is not iterated.
- Whether a write yields an outcome value. Today a write is a statement.

## Evidence

Club Locker, run from the terminal over the in-memory engine and the native
engine, is the evidence; [local applications](local-applications.md) lists
what it must exercise.
