# Marrow

Marrow is a general-purpose statically typed compiled language for programs that
work with durable data. Its distinctive model is that hierarchical durable
locations are typed language places rather than tables, queries, repository
objects, or string-keyed storage calls.

```text
task.status = Status::done
^tasks[id].status = Status::done
```

The first assignment changes a local value. The second changes durable state.
Both resolve `status` through the language's type system; `^` makes the durable
effect visible. Durable code still has additional rules for presence, keyed
children, transactions, bounded traversal, and failure.

Marrow is language- and compiler-first, and is designed to be built with
production at scale in mind: its architecture, representations, and semantics are
judged against what a widely used general-purpose language and its largest
deployments require, not against what a prototype or demo can get away with. It
is not a query engine and has no supported public serving profile.

The language is unreleased and on a v0.1 beta line. The project is refounding an
entangled prototype: at lane B00 the prototype's compiler, interpreter, and
durable owners were deleted, leaving a retained core (the syntax owner, the
diagnostic-code registry, and an ordered-byte storage engine) plus a thin CLI.
The verticals below — a reproducible program image, an independent verifier, a
bytecode VM, a path kernel, and the durable model — are being rebuilt lane by
lane. A feature is absent until its lane lands it. Current bounds and capability
gaps are honest waypoints on that path, not the design bar.

## Example

This example shows the durable model. It checks on the beta line, runs under
`marrow test` through a fresh ephemeral attachment, and — against a provisioned
store with the companion runner installed — runs under `marrow run --store`.

```mw
module app::tasks

enum Status {
    open
    done
}

resource Task {
    required title: string
    required status: Status
}

store ^tasks[id: int]: Task

pub fn add(id: Id(^tasks), title: string): Id(^tasks) {
    transaction {
        ^tasks[id].title = title
        ^tasks[id].status = Status::open
    }
    return id
}

pub fn complete(id: Id(^tasks)): bool {
    transaction {
        if not exists(^tasks[id]) { return false }
        ^tasks[id].status = Status::done
        return true
    }
}
```

Every durable write occurs in an explicit `transaction`. The caller supplies the
entry identity as an `Id(^tasks)` rather than the store minting one.
[Project status](docs/status.md) identifies current and future work precisely.

## Purpose

Applications commonly repeat the meaning of durable state across source types,
serialization, database access, migrations, external interfaces, and
authorization code. Marrow is designed so that one compiler-owned semantic model
can remove much of that seam while keeping the realities of durable data
explicit:

- point reads and writes can fail;
- collections may be larger than memory and require bounded traversal;
- related writes need visible atomicity;
- code changes must account for existing data; and
- executing code needs authority independent of the accesses it happens to
  contain.

The intended language is useful without a store. Ordinary modules, functions,
algebraic data types, generics, collections, packages, formatting, testing, and
editor support are language foundations, with closures deferred until a
maintained program needs them. Direct durable state is the differentiator, not a
substitute for those foundations.

## Current implementation

Marrow is early and unreleased. The beta line began at lane B00 with a
deliberate capability trough: the entangled prototype owners were deleted and
the trustworthy decoupled parts retained.

| Area | Current implementation |
|---|---|
| Front end | Native lexer, parser, and formatter for `.mw` source with typed parse diagnostics |
| CLI | `marrow init`, `marrow fmt` (a single file or a project directory, `--check`/`--write`), `marrow check` (check a project and describe each export's durable access demand), `marrow run`, `marrow test`, `marrow client typescript`, and `marrow lsp` (the language server), plus `--version`/`--help`; the refounding command names (`data`, `doctor`, `evolve`, `serve`, `backup`, `restore`) report `cli.command_unsupported` until they land |
| Language server | `marrow lsp` runs an in-tree LSP server over stdio, serving diagnostics, whole-document formatting, hover, and definition from the compiler's published analysis facts; it reconstructs no language semantics and opens no store |
| Compiler pipeline | The storeless compiler, reproducible program image, independent verifier, bytecode VM, and typed path kernel run a narrow, growing language subset end to end; a well-formed construct outside the admitted subset is a typed `check.unsupported` diagnostic |
| Storage engine | A private ordered-byte engine contract with in-memory and redb backends under one conformance suite, plus the logical key/value/civil-date codecs; the path kernel is the engine's source-language consumer through a narrow byte seam |

The prototype checker, tree-walking interpreter, catalog, durable lifecycle, and
the `surface`/server/client families were deleted at B00 and are being rebuilt
as new owners. [Project status](docs/status.md) lists what returns through which
lane.

## v0.1 direction

The beta is planned as one canonical distribution with:

- an ordinary storeless language demonstrated by a useful command-line program;
- a light Git/path package workflow with exact pinned dependency edges, a
  separate stable-identity ledger, and a verified offline cache, with no
  dependency lock file or vendoring;
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
generator, compiler-integrated path authorization, a supported
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
Start with [Installation](docs/install.md), then the
[Quickstart](docs/quickstart.md) — checkout to a running durable program.

- [What Marrow is and is not](docs/what-marrow-is.md) states the scope in one page.
- [Documentation map](docs/) explains authority and navigation.
- [Language reference](docs/language/) defines current `.mw` behavior.
- [Tool reference](docs/tools/) and [Operations](docs/operations/) document the
  current CLI and store.
- [Project status](docs/status.md) separates current and future work.
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
