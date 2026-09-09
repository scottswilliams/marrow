# Vision

Marrow is a statically typed compiled language in which durable data is ordinary
program state. A program declares the shape of its data once and reads and
writes a durable place the way it reads and writes a local value. Where work is
atomic, bounded, or able to fail, the program says so. The aim is a
general-purpose language whose programs keep that shape from a command-line tool
to a served system.

A local value and a durable place are written the same way:

```mw
module docs::vision::tasks

resource Task {
    required title: string
    done: bool
}

store ^tasks[id: int]: Task

pub fn record(id: int, title: string, done: bool) {
    var task = Task(title: title)
    task.done = done
    transaction {
        ^tasks[id] = task
    }
}

pub fn isDone(id: int): bool {
    return ^tasks[id].done ?? false
}

test "a durable write outlives the call" {
    record(1, "write docs", true)
    assert isDone(1)
    assert not isDone(2)
}
```

`task.done = done` changes a local value. `^tasks[id] = task` copies it to a
durable place, and the copy is still there after `record` returns. Both use the
declared resource type; the durable write also requires a transaction.
`isDone` reads the field as `bool?` and supplies a default, because the entry may
be absent. The test runs against a fresh in-memory store.

## Durable data as language data

Durable data differs from local data in five ways, and the language shows each
difference where it occurs.

A read can find nothing. Untested and sparse durable reads are optional, and the
program handles absence explicitly. Required fields read through a proved named
place have their declared types ([durable places](language/durable-places.md#named-places)).

The data can be larger than memory. A loop over a root, a branch, or an index
says how many keys it visits with `at most N` and what to do when more remain
with `on more`. Larger work is repeated bounded batches.

Related writes commit together. A mutating export owns one `transaction` block,
and every durable write sits inside it. When the block ends, its writes commit as
one change. If it faults, none of them apply, and the report names the
[durable outcome](language/errors-and-transactions.md#interrupted-invocations).

A code change meets stored data. A change to a function body reopens an existing
store and keeps every value in it. A change to a durable declaration or to the
exported interface is refused, and the
[prior program stays usable](operations/README.md#changing-the-program).
Evolving stored data under new declarations is described under
[durable programming](future/durable-programming.md).

Running code needs authority. `marrow check --demand` lists the durable places
each export reads and writes. That demand describes; it grants nothing. Attaching
deployment authority to the same paths is described under
[path effects and authority](future/path-effects-and-authority.md).

## Design constraints

Data is navigated, not queried. A program reads or changes one durable element by
its path and walks a subtree with an ordinary loop, the same way it works with
local state. The `resource` declaration is the only description of the data. The
compiler knows the program's types, durable places, and effects, and it reports
them; no schema file, serializer, or access layer repeats them. Compiling opens
no store; attaching a compiled program to a store is a separate step.

A storage engine supplies ordered bytes, snapshots, atomic commits, and recovery
behind a private boundary. It defines none of the language's types, paths, or
effects, and the choice of engine is not a language feature.

Compile and test time is a design constraint of the language.
[Compilation and test speed](implementation/speed.md) states the rules that
follow from it.

The compiler owns resolved types, paths and effects. The runtime consumes a
verified image; editor tools and clients consume published facts. A feature
must have a maintained caller or establish a necessary correctness or resource
bound. It must not add a second semantic model, repeated source analysis or an
application mapping layer. Existing abstractions are subject to the same test.

## Beta scope

The beta target is a useful storeless program and a recoverable local
application on one machine. This is a scope decision, not a readiness claim;
[status](status.md) records the substantial work still missing.

| Experience | Required outcome |
|---|---|
| Ordinary programming | Values, functions, generics and collections compose consistently; source reuse, bounded text I/O, tests and an accurate editor support a useful storeless program. |
| Direct durable programming | Complete entries, explicit presence proofs, typed identities, managed indexes, bounded traversal and one serial transaction owner remain ordinary language operations. |
| Local application lifetime | The same exports serve a terminal and desktop client; tools provision, update code and add a sparse field on populated data, audit, back up, restore and handle interrupted outcomes without automatic replay. |

Graph Report supplies the small storeless example. The external
`marrow-acceptance` suite maintains Club Locker and EMR with real clients,
source tests and data journeys. A beta needs qualified installed artifacts and
sustained maintained use in addition to passing compiler tests.

Exact local-path source reuse comes before remote acquisition. Closures,
decimal, enum grouping, a standard-library portfolio, automatic presence
inference, general migrations and a second storage engine are deferred. So are
reader overlap, parallel mutation, jobs, public serving and principal policy.
They are not prerequisites hidden in a future page.

The longer-term aim is to retain the same language and durable model in a
[served system](future/served-execution.md). That continuity needs evidence;
pre-release source and stored formats may change. UI, network and identity
services integrate through host boundaries rather than becoming a Marrow
application framework.

## Lineage

MUMPS demonstrates that direct hierarchical durable state can support important
long-lived transactional systems. It is evidence and inspiration, and it is not a
compatibility target: Marrow inherits none of that language's syntax, dynamic
typing, or schema-by-convention. Hierarchical and orthogonal persistence, effect
systems, content-addressed code, language-integrated databases, and local
application runtimes all have prior art; the parts are old, and the combination
is what Marrow tests, with working programs.
