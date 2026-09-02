# Quickstart

Two programs, run from the terminal: one without a store, then one that keeps
notes in a durable place. [Install](install.md) `marrow` first; `marrow
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

Replace `src/main.mw` with a store of notes:

```mw
resource Note {
    required text: string
    pinned: bool
}

store ^notes[id: int]: Note

pub fn add(id: int, text: string): bool {
    transaction {
        if exists(^notes[id]) {
            return false
        }
        ^notes[id].text = text
    }
    return true
}

pub fn pin(id: int): bool {
    transaction {
        place slot = ^notes[id]
        if not exists(slot) {
            return false
        }
        slot.pinned = true
    }
    return true
}

pub fn textOf(id: int): string? {
    return ^notes[id].text
}

test "add and read back" {
    assert add(1, "first note")
    assert textOf(1) ?? "" == "first note"
    assert not add(1, "duplicate")
}
```

`resource Note` declares the shape of a stored value: `text` is required and
`pinned` is sparse. `store ^notes[id: int]: Note` declares a durable root keyed
by an `int`; `^notes[id]` is one entry. Every durable write sits inside a
`transaction` block, and `exists(^notes[id])` inside the block tests presence
before `add` writes. `place slot = ^notes[id]` in `pin` names the entry once;
`exists(slot)` proves it is present, and `slot.pinned = true` writes one field.
`textOf` returns `string?` because the entry may be absent, and `??` supplies a
default. The test drives the exports and checks the round trip against a fresh
in-memory store.

## Minting identities

Each durable declaration gets a stable identity, recorded in `.marrow/ids`.
The first `marrow run` writes that file; commit it with the source. Until it
exists, `marrow check` and `marrow test` report `check.durable_identity`. Run
any export once to create it:

```sh
marrow run add -- 1 x
```

```text
cli.durable_unsupported
```

The run writes `.marrow/ids` and then stops with `cli.durable_unsupported`:
`add` needs a store and none was given. `marrow check .` is now clean, and
`--demand` lists the places each export reads and writes:

```sh
marrow check --demand .
```

```text
main.add reads ^notes; writes ^notes.text
main.pin reads ^notes; writes ^notes.pinned
main.textOf reads ^notes.text
```

```sh
marrow test
```

```text
ok    add and read back
1 passed, 0 failed, 0 errored (1/1 selected)
```

A test that reads or writes durable data runs against a store that exists only
for that test. `marrow test` starts from an empty store every run.

## Running against a store

To keep data between runs, an export runs against a store on disk with
`marrow run <export> --store <dir>`. `marrow import` creates the store and fills
it from a file of one JSON object per line, each member a scalar named for a
key or a field of the root. Both commands need the companion layout described
under [Install](install.md#running-against-a-store).

```sh
printf '{"id": 1, "text": "imported note"}\n{"id": 2, "text": "second"}\n' > seed.jsonl
marrow import --store ./store --jsonl seed.jsonl --root notes --keys id
```

```text
provisioned a fresh store at ./store
{"batches_committed":1,"rows_imported":2}
```

The store now holds the two notes. Later runs read and write the same data:

```sh
marrow run textOf --store ./store -- 1      # imported note
marrow run add --store ./store -- 3 "added via run"   # true
marrow run textOf --store ./store -- 3      # added via run
```

`marrow run --store` reads `.marrow/ids` and leaves it unchanged; a missing
identity is reported as `check.durable_identity`.

## Where next

The [walkthrough](walkthrough.md) reads a complete durable application line by
line. The [language reference](language/) defines current `.mw` behavior;
[durable places](language/durable-places.md) is the chapter to start with. The
[CLI reference](tools/cli.md) documents every command, and
[status](status.md) separates current from future work.
