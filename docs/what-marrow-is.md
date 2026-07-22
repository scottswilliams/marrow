# What Marrow Is and Is Not

This page states Marrow's scope in one place. It is a summary; the
[vision](vision.md) gives the direction, the [status](status.md) gives the current
evidence, and the [language reference](language/) defines current behavior.

## What Marrow is

Marrow is a general-purpose statically typed compiled language whose distinctive
capability is direct interaction with durable hierarchical data. Its product
boundary is one canonical implementation: a package graph, a compiler, a
reproducible program image, an independent verifier, a bytecode VM, a path kernel,
lifecycle tooling, and a reference.

- **An ordinary language first.** Modules, functions, algebraic data types,
  generics, local collections, packages, formatting, source tests, and editor
  facts are the foundation. A program that uses no durable data needs no store.
- **Durable data as language data.** A durable location is a typed place written
  with `^`. Point reads, creation, replacement, and exact erasure address exact
  elements through typed keys; a visible `transaction` groups atomic changes; and
  narrow compiler-maintained indexes give additional access paths. Persistence
  needs no parallel table or document model, serializer, repository layer, or
  string-keyed storage API.
- **Explicit about the physics of durable data.** The language keeps visible that
  durable data can be absent, larger than memory, contended, unavailable, or
  damaged. Traversal is ordinary nested `for` with an explicit compile-time bound
  and explicit overflow handling. Writes can fail, and the failure families stay
  distinct.
- **Compiler-owned meaning.** One compiler owns the resolved names, types, durable
  graph, effects, and exports that conventional systems often repeat across source
  types, persistence calls, migrations, interfaces, and authorization code. The
  compiler opens no store; a separate lifecycle binds a verified image to a store
  before durable execution.

Data is navigated, not queried. A program reads or changes individual durable
elements and iterates explicit subtrees using typed paths and ordinary control
flow — the same way it works with local state.

## What Marrow is not

- It is **not a query language or database engine.** There is no query planner,
  `EXPLAIN`, cost model, scan cursor, or result-set API, and none is planned as
  the model. Storage substrates supply ordered bytes, atomic transactions,
  durability, and native recovery behind a private boundary; backend choice is not
  a language feature.
- It is **not an ORM, repository, or CRUD framework.** No object–relational
  mapping, entity hydration, repository objects, query builder, or generated CRUD
  stands between code and data.
- It is **not a relational or document system in disguise.** There are no rows,
  columns, joins, or `SELECT`-shaped operations, even as analogies. The model is a
  typed hierarchy of durable places.
- It is **not a UI, service, or identity framework.** HTTP, TLS, identity
  providers, native and Electron UI frameworks, messaging, search, and analytics
  integrate through typed host boundaries when a program needs them; the core
  language does not become one of them.
- It is **not MUMPS or an M implementation.** MUMPS is evidence that direct
  hierarchical durable programming can support important long-lived systems. It is
  not a compatibility target, and Marrow inherits no M syntax, dynamic typing,
  schema-by-convention, or tooling.

## Current status in one line

Marrow is unreleased and on a v0.1 beta line. The compiler, program image,
verifier, VM, and path kernel run a narrow, growing language subset end to end; a
well-formed construct outside that subset is a typed `check.unsupported`
diagnostic. Current bounds and capability gaps are honest waypoints, not the
design bar. The [status](status.md) page is the authority on what is implemented,
and no claim of production readiness, safety, or scale is made here without the
evidence that page requires.
