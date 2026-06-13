# Quickstart

Create a Marrow project, inspect the generated resource and test, run it, and
inspect the saved data.

If you do not have the `marrow` binary yet, see [Install](install.md).

## 1. Create The Project

Start with the quickstart scaffold:

```sh
marrow init shelf
cd shelf
```

This creates a Marrow project directory with `marrow.json`, one library module,
and one test file. The generated `marrow.json` is:

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::books::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests"]
}
```

- `sourceRoots` lists the directories searched for `.mw` library modules.
- `run.defaultEntry` is the public function `marrow run` calls when you do not
  pass `--entry`; qualify it as `module::function` unless the bare name is
  unique.
- `store` selects where saved data lives. `native` is the persistent on-disk
  store and requires a `dataDir`. This project declares saved data, so it needs
  one: without a native store, `marrow run` refuses with
  `run.durable_store_required`. (Tests always run in-memory.)
- `tests` lists plain paths to test files or test directories.

A module's name must match its path under the source root. Because the file
below is `src/shelf/books.mw`, it declares `module shelf::books`. A file at
`src/books.mw` would have to declare `module books`. See
[Project Configuration](project-config.md) for every field.

## 2. Inspect The Resource And Function

The generated `src/shelf/books.mw` is:

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

What this declares:

- `resource Book` declares the typed tree shape. `store ^books(id: int): Book`
  saves that shape under `^books`, keyed by an `int` identity whose type is
  `Id(^books)`.
- `required` fields must be present; sparse fields like `loanedTo` may be
  absent.
- `index byShelf(shelf, id)` belongs to the store and declares an alternate
  lookup tree. Assigning a `Book` maintains the index in the same managed write.
- `nextId(^books)` allocates the next identity for the root.
- `^books(id) = book` saves the local `book` value under that identity. The `^`
  is what makes data saved rather than local to the run.

For the full data model — required and sparse fields, keyed child layers,
history, sequences, transactions — see [Data Modeling](data-modeling.md) and
the [Language Reference](language/).

## 3. Check And Run It

Check the project before running. Checking parses and type-checks every module:

```sh
marrow check .
```

```text
ok: . checked
```

Run the default entry (`shelf::books::main`):

```sh
marrow run .
```

```text
1: Small Gods by Terry Pratchett
2: Sourcery by Terry Pratchett
```

`marrow run` checks the project first, then runs the entry against the store
`marrow.json` selects. Output from `print` goes to stdout. Use
`--entry <entry>` to call a different no-argument public function instead of the
default; qualify it as `module::function` unless the bare function name is
unique.

This project selects the `native` store, so the data persists. Run it again and
the new books appear alongside the first two:

```sh
marrow run .
```

```text
1: Small Gods by Terry Pratchett
2: Sourcery by Terry Pratchett
3: Small Gods by Terry Pratchett
4: Sourcery by Terry Pratchett
```

(If `marrow.json` had no `store`, the run would refuse with
`run.durable_store_required`: a program that declares saved data requires a
native store.)

## 4. Inspect The Saved Data

`marrow data` reads a project's store without modifying it. List the saved
roots:

```sh
marrow data roots .
```

```text
^books
```

Count roots and records:

```sh
marrow data stats .
```

After the two runs above, four books are saved, each with three populated
fields:

```text
roots: 1
records: 12
```

Dump every saved field path and value:

```sh
marrow data dump .
```

```text
^books(1).title	Small Gods
^books(1).author	Terry Pratchett
^books(1).shelf	fiction
^books(2).title	Sourcery
^books(2).author	Terry Pratchett
^books(2).shelf	fiction
^books(3).title	Small Gods
^books(3).author	Terry Pratchett
^books(3).shelf	fiction
^books(4).title	Sourcery
^books(4).author	Terry Pratchett
^books(4).shelf	fiction
```

`data dump` reports stored field values, not the generated index entries, which
are derived data. Read a single path:

```sh
marrow data get . '^books(1).title'
```

```text
Small Gods
```

Verify that every stored value decodes against the schema:

```sh
marrow data integrity .
```

```text
ok: . integrity verified (12 records)
```

Every `marrow data` subcommand also takes `--format text|json|jsonl` for
tooling:

```sh
marrow data stats --format json .
```

```text
{"project":".","records":12,"roots":1}
```

`marrow data` inspection commands are read-only; `marrow data recover` is the
explicit store-open repair command. The `diff` and `load` subcommands are
deferred — see [future/data-tools.md](future/data-tools.md).

## 5. Inspect And Run The Test

A test file is any `.mw` file selected by the `tests` paths. `marrow test`
runs every `pub fn` with no parameters in those files as a test; functions with
parameters are helpers. Each test runs against a fresh in-memory store, so tests
never touch saved data and never depend on each other.

The generated `tests/books_test.mw` is:

```mw
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

`use shelf::books` imports the module so you can call `books::add`. Write
equality assertions by passing a boolean to `std::assert::isTrue` — `==` is the
equality operator (`=` is assignment). Run the tests:

```sh
marrow test .
```

```text
ok    tests::books_test::addThenFind

1 test: 1 passed, 0 failed, 0 errored
```

A failed `std::assert::*` is reported as a located test failure. The command
exits non-zero if any test fails.

## Exit Codes

The CLI uses three exit codes: `0` success, `1` a check/runtime/storage/project
failure, `2` a command-line usage error. See [Errors](error-codes.md) for the
full contract and the machine-readable error envelope.

## Next Steps

- [CLI Reference](cli.md) — every command, flag, and output format.
- [Project Configuration](project-config.md) — every `marrow.json` field.
- [Data Modeling](data-modeling.md) — resources, identity, indexes, and history.
- [Language Reference](language/) — the full `.mw` language.
