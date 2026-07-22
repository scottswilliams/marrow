# Quickstart

This page takes a developer who has not seen Marrow before from a source
checkout to a running durable program. Every command below is run exactly as
shown. Marrow is unreleased and its admitted language subset is narrow and
growing; the [status](status.md) page states what is current.

## Build the command

Marrow builds from source with the pinned Rust toolchain (Rust 1.89) on Linux or
macOS. See [Install from source](install.md) for the requirements.

```sh
git clone https://github.com/scottswilliams/marrow
cd marrow
cargo install --locked --path crates/marrow
marrow --version
```

`marrow --version` prints the package version of the binary. Installing Marrow
starts no service and creates no data directory.

## Create a project

```sh
marrow init hello
cd hello
```

`marrow init` writes a [project](tools/projects.md): a `marrow.toml` manifest and
a `src/main.mw` starter script.

```text
hello/
  marrow.toml      edition = "2026"
  src/main.mw      a pub fn main() starter script
```

Source lives under `src`. A file's name is derived from its path
(`src/main.mw` is `main`, `src/shelf/books.mw` is `shelf.books`), and a file with
no `module` header is a script whose exported functions are still addressable from
the command line.

## A first program without a store

Replace `src/main.mw` with a small function and a test. This program uses no
durable data, so it needs no store.

```mw
pub fn greet(name: string): string {
    return $"Hello, {name}!"
}

test "greet names the caller" {
    assert greet("world") == "Hello, world!"
}
```

Check, run, and test it:

```sh
marrow check .
```

```text
main.greet reads or writes no durable data
```

`marrow check` captures and type-checks the project, then prints one line per
exported (`pub fn`) function describing the durable data it touches — here, none.

```sh
marrow run greet -- world
```

```text
Hello, world!
```

`marrow run` compiles the project to a reproducible program image, an independent
verifier seals that image, and the bytecode VM runs the named export. Arguments
after `--` are decoded against the export's scalar parameters.

```sh
marrow test
```

```text
ok    greet names the caller
1 passed, 0 failed, 0 errored (1/1 selected)
```

`marrow test` runs every `test` declaration through the same compile-and-verify
path.

## A durable program

Durable data is written and read as ordinary typed program state. Replace
`src/main.mw` with a store of notes. A `resource` declares the stored shape; a
`store` declares a durable root keyed by an `int`; each write happens inside an
explicit `transaction`; and `exists` and `?.` make presence visible.

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

A durable declaration has a stable identity recorded in a machine-written ledger
(`.marrow/ids`). The first storeless `marrow run` mints any missing identities and
writes the ledger; commit that file with the source. Mint the identities by
running any export once:

```sh
marrow run add -- 1 x
```

```text
cli.durable_unsupported
```

This first run mints `.marrow/ids` (commit it) and then reports
`cli.durable_unsupported`, because a durable export run without a store has no
store to act on. With the ledger in place, `marrow check` is clean and prints
each export's durable demand:

```sh
marrow check .
```

```text
main.add reads ^notes; writes ^notes.text
main.pin reads ^notes; writes ^notes.pinned
main.textOf reads ^notes.text
```

The durable model runs end to end under `marrow test`: a `test` that reads or
writes durable data runs against its own fresh in-memory attachment, so the
transaction, the presence check, and the read-back all execute through the real
compiler, verifier, VM, and path kernel.

```sh
marrow test
```

```text
ok    add and read back
1 passed, 0 failed, 0 errored (1/1 selected)
```

## Running against a persistent store

`marrow test` proves the durable program against a fresh attachment each run. To
keep data between runs, an export runs against a provisioned store on disk with
`marrow run <export> --store <dir>`, and a store is populated and provisioned from
a flat-scalar export with `marrow import`.

The persistent path runs the program in a separate companion runner attached to
the store; the terminal never opens the store itself. This requires the **stock
install layout**: the `marrow-runner` binary and the `marrow-companions` release
manifest installed in the same directory as `marrow`. A plain
`cargo install --path crates/marrow` installs only the `marrow` command, which
gives the storeless and `marrow test` paths above; it does not assemble the
companion layout. The worked applications [`apps/emr`](../apps/emr/README.md) and
[`apps/club-locker`](../apps/club-locker/README.md) carry their own build tooling
that assembles the layout and runs against a native store.

With the stock layout present, provisioning and running against a store looks like
this. `import` reads one JSON object per line, each member a scalar named exactly
as a key column or field, and provisions the store on first use:

```sh
printf '{"id": 1, "text": "imported note"}\n{"id": 2, "text": "second"}\n' > seed.jsonl
marrow import --store ./store --jsonl seed.jsonl --root notes --keys id
```

```text
provisioned a fresh store at ./store
{"batches_committed":1,"rows_imported":2}
```

The store now holds the imported notes, and later runs read and write the same
data:

```sh
marrow run textOf --store ./store -- 1      # imported note
marrow run add --store ./store -- 3 "added via run"   # true
marrow run textOf --store ./store -- 3      # added via run
```

Every imported entry and every write is created through the compiler-checked path
kernel; no command receives a raw storage key, an engine handle, or a transaction
object. The `run` mint convenience is storeless only — a durable declaration with
no identity is a precise `check.durable_identity` failure against a store, never a
silent mint.

## Where to go next

- [The durable model, narrated](walkthrough.md) walks a complete durable
  application line by line.
- [Language reference](language/) defines current `.mw` behavior; start with
  [durable places](language/durable-places.md) and [idioms](language/idioms.md).
- [CLI reference](tools/cli.md) documents every command.
- [What Marrow is and is not](what-marrow-is.md) states the scope in one page.
- [Project status](status.md) separates current, and future work.
