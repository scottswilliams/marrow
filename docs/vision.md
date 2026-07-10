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

## Language and compiler first

Marrow's product boundary is one canonical language implementation: package
graph, compiler, immutable program image, independent verifier, portable VM,
path kernel, lifecycle, tools, and reference. It is not an abstract language
standard whose usable implementations are left to database vendors.

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

## Durable data as language data

Working with durable state should resemble working with local data where the
physics permit it:

- paths and keys are typed rather than assembled as strings;
- point reads and writes address exact elements;
- ordinary functions express business behavior;
- a visible transaction groups atomic durable changes; and
- application code maintains secondary access trees and allocation policy with
  the same transaction semantics as primary state.

The language must not hide that durable data can be absent, larger than memory,
contended, unavailable, or damaged. Ordered traversal is explicit and bounded.
Writes can fail. A compiler check cannot prove business intent or hardware
durability.

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
store. Inferred effects describe demand; they never grant permission. Runtime
access should be the intersection of verified demand, an exact accepted
candidate, a separately owned maximum ceiling, and invocation attenuation at
one path kernel.

## Product progression

The v0.1 beta should establish two independent acceptance programs:

- a useful storeless command-line program exercises the ordinary language,
  package workflow, compiled image, verifier, VM, formatter, and editor facts;
- a terminal-first local application exercises durable values, transactions,
  ordered trees, executable/store binding, recovery, backup, and restore before
  adding generated TypeScript bindings and a supervised desktop sidecar.

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
