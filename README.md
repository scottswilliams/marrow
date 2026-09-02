# Marrow

Marrow is a statically typed compiled language in which durable data is
ordinary program state.

```text
task.status = Status::done
^tasks[id].status = Status::done
```

The first assignment changes a local value. The second changes durable state.
The `^` is the whole difference: both lines resolve `status` through the same
type, and a durable place is read, assigned, and deleted like a local one.

## Example

A module with one durable root and two exports:

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
        if not exists(^tasks[id]) {
            return false
        }
        ^tasks[id].status = Status::done
        return true
    }
}
```

`resource Task` is an ordinary value shape, and `store ^tasks[id: int]: Task`
gives it a durable root keyed by an `int`. `^tasks[id]` is one entry and
`^tasks[id].title` is one field of it. Every durable write sits inside a
`transaction`; when the block ends, its writes commit together.
`exists(^tasks[id])` tests presence, so `complete` returns `false` for an
absent entry. The caller passes the entry identity as an `Id(^tasks)`, the
identity type of that root.

`marrow test` runs a project's tests against a fresh in-memory store, one store
per test. Running an export against a store on disk needs the companion layout
described in [Installation](docs/install.md#running-against-a-store).

## Why

Durable data differs from local data in five ways, and Marrow keeps each one
visible in the source. A read may find nothing, so a durable read yields an
optional such as `string?` and the program says what happens when the value is
absent. A collection may be larger than memory, so a loop over a durable root
states its bound with `at most N` and its overflow behavior with `on more`.
Related writes belong together, so they share one `transaction` block and
commit as one. A new program meets data the previous program wrote, so a store
checks the program's durable shape before it opens. Running code needs authority
over the places it touches, so `marrow check` reports the durable places each
export reads and writes.

Data is navigated, not queried. A program reads or changes one durable element
by its path and walks an explicit subtree with an ordinary loop. No mapping
layer, serializer, or repository stands between the code and the data, and a
program that uses no durable data needs no store.

Transparent persistence, as in an object database, hides the commit, the disk
walk, and the data format. Marrow spells out all three: `transaction` marks the
commit, a bounded `for` marks the walk, and `resource` declares the shape.

## Status

Marrow is unreleased; today, keyed durable roots, transactions, bounded
traversal, indexes, and durable tests run end to end. Packages, schema
evolution, and path authority are future work ([status](docs/status.md)).

## Documentation

- [Installation](docs/install.md) builds `marrow` from source.
- [Quickstart](docs/quickstart.md) goes from `marrow init` to a durable program.
- [Walkthrough](docs/walkthrough.md) reads one durable application line by line.
- [Language reference](docs/language/) defines current `.mw` behavior.
- [Tool reference](docs/tools/) covers `marrow` and the `marrow-lsp` editor server.
- [Operations](docs/operations/) covers a store on disk.
- [Project status](docs/status.md) lists what is implemented.
- [Vision](docs/vision.md) states the product direction.
- [Contributing](CONTRIBUTING.md) gives the checks and the documentation rules.
- [Security policy](SECURITY.md) gives the reporting channel.

## License

Apache-2.0
