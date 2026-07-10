# Marrow Language Reference

This directory is the normative reference for the currently implemented Marrow
language. It describes current `.mw` syntax and semantics, not the complete
long-term design.

On the main branch, “current” means the implementation at the same Git revision
as these files. A release snapshot must identify its release and source
revision.

Marrow is statically typed. The current runtime executes a checked program with
a tree-walking interpreter; it does not emit bytecode or native machine code.
See [Project Status](../status.md) for the boundary between current behavior and
architectural direction.

## Central Model

A resource declaration defines typed tree members. Those member types and the
ordinary path syntax are reused for local values and durable places.

```mw
var task: Task
task.status = Status::done

^tasks(id).status = Status::done
```

`task.status` is local. `^tasks(id).status` is durable. Both places resolve the
same `status` member and type. Durable places additionally have structural
presence, keyed-child materialization, transaction, and storage rules; local
and durable values are not observationally identical.

Durable state is accessed with ordinary expressions and statements:

- read a path;
- assign a value;
- test whether a node exists;
- delete a path or subtree;
- iterate a keyed layer with `for`;
- group several changes in a `transaction`.

These operations use the same expression and statement language as local work.

## Core Terms

| Term | Meaning |
|---|---|
| Resource | A named typed tree shape |
| Local value | A value that is not part of durable state |
| Durable place | A `^` path in the project's durable tree |
| Store declaration | A declaration attaching resource members to a durable root |
| Keyed layer | A repeatable tree layer addressed by typed keys |
| Entry identity | A nominal `Id(^root)` value naming one store entry |
| Presence | Whether a tree node exists independently of a field value |
| Transaction | A lexical block whose durable changes commit or roll back together |
| Index | A declared alternate tree maintained with its owning store |

Source spelling, accepted declaration identity, entry identity, public text,
and physical backend encoding are distinct concepts. Application source never
constructs physical backend keys.

## First Look

```mw
module app::tasks

enum Status
    open
    done

resource Task
    required title: string
    required status: Status
    note: string

    history(changedAt: instant)
        required status: Status

store ^tasks(id: int): Task
    index byStatus(status, id)

pub fn add(title: string): Id(^tasks)
    var task: Task
    task.title = title
    task.status = Status::open

    const id: Id(^tasks) = nextId(^tasks)
    ^tasks(id) = task
    return id

pub fn complete(id: Id(^tasks), changedAt: instant): bool
    if not exists(^tasks(id))
        return false

    transaction
        ^tasks(id).status = Status::done
        ^tasks(id).history(changedAt).status = Status::done

    return true

pub fn printOpen()
    for id in ^tasks.byStatus(Status::open)
        if const title = ^tasks(id).title
            print($"{id}: {title}")
```

This example illustrates the main rules:

- `resource Task` defines a tree shape.
- Fields are sparse unless marked `required`; `note` may be absent.
- `history(changedAt: instant)` is a keyed child layer.
- `store ^tasks(id: int): Task` declares a durable root.
- `Id(^tasks)` is the nominal entry identity type for that root.
- `index byStatus(status, id)` declares an alternate tree maintained with the
  base data.
- `transaction` makes the status and history changes one atomic unit.
- Iteration uses an ordinary `for` loop over stored keys.

The guarded `title` read reflects the current presence model: `required` is a
validity rule for populated data, not a static proof that every durable read is
present. `nextId` proposes an entry identity from the current tree and does not
reserve it before the following write.

## Trees And Presence

Resource indentation defines tree layers:

```mw
resource Patient
    required name: string

    address
        city: string
        postalCode: string

    visits(visitDate: date)
        required provider: string
        note: string
```

An unmarked field is sparse: it may have a typed value or be absent. Absence is
not a stored `null`. Reads of maybe-present places have type `T?` and must be
resolved explicitly with `??`, `if const`, `exists`, or optional chaining.

```mw
const note: string = ^patients(id).visits(day).note ?? ""

if const note = ^patients(id).visits(day).note
    print(note)
```

