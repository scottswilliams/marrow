# Vision

Marrow is intended to be a general-purpose statically typed compiled language
whose distinctive capability is direct interaction with durable hierarchical
data.

An ordinary program should be able to define types and functions, transform
local collections, use packages, call explicit host facilities, and run without
a store. A durable program should use the same language while making durable
places, transaction boundaries, potentially large traversal, and authority
visible. Persistence should require no parallel table/document model,
serializer, repository layer, or string-keyed database API.

Marrow is not an experimental or hobby language. It is designed to be built with
production at scale in mind: its architecture, representations, and semantics are
judged against what a widely used mainstream language and its largest deployments
require, not against what a prototype can get away with, and no design may assume
smallness of programs, data, teams, or deployment lifetime. The v0.1 beta scope
below is a milestone on that path, not the ceiling; current bounds and capability
gaps are honest, evidence-widened waypoints rather than the bar. This states an
ambition and a design standard; it is not a maturity claim. What may be called
production-ready is governed by the [status](status.md) page and the evidence
rules it inherits.

## Language and compiler first

Marrow's product boundary is one canonical language implementation: package
graph, compiler, immutable program image, independent verifier, bytecode VM,
path kernel, lifecycle, tools, and reference. It is not an abstract language
standard whose usable implementations are left to database vendors. The v0.1 beta
qualifies one target rather than claiming a portable virtual machine.

Storage substrates provide ordered bytes, snapshots, atomic transactions,
durability, and native recovery behind a private boundary. They do not define
Marrow types, source paths, effects, authorization, evolution, or public APIs.
The project should select one qualified private engine for a release rather than
turn backend choice into a language feature.

The compiler is intended to own the application meaning that conventional
systems often repeat across source types, persistence calls, migrations,
external interfaces, and authorization code. Compilation itself remains
storeless. A separate lifecycle admits and binds an exact verified image to a
particular store before durable execution.

## Language experience

Marrow should optimize for the shortest honest program. Domain values and
ordinary functions should dominate source, while every visible durable
construct should correspond to a real difference in presence, mutation,
atomicity, work, failure, or authority. Compiler-derived identity, effect, and
lifecycle facts belong in diagnostics, hover information, and change review
rather than being copied into business signatures.

A first project should support a complete storeless check, format, test, build,
and run journey. Adding retained state should add typed durable declarations,
exact operations, one transaction boundary, and bounded traversal without also
adding a schema duplicate, serializer, repository, connection, transaction
object, or storage administration. Long-lived maintenance should expose
identity-preserving renames and package, API, effect, retained-data, and binding
changes before a store can be activated under new code.

This progression gives the language a recognizable center: ordinary typed code
around visibly durable places, with broader work and atomic changes stated
where they occur. Marrow should not become a query API, a handwritten effect or
capability calculus, or a small pure language followed by a separate
persistence sublanguage.

Large durable programs should retain that shape. Traversal is ordinary nested
`for` iteration over roots, branches, and narrow managed indexes with an explicit
compile-time bound and explicit overflow handling; there is no public page,
cursor, or resumable continuation value. One transaction may apply complex
multi-place business logic to a bounded batch. Work larger than one safe
transaction uses application-owned typed progress and repeated batches, so
restart and lost-result recovery remain explicit without replacing the domain
program with a query planner, storage cursor, or repository callback API. Closures
and richer traversal forms are deferred until a maintained program is materially
worse without them.

## Durable data as language data

Working with durable state should resemble working with local data where the
physics permit it:

- paths and keys are typed rather than assembled as strings;
- point reads, creation, replacement, and exact erasure address exact elements;
- ordinary functions express business behavior;
- a visible transaction groups atomic durable changes; and
- narrow compiler-maintained indexes and application-owned secondary trees give
  additional access paths, maintained atomically with primary state.

The language must not hide that durable data can be absent, larger than memory,
contended, unavailable, or damaged. Ordered traversal is explicit and bounded.
Exact erasure and broader subtree removal are different operations. Writes can
fail. A compiler check cannot prove business intent or hardware durability.

## One semantic coordinate system

The same durable declaration can participate in several related models without
collapsing them into one string or identifier:

```text
source declaration
    -> stable semantic identity
        -> concrete address with typed keys
            -> private logical and physical encoding
```

Package lineage and snapshots, source spelling, durable representation,
concrete address, store identity, executable binding, public URI, authority
region, and physical key have different owners and lifecycles.

This separation enables a compiler to report how a package or code change
affects types, durable representation, callable effects, and a particular
store. Marrow can project these facts from stable semantic identities while
retaining their different owners and lifecycles. Inferred effects describe
demand; they never grant permission.
Runtime access should be the intersection of verified demand, an exact accepted
candidate, a separately owned maximum ceiling, and invocation attenuation at
one path kernel.

## Product progression

The v0.1 beta should establish two independent acceptance programs:

- a useful storeless command-line program exercises the ordinary language,
  package workflow, compiled image, verifier, VM, formatter, and editor facts;
- a personal local application exercises durable values, transactions, ordered
  trees, narrow managed indexes, executable/store binding, recovery, backup, and
  restore. Its durable model is proven terminal-first, and its release gate is
  invocation through a generated strict TypeScript client supervised by an
  Electron/Node application whose end user installs neither Rust nor a database.

The later served profile should run the same images, durable declarations, and
ordinary business functions under authenticated principal/client invocations.
Becoming served must not require a CRUD service layer or a second data model.
Concurrent execution, principal policy, public paths, online evolution,
replication, and high availability are separate later problems.

Candidate domains include inventory, scheduling, work orders, case management,
terminal systems, and clinical or administrative software. Naming those domains
does not establish production, compliance, or institutional readiness.

## Boundaries

Marrow does not need a query planner, `EXPLAIN`, ORM, generated record CRUD,
automatic REST publication, database leaderboard, UI framework, identity
provider, analytics engine, or replication protocol.

HTTP, TLS, identity providers, Electron or native UI frameworks, messaging,
search, analytics, and operating-system services should integrate through typed
host boundaries when needed. A library or application can build a database
system in Marrow; the core language does not need to become one.

## Lineage and evidence

MUMPS demonstrates that direct hierarchical durable state can support important
long-lived transactional systems. It is product evidence and inspiration, not a
compatibility target. Marrow does not inherit M syntax, dynamic typing,
schema-by-convention, implementation architecture, or historical tooling limits.

Hierarchical and orthogonal persistence, effect and capability systems,
content-addressed code, typed routing, language-integrated databases, and local
application runtimes all have prior art. Marrow's combination must earn its
complexity through working applications, a smaller trusted boundary, precise
failure behavior, and reproducible measurements.

This page states direction, not implementation evidence. [Project
status](status.md) identifies what is current, legacy, and future; the
[reference](language/) defines only current behavior.
