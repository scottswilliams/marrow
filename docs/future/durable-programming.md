# Durable programming

Durable state is a forest of typed sparse ordered trees. Local values and
durable payloads use declared value types; a present entry contains one complete
resource value.

## Today

[Resources](../language/resources.md), [durable places](../language/durable-places.md),
[transactions](../language/errors-and-transactions.md) and
[bounded traversal](../language/traversal-and-indexes.md) define implemented
behavior. Explicit presence proofs permit required field and group-leaf reads
with their declared types. Sparse and untested reads remain optional. A parent
entry's erasure leaves its keyed descendants in place.

## Selected beta rules

Keep explicit places and presence guards. Capture keys once; erasing an entry
family invalidates its presence facts conservatively, including through helpers.
A copied resource value is detached from its entry. Whole-entry assignment
creates or replaces a complete payload. `delete` remains the single clearing
form. Automatic traversal and index-hit proofs and bare whole-value reads are
deferred; they require no speculative provenance solver or new reference syntax.

Ordinary enum and product payloads should compose without declaration-order or
generic-substitution exceptions. Widen only the value combinations a maintained
caller needs, consistently across their production boundaries. Nominal-bearing
public aggregates and stored values remain refused until their constraints
survive the executable representation
([compiled programs](compiled-programs.md)). Do not widen unrelated syntax to
remove an application string-tag workaround.

Select one durable test model. Setup uses ordinary seed exports, each
transaction-owning export executes as a normal invocation, and read-only
observations inspect committed state between calls. Tests need no extra public
read exports solely for assertions. Migrate the current direct-write tests and
remove their implicit transaction path and the direct/driver split together.

Provisioning creates metadata, then ordinary exports seed application data.
The data-populating importer retires only after the external EMR baseline has an
equivalent seed path and bounded external ingestion remains possible. Setup
must not rerun on attach, restart, update or restore.

## Work and lifetime

One writer owns the invocation from before its first durable decision read
through return. Reads before its transaction belong to that same invocation.
Work larger than one invocation uses repeated bounded exports with
application-owned progress committed alongside the effects. No suspended VM,
automatic continuation or job service is required.

Complete backup and restore must visit every declared entry family, including
children beneath absent ancestors. Walking only present parents cannot do that.
General application subtree enumeration/removal, automatic cascading deletion,
composite-key traversal and operations over currently unsupported singleton or
nested-group places are deferred. Exact deletion must keep its current meaning;
the acceptance applications must maintain their own references and lifetime rules.

## Evidence

Production source tests cover direct and generic value composition, captured
possibly equal keys, helper erasure, loops and detached copies. Exit tests cover
explicit and propagated returns before entry, inside a transaction and after
commit; helper returns; faults during return-value evaluation; and refused
re-entry or durable access after commit. No path commits twice or leaks staged
writes, and test assertions inspect the state produced by real invocation
boundaries.

Migrate Club Locker and EMR together with their expectations. Preserve their
initial and final logical data, including descendant-only entries, and compare
in-memory and native execution. Record compile latency, test wall time, Rust
build time and bounded engine work; scan-count bounds alone do not establish
backend latency or cache residency.
