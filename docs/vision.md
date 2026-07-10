# Vision

Marrow is intended to be a statically typed language for durable operational
software: programs whose state is long-lived, transactional, and central to
their behavior.

The central model is direct. Resource declarations describe tree-shaped
values. A `^` path addresses durable state. Ordinary functions read, assign,
delete, and iterate those paths, and a `transaction` block groups durable
changes atomically. Persistence is part of the language rather than a separate
query or object-mapping API.

## Compiler-first durable state

Marrow's product boundary is the language and compiler, not a new database
engine. Storage substrates provide ordered persistence, transactions,
durability, and recovery beneath the language. The compiler is intended to own
the application meaning that otherwise becomes duplicated among types,
storage calls, migrations, public interfaces, and authorization checks.

That ownership includes:

- value, presence, and entry-identity types;
- stable semantic identities for durable paths;
- direct and transitive path effects of functions;
- transaction and host-effect requirements;
- changes to populated durable data;
- explicit public address projections; and
- the maximum path authority required by callable code.

The compiler must produce a reproducible program artifact without opening a
user store. Runtime admission compares that artifact with a particular store;
execution never infers program meaning from physical keys.

## One semantic path model

A source spelling, stable schema identity, concrete keyed address, public URI,
authority region, and physical storage key can refer to related places without
being the same representation.

```text
source path
    -> stable semantic path
        -> concrete address with typed keys
            -> private logical and physical encodings
```

Public addressing and authorization should project from the same semantic
path graph. Publishing an address does not grant permission, and permission
does not expose physical storage. Evolution relates graph versions while
preserving or explicitly changing semantic identity.

## Product profiles

The first profile is an embedded local application: a trusted host owns the
program and store while an untrusted UI receives generated typed callable
bindings. This targets the space currently served by a desktop web UI plus an
embedded SQL store, without requiring an application server.

The later served profile uses the same durable model and ordinary business
functions with authenticated principals and multiple terminals. Promotion is a
real architectural test: becoming served should not require rewriting the
application around transport, CRUD, or a second data model.

Target workloads include scheduling, inventory, work orders, case management,
terminal systems, and clinical or administrative systems. They share sparse
hierarchical state, direct element access, long histories, and transactions.
FHIR and institutional compliance remain integration and evidence problems,
not claims implied by the language design.

## Boundaries

Marrow should integrate with established components at typed boundaries:

- Electron, browser, terminal, and native UI frameworks;
- storage and replication systems;
- HTTP, TLS, identity providers, and enterprise directories;
- messaging, scheduling, analytics, and search systems; and
- operating-system and deployment tooling.

Marrow does not need a query planner, ORM, generated record CRUD, automatic
REST publication, database leaderboard, UI framework, identity provider,
analytics engine, or replication protocol.

## Lineage

MUMPS demonstrates that direct hierarchical durable state is useful in
long-lived transactional systems. It is product evidence and inspiration, not
a compatibility target. Marrow does not inherit M syntax, dynamic typing,
schema-by-convention, tooling limitations, or historical runtime architecture.

Hierarchical persistence, orthogonal persistence, typed effects, capability
security, and typed routing all have prior art. Marrow's hypothesis is that
combining them around one compiler-owned path model yields a smaller and more
coherent application boundary than repeating the same meaning across several
layers.

## Evidence

The vision is direction, not evidence. Documentation should say whether a
behavior is current, legacy, or future; whether a guarantee is enforced by the
compiler or runtime; and whether a claim is tested, measured, or operationally
assumed. Marrow should not describe itself as safe, scalable, portable, or
institution-ready without the corresponding evidence.
