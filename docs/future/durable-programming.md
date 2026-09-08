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

The next durable-language increments keep serial execution and build on the
complete-entry invariant. Place bindings capture keys once, and required fields
and group leaves read through explicit proofs have their declared types today
([named places](../language/durable-places.md#named-places)). A traversal binding
is to retain an automatic presence fact when its region cannot erase entries in
that family; today a pin needs an explicit proof inside its own iteration.

Whole-entry and whole-group reads through a tested or traversal binding are to consume its presence
fact and yield a complete value. Reads through untested bindings remain optional;
an invalidated tested binding must be rechecked or recaptured without a presence
assumption. Detached copies remain valid. One clearing operation covers
local sparse fields, local map entries and durable sparse fields; the durable
mark identifies which state is affected. Reference examples, direct-touch tests
and both applications migrate with these rules.

The compiler composes callee effects once and checks proof uses over resolved
operations in a forward pass; an entry erase invalidates only its exact entry
family, and the current implementation loses knowledge about every key of that
family. Later increments must preserve ordinary storeless work without another
source effect declaration, and add no first-class address values, reference
parameters, borrow-region syntax, key-provenance analysis, or whole-program
fixpoint.

Unsupported images are rejected without changing stored data. Images using
retired instructions require recompilation. General migration of stored data
is future work; the current entry-family layout requires fresh provisioning,
and older stores require matching tools
([compatibility](../compatibility.md#versioning)). Ordinary exports should
initialize and change application data;
retirement of the current data-populating importer is future work. The EMR
baseline data in the `marrow-acceptance` repository needs explicit application
seed exports; replaying ordinary transitions changes its meaning. A source migration can
change demand, so a migrated program must still pass store admission.

Work larger than one invocation advances by application-owned progress over
repeated bounded exports. A non-idempotent batch that can be submitted twice
checks an ordinary durable identity and generation before its effects, and
commits its progress with those effects. A cursor does not freeze a population;
each application states which entries belong to its work. No suspended VM,
automatic continuation, job service or automatic replay is required.

Writer invocations serialize from before their first durable read through
return, including reads before the transaction block. Reader overlap is a later
local increment; [served execution](served-execution.md) retains that same
one-store model for several terminals. Parallel mutating bodies and compiled
reservations are deferred until a measured application justifies them. Backup
and restore are [local applications](local-applications.md).

Branch navigation beneath an absent ancestor requires its keys. Walking
present parents cannot discover children beneath unknown absent ancestors.
Complete subtree enumeration and removal, including such data, remain future
work; the current bounded traversals do not supply backup or restore.

An index is built from one root's own keys and fields. A computed or aggregate
index is not planned.

## Open forks

These designs are undecided. Each states what the language does today.

- Whether a transaction has an explicit rejection exit. Today every `return`
  inside the block commits, and a deliberate failure is returned before the
  first write.
- Whether a traversal binds the whole key tuple. Today the loop variable binds
  one key component, and a composite-keyed layer is not iterated.

Whole-entry assignment continues to create or replace; writes remain statements.
Optional field reads continue to represent both absent entry and absent field
as absence. A separate entry-presence check supplies the distinction when needed.

## Evidence

Evidence for whole-value reads and automatic traversal proofs checks stable
place-binding types through the production compiler. Include possibly equal keys,
late-declared and generic helpers, loops and copied values. Record distinct
return, break and continue transfers during lowering; preserve direct
required-field reads in non-erasing traversals without redundant probes. No
fixpoint or repeated source resolution is an acceptable shortcut.

Compare current whole-value reads, current direct optional-field reads and
checked address reads. Measure allocations and work as declared field width,
referenced sites and executed operations vary independently; include wide groups
and indexed updates. Sparse metadata and group-projection optimization are
separate work. Every kernel operation must have an engine-call bound from its
declared shape or explicit traversal limit. Preserve the current family
navigation bound and encountered-orphan checks
([storage implementation](../implementation/storage.md#navigating-entries)).
Count setup and commit work too, varying unrelated stored population
independently. Scan-call bounds do not establish backend CPU, allocation or
latency guarantees; measure returned pages, setup and disposal separately.
Record Marrow compile latency first, test wall time second, and Rust build time
third. These are evidence requirements, not measured benefits.

Migrate Club Locker, in the `marrow-acceptance` repository, and compare its
ordinary serial behavior on the in-memory and native engines. Populate EMR,
in the same repository, through ordinary seed exports and compare its baseline
state. Club Locker's business functions need no concurrency annotations;
the selected language cleanup does require source edits. The first increment
must be useful with every future worker and job facility absent.
