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

The next durable-language increment keeps serial execution and establishes one
invariant: every present entry has complete required payload. Whole-entry
assignment creates or replaces that payload; field and group assignment update
an entry whose presence has been checked. Required group leaves participate in
the same invariant. Partial field-created entries and their commit-time
completion protocol are removed together.

A place binding captures keys once; an ordinary value binding copies data.
One place-binding family replaces the separate address keyword and traversal
pins in one migration. A presence-tested binding gives required fields their
ordinary declared types; sparse fields remain optional. A traversal binding
uses the same field types and retains its presence fact when the loop region
cannot erase entries in that family. Otherwise the body must recheck before a
required read or update. A subsequent erase through a possibly matching
alias or helper invalidates proof-dependent uses before execution. Copied values
remain valid. Types stay stable: validity checking does not retroactively turn a
required read into an optional one. An untested address supports optional reads
without a presence assumption.

Whole-entry reads remain optional copied values, including reads through an
address whose earlier presence check has ended. One clearing operation covers
local sparse fields, local map entries and durable sparse fields; the durable
mark identifies which state is affected. Reference examples, direct-touch tests
and both applications migrate with these rules.

The compiler composes callee effects once and checks proof uses over resolved
operations in a forward pass. The image verifier checks types, demand and
transaction ownership; the kernel checks operation preconditions even for an
image supplied without source proofs. The first implementation may conservatively
lose knowledge about different keys in the same entry family. It must preserve ordinary storeless
work without another source effect declaration. It adds no first-class address
values, reference parameters, borrow-region syntax, key-provenance analysis, or
whole-program fixpoint.

The first release of this invariant provisions new stores. Older store/image
formats are refused without changing their data; a matching older toolchain
remains necessary to use them. General data migration is separate work.
Ordinary exports initialize and change application data; the separate
data-populating importer is retired. Existing EMR baseline data needs explicit
application seed exports; replaying ordinary transitions changes its meaning.
Source migration changes address bindings, guarded updates and implicit counter
creation. It can change demand, so the migrated program must still pass store
admission. This is selected direction, not current syntax or implemented behavior.

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

The first evidence lane checks stable place-binding types and destructive-use
refusal through the production compiler before the full migration. Include
possibly equal keys, late-declared and generic helpers, loops and copied values.
Record distinct return, break and continue transfers during lowering; preserve
direct required-field reads in non-erasing traversals without redundant probes.
No fixpoint or repeated source resolution is an acceptable shortcut. Separate
evidence must establish complete required groups and kernel preconditions for
images that omit presence guards; such images may be refused at execution.

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

Migrate Club Locker and compare its ordinary serial behavior on the in-memory
and native engines. Populate EMR through ordinary seed exports and compare its
baseline state. Club Locker's business functions need no concurrency annotations;
the selected language cleanup does require source edits. The first increment
must be useful with every future worker and job facility absent.
