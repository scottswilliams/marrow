# Marrow

Marrow is an experimental statically typed programming language in which
hierarchical paths can denote durable application state.

```text
task.status = Status::done
^tasks(id).status = Status::done
```

The first assignment changes a local value. The second changes durable data.
Both resolve `status` through the same resource member and type; `^` marks the
durable place. Durable places additionally have presence, keyed-child,
transaction, and storage rules. Programs use ordinary expressions and control
flow to address and change tree elements; persistence is not a separate
source-language API.

## Example

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

`resource Task` defines typed tree members. `store ^tasks(id: int): Task`
attaches those members to a durable keyed root. `Id(^tasks)` is the nominal
entry identity type for that root and is not interchangeable with an entry
identity from another root. `nextId(^tasks)` proposes the next entry identity
from the current tree; the following write, rather than the call itself,
occupies it.

Marrow programs read, write, delete, and iterate durable paths with ordinary
language constructs. A `transaction` block groups several durable changes into
one atomic unit. Declared indexes are alternate trees maintained with their
owning store.

## Purpose

Marrow explores a language-first approach to durable operational software.

In a conventional application, the meaning of durable state is often repeated
across language types, persistence calls, migration files, external interfaces,
and authorization code. Marrow instead makes the durable tree part of the
language's type and effect system.

The current implementation applies that model to source checking,
accepted declaration identities, managed writes, transactions, indexes,
backup and restore, and supported changes to populated data. The longer-term
design extends one semantic path graph to public URI representations and path
authorization.

The intended development path is:

```text
embedded local application
        ↓
shared transactional service
```

The durable model and ordinary business functions should survive that
transition. This direction is intended for applications such as scheduling,
inventory, case management, work orders, and clinical or administrative
systems whose state is both long-lived and frequently changed.

## Current Implementation

Marrow is early software. The repository currently contains:

| Area | Implementation |
|---|---|
| Language | Parser, formatter, modules, functions, static checking, resources, enums, presence, keyed layers, entry identities, control flow, and transactions |
| Durable state | An embedded ordered-tree backend, direct path reads and writes, managed indexes, inspection, backup, and restore |
| Existing-data changes | Accepted declaration identities, read-only preview, state-bound witnesses, and apply steps for supported changes |
| Execution | A tree-walking interpreter over a checked executable representation |
| Tooling | `check`, `run`, `test`, `fmt`, data tools, evolution, backup, restore, and structured diagnostics |

The runtime does not currently emit bytecode or native machine code.
“Compiler-first” describes where Marrow intends semantic ownership to reside;
it is not a claim of native compilation today.

[Project Status](docs/status.md) separates current behavior, legacy mechanisms,
and future direction.

## Architectural Direction

The long-term design is organized around compiler-owned semantic paths. A
stable schema path identifies one node in the program's durable model. A
concrete durable address follows those nodes with typed key values. Source
names, public URIs, authority regions, and physical storage are related to that
address without becoming the same representation.

For example, the following rows could describe one logical place. The URI and
authority rows are illustrative design, not current Marrow syntax.

| Role | Representation |
|---|---|
| Source | `^patients(patientId).visits(visitDate)` |
| Stable schema path identity | Compiler-owned identities for the `patients` and `visits` nodes |
| Concrete address | Those nodes instantiated with one patient ID and visit date |
| Published URI | `/patients/{patientId}/visits/{visitDate}` |
| Authority region | Permission to observe or change addresses in that patient's visits |
| Physical key | A private encoding chosen by the storage substrate |

These mappings need not be one-to-one. Evolution relates versions of the path
graph while preserving or explicitly changing schema path identities.

The design is intended to compile a program image reproducibly, admit that image
against a particular populated store, report consequential changes before
activation, enforce authority at every application-level durable access, and
change storage substrates without exposing backend-specific concepts in `.mw`
source.

See [Vision](docs/vision.md) for the design principles and
[Project Status](docs/status.md) for their implementation state.

## Lineage And Scope

The `^` notation and direct use of hierarchical durable state are inspired by
MUMPS and its descendants. Marrow is not a subset, implementation, or
compatibility layer for M. It retains that starting idea without treating M
syntax, dynamic typing, schema-by-convention, or historical tooling constraints
as requirements.

Marrow targets operational state addressed through typed hierarchical paths and
explicitly declared indexes. Storage engines implement transactions and ordered
tree operations beneath the language boundary. Analytical, search, and other
specialized workloads integrate through typed external boundaries.

## Why A Language?

Many parts of Marrow can be provided by libraries and generated code. A library
can enforce substantial properties when all durable access remains behind its
API. Marrow tests whether making durable paths language places, and checking all
application-level `.mw` access in one compilation, produces a smaller trusted
boundary and one coherent path-identity and effect model across typing,
store admission, activation, evolution, tooling, public addressing, and
authority. Administrative repair, restore, and physical storage remain named
parts of the trusted runtime boundary.

Hierarchical persistence, orthogonal persistence, effect systems, capability
systems, and typed routing all have prior art. The project's hypothesis is their
combination around stable typed path identity, not that any one mechanism is
new. If that combination does not materially improve guarantees or usability
over a library, a new language would not be justified.

## Current Limitations

Marrow should presently be treated as an experimental language implementation,
not as a production database or institutional application platform.

- Installation requires a Rust toolchain and a source build.
- Linux and macOS are the supported platforms.
- Execution uses a tree-walking interpreter.
- The native persistent profile has one owning process or session while open
  for writing.
- Durable roots are project-wide under the current language.
- Compiler-integrated, runtime-enforced path authorization is not implemented.
- Public URI projection and embedded-to-served promotion remain design work.
- Current surface and storage-accounting mechanisms are legacy and are not the
  long-term model.

See [Project Status](docs/status.md) and
[Compatibility](docs/compatibility.md) for exact current boundaries.

## Documentation

- [Documentation Map](docs/) explains authority and status.
- [Quickstart](docs/quickstart.md) builds and runs a small durable project.
- [Language Reference](docs/language/) defines current `.mw` behavior.
- [Tool Reference](docs/tools/) covers commands, project configuration, data
  evolution, and backup/restore.
- [Operations](docs/operations/) covers native-store ownership and recovery.
- [Implementation Map](docs/implementation/) describes the Rust code.
- [Vision](docs/vision.md) describes the long-term architecture.
- [Project Status](docs/status.md) distinguishes current, legacy, and future
  behavior.
- [Security Policy](SECURITY.md) explains private vulnerability reporting and
  the current support boundary.
- [Contributing](CONTRIBUTING.md) gives the local development and verification
  workflow.

Start with [Installation](docs/install.md) and the
[Quickstart](docs/quickstart.md).

## License

Apache-2.0