A resource node may exist even when it has no populated fields. `exists(path)`
tests structural presence rather than guessing from a required field.

See [Types](types.md) and
[Resources and Saved Data](resources-and-storage.md) for the complete rules.

## Durable Paths

A durable path begins at a declared `^root` and continues through typed key
and member segments:

```mw
^patients(patientId).visits(visitDate).note
```

The checker resolves the root, key types, resource members, value type, and
presence of the place. Keyed roots, child layers, and indexes are traversed
directly with `for`. Indexes are declared trees owned by a store; maintaining
them is part of the managed write.

Entry identities are nominal. An `Id(^patients)` is not interchangeable with an
entry identity from another root, even if both roots use the same key type or
resource shape.

## Functions And Effects

Marrow has one function form:

```mw
fn rename(id: Id(^patients), name: string)
    ^patients(id).name = name
```

A function may compute local values, access durable places, call other
functions, throw structured errors, or use host capabilities. There is no
separate function category for durable work.

The checker records typed semantic facts about calls and effects for
diagnostics, tooling, and runtime execution. See
[Modules and Functions](modules-functions.md) and
[Control Flow and Effects](control-flow-and-effects.md).

## Transactions

A managed assignment performs one durable change. Use `transaction` when
several durable reads and writes form one atomic unit:

```mw
transaction
    ^accounts(from).balance = fromBalance - amount
    ^accounts(to).balance = toBalance + amount
```

If the block fails, none of its buffered durable writes commit. Reads within
the transaction observe its preceding writes according to the transaction
rules. Host effects remain outside the durable commit guarantee.

The complete nesting, rollback, and error behavior is defined in
[Resources and Saved Data](resources-and-storage.md).

## Changes To Populated Data

Durable declarations have identities beyond their current source spelling.
Adding a required field, removing populated data, changing a type, or renaming
a member may require an explicit evolution.

For supported changes, Marrow can:

1. compare the proposed checked program with the store's active schema;
2. preview its obligations against the current populated state;
3. produce a witness bound to that state;
4. apply the change only while the witness still matches.

This mechanism does not make every source edit automatically safe. It makes the
required transition explicit and rejects unsupported or stale transitions.

See [Data Evolution](../data-evolution.md).

## Scope

The hierarchical model is intended for operational state addressed by stable
typed paths and explicitly declared indexes. Specialized analytical and
retrieval workloads integrate at typed boundaries. Physical storage keys and
backend-specific APIs are not application-language semantics.

## Reference Map

- [Syntax](syntax.md) — declarations, statements, expressions, operators,
  literals, and punctuation.
- [Types](types.md) — primitives, resources, optional values, identities,
  sequences, keyed trees, and conversion.
- [Enums](enums.md) — nominal enums, hierarchical members, matching, and each
  member's accepted declaration identity.
- [Resources and Saved Data](resources-and-storage.md) — resources, stores,
  keyed layers, indexes, presence, reads, writes, deletion, traversal, and
  transactions.
- [Modules and Functions](modules-functions.md) — modules, imports, visibility,
  parameters, returns, calls, and name resolution.
- [Control Flow and Effects](control-flow-and-effects.md) — conditionals, loops,
  matching, short-circuiting, errors, and effects.
- [Builtins](builtins.md) — always-available language operations.
- [Standard Library](standard-library.md) — `std::` modules and host-capability
  boundaries.
- [Reference Sample](sample.md) — one compact complete project.
- [Formal Grammar](grammar.md) — EBNF-style grammar.

The current storage-accounting contract in [Cost Model](cost-model.md) and the
current `surface` declarations documented elsewhere in this reference remain
implemented behavior under architectural reconsideration. They are not part of
the long-term design in [Vision](../vision.md) and will be removed from the
reference only with their implementation replacements.

## Reference Status

Future architecture belongs in [Vision](../vision.md), with implementation
status recorded in [Project Status](../status.md). Examples in this directory
should parse and check under the current toolchain. When implementation and
reference disagree, record and fix the discrepancy rather than introducing a
second informal rule.
