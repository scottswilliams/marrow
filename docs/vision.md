# Marrow Vision

This page describes Marrow's long-term purpose and architectural direction. It
does not define current `.mw` behavior. The current language is defined by the
[Language Reference](language/), and implementation state is recorded in
[Project Status](status.md).

## Purpose

Marrow is intended to be a statically typed language for durable operational
software: programs whose state is long-lived, transactional, and central to
their behavior.

Its central programming model is direct:

- resource declarations define typed tree members reused by local values and
  durable places;
- `^` identifies a durable path;
- ordinary functions read, assign, delete, and iterate durable paths;
- transactions group durable changes.

Durable places add presence, keyed-child, transaction, and storage semantics;
they are not required to behave exactly like local values. Marrow retains one
ordinary expression and control-flow language for both.

## Compiler Ownership

Marrow is language- and compiler-first. The compiler is intended to own the
meaning of durable paths across the parts of an application that otherwise
drift apart:

- value and presence types;
- stable schema path identities and typed entry identities;
- function path effects;
- transaction requirements;
- changes to populated data;
- explicitly published URI representations;
- authorization scopes;
- editor, build, and deployment facts.

Each durable root is intended to have one semantic owner, with transitive path
effects making cross-module access visible. Each successful transaction should
produce one internal logical change fact; UI, audit, replication, and other
consumers receive separately authorized projections outside the transaction.

Storage engines implement ordered durable operations, transactions, recovery,
and other physical mechanisms beneath this semantic boundary. They are not the
source of Marrow language meaning or the project's product identity.

Every logical tree access uses the path kernel. Physical substrate recovery is
a separate named trusted component beneath that kernel: it is unavailable to
application principals and cannot return a store to service until the physical
store and accepted schema state validate and read-only admission returns a fresh
already-active verdict for the supplied image and store binding.

## One Semantic Path Graph

The intended path graph contains stable schema path identities. A concrete
durable address is a route through those nodes instantiated with typed key
values. Related representations remain distinct:

| Representation | Relation to the graph |
|---|---|
| Source path | A versioned spelling that resolves to schema path identities |
| Concrete durable address | A graph route instantiated with typed entry keys |
| Public URI | A partial encoding of an explicitly published address space |
| Authority scope | A typed set of concrete addresses a principal may observe or change |
| Physical key | A private storage-substrate encoding of a concrete address |

Evolution relates graph versions and refers to their stable schema path
identities; it is not another path identity. These mappings need not be
bijective. One schema node has many keyed addresses, only selected addresses may
be public, and several authorized regions may overlap.

Three uses of “identity” must remain separate:

- a **schema path identity** names a durable declaration node across accepted
  program changes;
- an **entry identity**, written `Id(^root)` today, is a typed key for one entry
  in a root;
- a **store UID** names one populated durable-store instance.

Current catalog metadata represents accepted declaration identities and parts of
this model; it is not automatically the final path-graph representation.

## Compilation, Store Admission, And Activation

Source compilation should reproducibly produce a versioned program image
without opening a user store. The image records program meaning and its stable
schema path identities. Its accepted contract must name the executable
representation, loader and verifier, module linking and initialization order,
host-capability imports, source/debug mapping, target portability, and image
compatibility. “Program image” does not by itself choose bytecode, native code,
or a virtual machine.

Store admission is read-only. It compares an image with the store's accepted
durable-schema state and snapshot. If the image and schema are already active,
admission returns an already-active verdict and no activation is needed. If a
transition is permitted—including a transition that changes only the image
binding—it returns an exact witness bound to the image, store UID, accepted
schema state, prior active-image binding, substrate profile, and observed
commit, as well as the canonical transition plan and explicit operator decisions.
Otherwise it returns a rejection verdict. A proposed change may preserve
identity, require a deterministic transform, require an explicit destructive
decision, or be unsupported.

