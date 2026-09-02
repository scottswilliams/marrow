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
durable place, and the copy is still there after `record` returns. The `^` is the
whole difference; the type checker resolves `done` the same way on both lines.
`isDone` reads the field as `bool?` and supplies a default, because the entry may
be absent. The test runs against a fresh in-memory store.

## Durable data as language data

Durable data differs from local data in five ways, and the language shows each
difference where it occurs.

A read can find nothing. A durable read is optional, and the program says what
happens when the entry or the field is absent: `??` supplies a default, `if const`
proves presence, and `exists` asks directly.

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

## One language, no layers

Data is navigated, not queried. A program reads or changes one durable element by
its path and walks a subtree with an ordinary loop, the same way it works with
local state. The `resource` declaration is the only description of the data. The
compiler knows the program's types, durable places, and effects, and it reports
them; no schema file, serializer, or access layer repeats them. Compiling opens
no store; attaching a compiled program to a store is a separate step.

Object databases made persistence transparent and hid the commit, the disk walk,
and the data format. Marrow spells out all three: `transaction` marks the commit,
a bounded `for` marks the traversal, and data moves in its declared shape. A
durable program reads like a local one, and each extra word marks a real
difference: presence, atomicity, bounded work, failure, or authority.

A storage engine supplies ordered bytes, snapshots, atomic commits, and recovery
behind a private boundary. It defines none of the language's types, paths, or
effects, and the choice of engine is not a language feature.

Compile and test time is a design constraint of the language.
[Compilation and test speed](implementation/speed.md) states the rules that
follow from it.

## What Marrow is not

Marrow is not a query language: a program reaches an entry by its path and walks
a subtree with a bounded loop. It is not an ORM: no mapping layer stands between
a value and the place it lives. It is not a relational or document system in
disguise: durable data is a hierarchy of places, and an index is a second path to
the same entries. It is not a UI or service framework: HTTP, TLS, identity
providers, and UI toolkits integrate through host boundaries when a program needs
them.

## Stages

Marrow is built in three stages that share one language and one durable model.
The first is a storeless command-line program, the second a local application
with its own store, and the third a small served system for a few terminals
sharing one store. Each stage adds deployment semantics and rewrites nothing in
the program. Today, a storeless program runs from a source install, and a
store on disk runs with the
[companion layout](install.md#running-against-a-store); a distributable
[local application](future/local-applications.md) and a
[served system](future/served-execution.md) are future work ([status](status.md)).

## Lineage

MUMPS demonstrates that direct hierarchical durable state can support important
long-lived transactional systems. It is evidence and inspiration, and it is not a
compatibility target: Marrow inherits none of that language's syntax, dynamic
typing, or schema-by-convention. Hierarchical and orthogonal persistence, effect
systems, content-addressed code, language-integrated databases, and local
application runtimes all have prior art; the parts are old, and the combination
is what Marrow tests, with working programs.
