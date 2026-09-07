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

The complete-entry invariant is current: every present entry has its complete
required payload. Whole-entry assignment creates or replaces an entry; a
field, group, or group-leaf write updates an entry that a presence proof
covers through a `place` or a traversal pin, and is refused otherwise; `delete`
is the one clearing form; and a proof ends at its block, at an erase of the
family, or at a call whose demand writes the family. The proof forms and the
loop rule are defined in [durable places](../language/durable-places.md#named-places).
The kernel refuses an incomplete entry from a hand-built image with
`run.corruption`.

## Direction

Provisioning creates control metadata and evaluates no application
initializer. Initial data is written afterward through ordinary exports, so no
initial value reruns on attach, restart, update, or restore.

A mutating invocation's whole call graph performs its host work before its
first durable access. No host effect exists today
([path effects and authority](path-effects-and-authority.md)).

The next durable-language increments keep serial execution and build on the
complete-entry invariant. A place binding captures keys once; an ordinary value
binding copies data. A presence-tested binding is to give required fields their
ordinary declared types; sparse fields remain optional. Today every read
through a place is optional. A traversal binding is to use the same field types
and retain its presence fact when the loop region cannot erase entries in that
family; today a pin is proved inside its own iteration. Types stay stable:
validity checking does not retroactively turn a required read into an optional
one. An untested address supports optional reads without a presence
assumption.

Whole-entry reads through a tested or traversal binding consume its presence
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

Older store/image formats are refused without changing their data; a matching
older toolchain remains necessary to use them. General data migration is
separate work. Ordinary exports initialize and change application data; the
separate data-populating importer is retired. The EMR baseline data in the
`marrow-acceptance` repository needs explicit application seed exports;
replaying ordinary transitions changes its meaning. A source migration can
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

Evidence for typed reads through a proved place checks stable place-binding
types through the production compiler. Include possibly equal keys,
late-declared and generic helpers, loops and copied values. Record distinct
return, break and continue transfers during lowering; preserve direct
required-field reads in non-erasing traversals without redundant probes. No
fixpoint or repeated source resolution is an acceptable shortcut.

Compare current whole-value reads, current direct optional-field reads and
checked address reads. Measure allocations and work as declared field width,
referenced sites and executed operations vary independently; include wide groups
and indexed updates. Sparse metadata and group-projection optimization are
separate work. Every kernel operation must have an engine-call bound from its
declared shape or explicit traversal limit. Family navigation must visit present
immediate entries without a population-dependent walk through descendant-only
prefixes; the first increment relocates entry presence into an ordered family
namespace rather than adding a second membership index. Preserve bounded orphan
checks when separating presence from payload addresses. These structural bounds
let the existing instruction budget bound invocation engine-call counts without
a new per-operation counter. Count setup and commit work too, varying unrelated
stored population independently. This is not a backend CPU, allocation or
latency guarantee; measure setup and disposal separately.
Record Marrow compile latency first, test wall time second, and Rust build time
third. These are evidence requirements, not measured benefits.

Migrate Club Locker, in the `marrow-acceptance` repository, and compare its
ordinary serial behavior on the in-memory and native engines. Populate EMR,
in the same repository, through ordinary seed exports and compare its baseline
state. Club Locker's business functions need no concurrency annotations;
the selected language cleanup does require source edits. The first increment
must be useful with every future worker and job facility absent.