Activation consumes an exact still-valid witness and atomically transitions
data, accepted schema state, and the store's active-image binding.
The immutable program-image artifact remains outside that transaction; the
binding records its digest and format identity. Activation emits a receipt only
after the transition commits; the receipt binds the consumed witness, resulting
commit and logical-data-state digest, accepted schema state, and active-image binding.
An image may run against the store only when its identity matches the binding.
Compilation establishes program meaning,
admission establishes whether a particular snapshot may transition, and
activation performs the transition.

The current `marrow evolve preview` and `marrow evolve apply` commands implement
a narrower populated-data evolution workflow. They are precursors, not the
general program-image admission and activation contract described here.

The admission report should cover schema identities, populated-data obligations,
public URI compatibility, authority changes, host capabilities, and
generated-binding changes. Witnesses and activation receipts supplement rather
than replace source and intent review.

## Embedded And Served Profiles

The initial product profile is an embedded local application with one trusted
owner and no required service. The same language is intended to support a
served profile with authenticated principals and multiple terminals.

The profiles should implement one reference transition semantics. A served
runtime may interleave transactions, but every committed history must correspond
to a permitted ordering under the declared isolation contract. Retry behavior,
non-repeatable host effects, conflicts, and failure visibility are part of that
contract and remain target-design work. Local owner mode supplies an explicit
full-tree authority; it is not an authorization bypass.

The architectural test is promotion: a useful embedded application should be
able to become a shared service without replacing its durable model or ordinary
business functions.

## Public Paths And Authority

Durable data is private unless explicitly published. Publishing a path creates
an external representation; it does not expose physical storage or grant
authority.

Authentication adapters establish typed principal identity. Authorization is
intended to use typed path capabilities whose construction and delegation are
restricted to named trusted runtime components. Invocation and every logical
tree access pass the same enforcement boundary. Generated clients, HTTP
adapters, terminal hosts, logical maintenance tools, and embedded calls must not
create alternate enforcement paths.

Information-flow guarantees require a separately specified model of labels,
observables, declassification, and side channels. Marrow should not describe a
partial protected-flow analysis as general confidentiality proof.

## Integration Boundaries

Marrow is not intended to implement every component of an application stack.
It should integrate with established systems at typed boundaries:

- Electron, browser, terminal, or native UI frameworks;
- storage engines and replication systems;
- HTTP and TLS implementations;
- operating-system identity, OIDC, SMART, and enterprise directories;
- messaging, scheduling, observability, and audit sinks;
- analytical and specialized search systems.

Marrow owns durable operational semantics. These components own their
respective protocols and physical mechanisms.

## Scope

Marrow is not intended to become a general-purpose language by accumulating
unrelated capabilities. It aims to cover the durable operational logic of its
target applications, with explicit integration for UI, analytics, and external
effects.

The hierarchical model is deliberate rather than universal. It is suited to
sparse state with owned nested structure, typed keyed paths, direct element
access, and transactions. Specialized analytical and retrieval workloads
integrate through typed external boundaries.

## Lineage

MUMPS demonstrates the utility of direct hierarchical durable state in
long-lived transactional systems. Marrow takes that result as a starting point,
not as a compatibility obligation. Static typing, explicit resource shapes,
stable semantic paths, compiler-owned evolution, structured tooling, and
integrated path authority are independent design choices.

Hierarchical and orthogonal persistence, type-and-effect systems, capability
security, and typed routing all provide relevant prior art. Marrow's distinct
hypothesis is that one stable typed path model can connect application code,
durable layout, store admission, activation and evolution, public addressing,
and authority.

## Evidence And Claims

Marrow documentation uses precise evidence terms:

| Term | Meaning |
|---|---|
| Designed | Recorded direction that is not a current contract or implementation |
| Accepted target | A human-approved unimplemented contract in `docs/design/` |
| Implemented | A reachable non-test implementation exists |
| Compiler-enforced | Invalid source is rejected by the compiler |
| Runtime-enforced | Dynamic cases are checked on every supported execution path |
| Conformance-tested | Implementations pass a shared behavioral corpus |
| Measured | A reproducible empirical result is published |
| Formally established | A theorem and its assumptions are published |
| Operational assumption | Correctness depends on the deployment environment |

Unqualified claims that Marrow is safe, proven, scalable, or institution-ready
are not supported by the vision alone.
