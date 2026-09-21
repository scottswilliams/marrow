# Quickstart

Two programs, run from the terminal: one without a store, then one that keeps
books in a durable place. [Install](install.md) `marrow` first; `marrow
--version` prints `marrow 0.1.0`.

## Create a project

```sh
marrow init hello
cd hello
```

`marrow init` writes a [project](tools/projects.md): a `marrow.toml` manifest
and a `src/main.mw` starter script.

```text
hello/
  marrow.toml      edition = "2026"
  src/main.mw      a pub fn main() starter script
```

A file's name comes from its path: `src/main.mw` is `main`, and
`src/shelf/books.mw` is `shelf.books`. A file with no `module` header is a
script; its exported functions are still addressable from the command line.

## A first program

Replace `src/main.mw` with a function and a test. This program touches no
durable data, so it needs no store. Check, run, and test it:

```mw
pub fn greet(name: string): string {
    return $"Hello, {name}!"
}

test "greet names the caller" {
    assert greet("world") == "Hello, world!"
}
```

```sh
marrow check .
```

```text
1 export across 1 module

main: 1 export, all storeless
```

`marrow check` type-checks the project and reports, per module, which durable
places its exported functions read and write. `greet` touches none.

```sh
marrow run greet -- world
```

```text
Hello, world!
```

`marrow run` compiles and verifies the project, then runs the named export.
Arguments after `--` are decoded against the export's scalar parameters.

```sh
marrow test
```

```text
ok    greet names the caller
1 passed, 0 failed, 0 errored (1/1 selected)
```

`marrow test` runs every `test` declaration and reports each outcome by name.

## A durable program

Replace `src/main.mw` with a store of books:

```mw
resource Book {
    required title: string
    read: bool
}

store ^books[id: int]: Book

pub fn add(id: int, title: string): bool {
    transaction {
        if exists(^books[id]) {
            return false
        }
        ^books[id] = Book(title: title)
    }
    return true
}

pub fn finish(id: int): bool {
    transaction {
        place book = ^books[id]
        if not exists(book) {
            return false
        }
        book.read = true
    }
    return true
}

pub fn titleOf(id: int): string? {
    return ^books[id].title
}

test "add and read back" {
    assert add(1, "Small Gods")
    assert titleOf(1) ?? "" == "Small Gods"
    assert not add(1, "Pyramids")
}
```

`resource Book` declares the shape of a stored value: `title` is required and
`read` is sparse. `store ^books[id: int]: Book` declares a durable root keyed
by an `int`; `^books[id]` is one entry. Every durable write sits inside a
`transaction` block, and `exists(^books[id])` inside the block tests presence
before `add` writes. `^books[id] = Book(title: title)` creates the entry as a
whole: the constructor names every required field, so a present entry is
complete from its first commit. `place book = ^books[id]` in `finish` names the
entry once; the guard on `exists(book)` returns when it is absent, which proves
it present for the rest of the block, and `book.read = true` updates one
field of the present entry. A field write never creates an entry.
`titleOf` returns `string?` because the entry may be absent, and `??` supplies a
default. The test drives the exports and checks the round trip against a fresh
in-memory store.

## Minting identities

Each durable declaration gets a stable identity, recorded in `.marrow/ids`.
The first storeless `marrow run` writes that file; commit it with the source.
Until it exists, `marrow check` and `marrow test` report
`check.durable_identity`. Run any export once to create it:

```sh
marrow run add -- 1 x
```

```text
cli.durable_unsupported
```

The run writes `.marrow/ids` and then stops with `cli.durable_unsupported`:
`add` needs a store and none was given.

A project whose durable declarations are used only by tests has no export to
run. `marrow run` mints the ids before it looks the export up, so any name
works: `marrow run mint` writes `.marrow/ids` and then reports that no such
export exists.

`marrow check .` is now clean and names every place each export reads and
writes:

```sh
marrow check .
```

```text
3 exports across 1 module

main: 3 exports
  add
    reads ^books
    writes ^books
  finish
    reads ^books
    writes ^books.read
  titleOf
    reads ^books.title
```

```sh
marrow test
```

```text
ok    add and read back
1 passed, 0 failed, 0 errored (1/1 selected)
```

A test that reads or writes durable data runs against a store that exists only
for that test.

## Running against a store

To keep data between runs, an export runs against a store on disk with
`marrow run <export> --store <dir>`. `marrow import` creates the store and
fills it from a file of one JSON object per line, each member a scalar named
for a key or a field of the root. Both commands need the companion layout
described under [Install](install.md#running-against-a-store); a source install
carries the `marrow` command alone, so the transcripts below come from an
installation that has it.

```sh
printf '{"id": 1, "title": "Small Gods"}\n{"id": 2, "title": "Pyramids"}\n' > seed.jsonl
marrow import --store ./store --jsonl seed.jsonl --root books --keys id
```

```text
provisioned a fresh store at ./store
{"batches_committed":1,"rows_imported":2}
```

The store now holds the two books. Later runs read and write the same data:

```sh
marrow run titleOf --store ./store -- 1
marrow run add --store ./store -- 3 Mort
marrow run titleOf --store ./store -- 3
```

```text
Small Gods
true
Mort
```

`marrow run --store` reads `.marrow/ids` and leaves it unchanged; a missing
identity is reported as `check.durable_identity`.

## Using a local library

Source that several projects share lives in its own project directory, and a
consumer names it in `marrow.toml` under an alias it chooses:

```toml
edition = "2026"

[dependencies]
graphtext = { path = "../graphtext" }
```

The alias roots every module the library contributes, so a library file
`src/text.mw` whose own header reads `module text` is `graphtext::text` here,
and a type it declares is written `graphtext::Pair`:

```text
use graphtext::text

pub fn key(line: string): string {
    const pair: graphtext::Pair = text::parsePair(line)
    return pair.key
}
```

A dependency is read, never written:
`marrow fmt` leaves its files alone, its identities come from its own
`.marrow/ids`, and `marrow run` and `marrow test` act on the project they are
invoked on, so the library's exports and tests run in the library's directory.
[Graph Report](../fixtures/v01/conformance/graph_report) and its
[text library](../fixtures/v01/conformance/graph_report_lib) are a worked pair.
[Projects](tools/projects.md#dependencies) states the manifest rules and
[modules and functions](language/modules-and-functions.md#dependencies) the
naming.

## Where next

The [walkthrough](walkthrough.md) reads a complete durable application line by
line. The [language reference](language/README.md) defines current `.mw`
behavior and states the order its chapters are meant to be read in. The
[CLI reference](tools/cli.md) documents every command, and
[status](status.md) separates current from future work.
