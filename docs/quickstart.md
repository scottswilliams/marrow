# Quickstart

This tutorial builds the current source, creates a native-store project, checks
and runs it, passes an entry argument, runs its test, and inspects its durable
data.

## Build Marrow

Marrow is unreleased, so build the same source revision as this documentation:

```sh
git clone https://github.com/scottswilliams/marrow marrow-source
cd marrow-source
cargo install --locked --path crates/marrow
cd ..
marrow --version
```

The current build requires Rust 1.89 on Linux or macOS. See
[Install From Source](install.md) for the support and distribution boundary.

## Create The Project

```sh
marrow init shelf
cd shelf
```

`init` creates `marrow.json`, one library module, and one test file. Its target
must not already exist, and the final path component must be a valid Marrow
module identifier.

The generated `marrow.json` is:

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::books::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests"]
}
```

The native backend is required because this project declares durable data. The
data directory is relative to the project root. See the
[`marrow.json` reference](tools/project-file.md) for validation and defaults.

## Read The Generated Module

`src/shelf/books.mw` contains one complete module:

```mw
module shelf::books

resource Book
    required title: string
    required author: string
    required shelf: string
    loanedTo: string

store ^books(id: int): Book
    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string): Id(^books)
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    const id: Id(^books) = nextId(^books)
    ^books(id) = book
    return id

pub fn listShelf(shelf: string)
    for id, book in ^books.byShelf(shelf)
        print($"{id}: {book.title} by {book.author}")

pub fn main()
    add(title: "Small Gods", author: "Terry Pratchett", shelf: "fiction")
    add(title: "Sourcery", author: "Terry Pratchett", shelf: "fiction")
    listShelf("fiction")
```

The resource defines the reusable `Book` shape. The store saves that shape at
`^books`, keyed by `int`. The assignment to `^books(id)` writes durable data;
the declared index is maintained with the managed write. The
[Language Reference](language/) defines these constructs.

## Check And Run

Check the project without changing its source or store:

```sh
marrow check .
```

```text
ok: . checked
```

Run the configured default entry:

```sh
marrow run .
```

```text
1: Small Gods by Terry Pratchett
2: Sourcery by Terry Pratchett
```

The native store and `marrow.lock` are established by this write-capable run.
Program output from `print` goes to stdout.

To invoke another public entry, name it and pass each parameter as
`--arg name=value`. This call reads the two saved books without adding more:

```sh
marrow run --entry shelf::books::listShelf --arg shelf=fiction .
```

```text
1: Small Gods by Terry Pratchett
2: Sourcery by Terry Pratchett
```

Argument values are decoded against the checked entry signature. Repeat
`--arg` only when supplying a supported sequence parameter. See the
[CLI Reference](tools/cli.md) for the supported run modes.

## Run The Test

The generated `tests/books_test.mw` is shown as text because it is one file in
the complete generated project and imports the library module above:

```text
module tests::books_test

use shelf::books

pub fn addThenFind()
    const id = books::add(title: "Mort", author: "Terry Pratchett", shelf: "fiction")
    std::assert::isTrue(exists(^books(id)))
    if const title = ^books(id).title
        std::assert::isTrue(title == "Mort")
    else
        std::assert::isTrue(false)
```

Run every zero-argument public function selected by the configured test paths:

```sh
marrow test .
```

```text
ok    tests::books_test::addThenFind

1 test: 1 passed, 0 failed, 0 errored
```

Each test runs against a fresh in-memory store and does not modify the native
project store.

## Inspect The Durable Data

List the saved roots and count the stored entities and values:

```sh
marrow data roots .
marrow data stats .
```

```text
^books
roots: 1
records: 2
cells: 6
```

Read one exact path:

```sh
marrow data get . '^books(1).title'
```

```text
"Small Gods"
```

Finally, verify the stored values and required fields against the checked
project:

```sh
marrow data integrity .
```

```text
ok: . integrity verified (6 cells)
```

The inspection commands are read-only. `records` counts saved entities such as
`^books(1)`; `cells` counts stored path/value pairs. Continue with the
[Data Tools Reference](tools/data.md), [Evolution Tools](tools/evolution.md), or
[Backup And Restore](tools/backup-and-restore.md).
