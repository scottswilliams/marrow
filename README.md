# Marrow

Marrow is an experimental statically typed programming language being developed
as a general-purpose language for programs that work with durable data. Its
distinctive model is that hierarchical durable locations are typed language
places rather than tables, queries, repository objects, or string-keyed storage
calls.

```text
task.status = Status::done
^tasks(id).status = Status::done
```

The first assignment changes a local value. The second changes durable state.
Both resolve `status` through the language's type system; `^` makes the durable
effect visible. Durable code still has additional rules for presence, keyed
children, transactions, bounded traversal, and failure.

Marrow is language- and compiler-first. It is not a new query engine and has no
supported public serving profile. The current implementation checks source and
executes it with a tree-walking interpreter; it does not currently emit bytecode
or native machine code. Reproducible, independently verified program images and
a portable VM are future work.

`marrow serve --remote` remains reachable legacy behavior. Its HTTP server is
synchronous, does not provide TLS or compiler-integrated authorization, and is
unsuitable for untrusted networks. It will be removed with the prototype
surface and generated-client family.

## Example

This example is current Marrow and is checked in CI.

```mw
module app::tasks

enum Status
    open
    done

resource Task
    required title: string
    required status: Status

store ^tasks(id: int): Task

pub fn add(title: string): Id(^tasks)
    var task: Task
    task.title = title
    task.status = Status::open

    const id: Id(^tasks) = nextId(^tasks)
    ^tasks(id) = task
    return id

pub fn complete(id: Id(^tasks)): bool
    if not exists(^tasks(id))
        return false

    ^tasks(id).status = Status::done
    return true
```

`resource` declarations, `nextId`, writes outside explicit transactions, and
managed indexes are implemented prototype behavior. They remain documented in
the current reference while reachable, but they are not assumptions for the
v0.1 beta design. [Project status](docs/status.md) identifies current,
transitional, and future work precisely.

## Purpose

Applications commonly repeat the meaning of durable state across source types,
serialization, database access, migrations, external interfaces, and
authorization code. Marrow investigates whether one compiler-owned semantic
model can remove much of that seam while keeping the realities of durable data
explicit:

- point reads and writes can fail;
- collections may be larger than memory and require bounded traversal;
- related writes need visible atomicity;
- code changes must account for existing data; and
- executing code needs authority independent of the accesses it happens to
  contain.

The intended language is useful without a store. Ordinary modules, functions,
algebraic data types, generics, closures, collections, packages, formatting,
testing, and editor support are language foundations. Direct durable state is
the differentiator, not a substitute for those foundations.

## Current implementation

Marrow is early and unreleased.

| Area | Current implementation |
|---|---|
| Front end | Native parser, formatter, modules, functions, resources, enums, control flow, static checking, and structured diagnostics |
| Durable state | Typed hierarchical places, direct reads and writes, ordered traversal, transactions, managed indexes, inspection, backup, and restore |
| Existing data | Accepted declaration identities and preview/apply workflows for supported populated-data changes |
| Execution | Checked in-memory representation executed by a tree-walking interpreter |
| Storage | Memory and redb implementations behind the current ordered-tree API |
| Tooling | Source-built CLI for check, run, test, format, data, evolution, backup, restore, and experimental local serving/client generation |

The experimental surface/server/client and user-facing storage-cost models are
legacy and will be removed. The checker/interpreter/catalog/redb stack is
current but transitional: it establishes useful behavior and test evidence, not
the final compiler architecture.

## v0.1 direction

The beta is planned as one canonical distribution with:

- an ordinary storeless language demonstrated by a useful command-line program;
- a light Git/path package workflow with an exact lock, offline cache, and
  vendoring;
- canonical, independently verified program images and a portable VM;
- direct durable trees over ordinary language values, explicit transactions,
  and bounded ordered traversal;
- compiler-described durable effects intersected with independently accepted
  execution authority at one path kernel;
- one private qualified native storage substrate rather than a public backend
  matrix;
- exact executable/store binding, narrow activation, logical backup, and
  fresh-store restore; and
- a terminal-first local application followed by generated TypeScript bindings
  and a supervised local sidecar.

Unimplemented details are indexed through the [documentation map](docs/) and
are not current syntax or guarantees. The repository deliberately has no parallel
specification or ADR archive; implemented code, tests, and concise current
reference pages move together.

## Scope

Marrow does not currently provide a query language, planner, ORM, general CRUD
generator, compiler-integrated path authorization, bytecode, a supported
desktop bundle or public serving profile, replication, high availability, or
institutional application certification.

In the target architecture, storage engines supply ordered bytes, atomic
transactions, durability, and native recovery beneath a private boundary.
Analytical search, messaging, HTTP, identity providers, and other systems
integrate through typed boundaries when a program needs them. Marrow source
should not inherit their physical concepts.

## Lineage

MUMPS demonstrates the usefulness of direct hierarchical durable state in
long-lived transactional systems. It is product evidence and inspiration, not a
compatibility target. Marrow does not inherit M syntax, dynamic typing,
schema-by-convention, implementation architecture, or historical tooling limits.

Hierarchical and orthogonal persistence, effect systems, capability systems,
content-addressed code, typed routing, and integrated databases all have prior
art. Marrow's direction combines selected ideas around a compiler-owned model;
novelty and benefit must be established by working software and measured
evidence rather than asserted in the README.

## Build and documentation

Installation currently requires the pinned Rust toolchain and a source build.
Start with [Installation](docs/install.md) and the
[Quickstart](docs/quickstart.md).

- [Documentation map](docs/) explains authority and navigation.
- [Language reference](docs/language/) defines current `.mw` behavior.
- [Tool reference](docs/tools/) and [Operations](docs/operations/) document the
  current CLI and store.
- [Project status](docs/status.md) separates current, legacy, and future work.
- [Vision](docs/vision.md) states the product direction and boundaries.
- [Implementation guide](docs/implementation/) maps the Rust code that exists.
- The documentation map links unimplemented direction without treating it as
  proposed syntax.
- [Security policy](SECURITY.md) gives the reporting channel and current trust
  boundary.
- [Contributing](CONTRIBUTING.md) gives the development and verification
  workflow.

## License

Apache-2.0
