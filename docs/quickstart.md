# Quickstart

Create a Marrow project, write one resource and a public function, run it,
inspect the saved data, and run a test.

If you do not have the `marrow` binary yet, see [Install](install.md).

## 1. Create The Project

A Marrow project is a directory with a `marrow.json` and one or more source
roots holding `.mw` files:

```sh
mkdir -p shelf/src/shelf shelf/tests
cd shelf
```

Write `marrow.json`:

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::books::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests/**/*.mw"]
}
```

- `sourceRoots` lists the directories searched for `.mw` library modules.
- `run.defaultEntry` is the public function `marrow run` calls when you do not
  pass `--entry`; qualify it as `module::function` unless the bare name is
  unique.
- `store` selects where saved data lives. `native` is the persistent on-disk
  store and requires a `dataDir`. Omit `store` entirely to run against a fresh
  in-memory store each time.
- `tests` lists the glob patterns for test files.

A module's name must match its path under the source root. Because the file
below is `src/shelf/books.mw`, it declares `module shelf::books`. A file at
`src/books.mw` would have to declare `module books`. See
[Project Configuration](project-config.md) for every field.

## 2. Write A Resource And A Function

Create `src/shelf/books.mw`:

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
    for id in ^books.byShelf(shelf)
        print($"{id}: {^books(id).title} by {^books(id).author}")

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

(If `marrow.json` had no `store`, each run would start from an empty in-memory
store and always print just the two books.)

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

```text
roots: 1
records: 8
```

Dump every saved path and value (after a single run):

```sh
marrow data dump .
```

```text
^books(1).author	Terry Pratchett
^books(1).shelf	fiction
^books(1).title	Small Gods
^books(2).author	Terry Pratchett
^books(2).shelf	fiction
^books(2).title	Sourcery
^books.byShelf("fiction")(1)	1
^books.byShelf("fiction")(2)	1
```

The `^books.byShelf(...)` rows are the generated index entries. Read a single
path:

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
ok: store integrity verified (8 records)
```

Every `marrow data` subcommand also takes `--format text|json|jsonl` for
tooling:

```sh
marrow data stats --format json .
```

```text
{"project":"/path/to/shelf","records":8,"roots":1}
```

`marrow data` is read-only. The `diff` and `load` subcommands are deferred —
see [future/data-tools.md](future/data-tools.md).

## 5. Write And Run A Test

A test file is any `.mw` file matched by the `tests` patterns. `marrow test`
runs every `pub fn` with no parameters in those files as a test; functions with
parameters are helpers. Each test runs against a fresh in-memory store, so tests
never touch saved data and never depend on each other.

Create `tests/books_test.mw`:

```mw
module tests::books_test

use shelf::books

pub fn addThenFind()
    const id = books::add(title: "Mort", author: "Terry Pratchett", shelf: "fiction")
    std::assert::isTrue(exists(^books(id)))
    std::assert::isTrue(^books(id).title == "Mort")
```

`use shelf::books` imports the module so you can call `books::add`. Write
equality assertions by passing a boolean to `std::assert::isTrue` — `=` is the
equality operator. Run the tests:

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
