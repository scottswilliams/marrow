# Marrow

Marrow is a statically typed compiled language in which durable data is
ordinary program state.

```text
book = Book(title: title, read: true)
^books[id] = Book(title: title, read: true)
```

The first assignment changes a local value. The second changes durable state.
The `^` is the whole difference: both lines build the same `Book`, and a
durable place is read and assigned like a local one.

## Example

A module with one durable root and two exported functions:

```mw
module app::books

resource Book {
    required title: string
    read: bool
}

store ^books[id: int]: Book

pub fn add(id: Id(^books), title: string): Id(^books) {
    transaction {
        ^books[id] = Book(title: title)
    }
    return id
}

pub fn finish(id: Id(^books)): bool {
    transaction {
        place book = ^books[id]
        if not exists(book) {
            return false
        }
        book.read = true
        return true
    }
}
```

`resource Book` is an ordinary value shape, and `store ^books[id: int]: Book`
gives it a durable root keyed by an `int`. `^books[id]` is one entry and
`^books[id].title` is one field of it. Every durable write sits inside a
`transaction`; when the block ends, its writes commit together. `add` writes
the entry whole, so it is complete from its first commit. `place book =
^books[id]` names the entry once, and `exists(book)` proves it present, so
`finish` returns `false` for an absent entry and updates `read` only on a
present one. The caller passes the entry identity as an `Id(^books)`, the
identity type of that root; `Id(^books, 7)` builds one from a key, so a caller
writes `add(Id(^books, 7), "Small Gods")`.

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

## Status

Marrow is unreleased. Today keyed durable roots, transactions, bounded
traversal, indexes, and durable tests run end to end, a project may name
local-path dependencies, and `marrow apply` adds a sparse scalar field to a
populated store. Remote package acquisition, schema evolution past that, and
path authority are future work.

The [beta scope](docs/vision.md#beta-scope) is a useful storeless program and a
recoverable local application on one machine; that is a scope decision, not a
readiness claim, and [status](docs/status.md) lists what is still missing.

## Documentation

- [Installation](docs/install.md) builds the toolchain from source: one script
  stages `marrow`, its companion runner, `marrow-lsp`, and the release manifest
  into the directory you put on `PATH`.
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
