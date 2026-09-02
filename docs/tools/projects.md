# Projects

A Marrow project is a directory with a `marrow.toml` manifest and a `src` tree of `.mw` files. Every command takes the whole project as its input.

A file under `src` names its own module:

```mw
module docs::projects::books

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^books[id].title = title
    }
}

pub fn title(id: int): string? {
    return ^books[id].title
}

test "add then read" {
    add(1, "Small Gods")
    assert title(1) ?? "" == "Small Gods"
}
```

This file lives at `src/docs/projects/books.mw`. The path gives the module its name, and the `module` line matches it. `marrow run docs.projects.books.add -- 1 x` addresses the export from the command line, and `marrow test` runs its test against a fresh in-memory store.

## Layout

```text
my_app/
  marrow.toml      manifest
  .marrow/
    ids            identity ledger, committed with the source
  src/
    main.mw        module `main`
    shelf/
      books.mw     module `shelf.books`
```

`src` holds every source file the tools read. A file outside `src` is not part of the program. A project with no `src` directory has no source files.

`.marrow` holds files the tools write for themselves. The identity ledger `.marrow/ids` is the one file there that belongs in version control. The tools write `.marrow/.gitignore` to keep their own working entries out of Git. A project with no durable declarations has no `.marrow` directory.

[`marrow init`](cli.md#marrow-init) creates a project with a manifest and a `src/main.mw` script.

## Manifest

`marrow.toml` holds one required key, `edition`, which fixes the language edition the project targets:

```toml
edition = "2026"
```

The schema is closed. An unknown key, a missing or non-string `edition`, an edition other than `2026`, or malformed TOML is a `config.invalid` error:

```text
$ marrow check .
config.invalid: unknown manifest key `name`; the only supported key is `edition`
```

The manifest carries no store, entry point, source root, test, or client settings.

## Modules

A source file's module name comes from its path under `src`: directory separators become dots and the `.mw` extension is dropped.

| Source path | Module name | Declaration |
|---|---|---|
| `src/main.mw` | `main` | `module main` |
| `src/shelf/books.mw` | `shelf.books` | `module shelf::books` |

A file that carries a matching `module` declaration is importable with `use`. A file with no `module` line is a script. It is checked under its path-derived name and no other file can import it, but its exports stay addressable from the command line, as in `marrow run main`. Names are relative to the project root, so moving the project changes nothing. Two files with the same module name, including paths that differ only in case, are a `project.module_collision` error. [Modules and functions](../language/modules-and-functions.md#modules-and-imports) defines imports, visibility, and exports.

## Identity ledger

A project that declares durable data carries `.marrow/ids`. Each line binds one durable declaration (the application, a resource, a field, a store root, a key, an index) to a random 128-bit id:

```text
marrow ids v0
machine-written by marrow; do not edit
id application . f5dd8d6b36a729b4e07cf416234a7874
id product Book 9e70f27c3cc9d8b8f0c368b58ce2ceba
id field Book.title fcf399ce0cb2622ce3819e50c5521165
id root books 18a30fc1d6b9901b118dfd5bbb3ada57
id key books.id 6a1a29b9d923efee77527b7750a3f4c4
high-water 0
end
```

Stored data is bound to these ids, so a declaration keeps its identity through edits, moves, and clones as long as the ledger travels with the source. Commit the file. The tools write it; a developer never edits, copies, or cites its contents. The file is line-diffable, and parallel branches merge it textually. A merge that leaves two lines claiming one identity, a truncated file, or any other damage is a `project.ids_corrupt` error; restore the file from version control.

The first storeless [`marrow run`](cli.md) of an export mints every missing id and writes the ledger. Until then, `marrow check` and `marrow test` report `check.durable_identity` for each missing declaration:

```text
$ marrow check .
src/docs/projects/books.mw:7:7: check.durable_identity: durable identity for application `.` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)
$ marrow run docs.projects.books.add -- 1 x
cli.durable_unsupported
$ marrow check .
3 exports across 2 modules
```

The run mints and then stops, because a durable export needs a store; `marrow run --store` runs it against one and never mints ([operations](../operations/README.md)). Inside a Git repository whose index lacks `.marrow/ids`, the mint prints a one-line reminder to commit it.

The mint is additive. It adds a line for each missing declaration and keeps every existing line. Renaming a field mints a new id for the new name, and the old line stays. Deleting a declaration and adding it back under the same name readopts its old id. A `retired` line and the retirement high-water are part of the ledger's grammar and are enforced when the ledger is read. No command writes one today.

The ledger has exactly one home. A file at the project root named `marrow.ids` is a `project.ids_location` error naming the move.

## Bounds

Source discovery is deterministic: files are ordered by their names, whatever order the filesystem reports them in. A symbolic link anywhere in the project is refused (`project.source_path` at `src`, `project.ids_corrupt` at the ledger, `io.read` elsewhere).

| Limit | Value | Diagnostic |
|---|---|---|
| Source files | 4,096 | `project.capture_limit` |
| One source file | 1 MiB | `project.capture_limit` |
| Source in total | 64 MiB | `project.capture_limit` |
| Directory entries visited under `src` | 65,536 | `io.read` |
| Directory depth under `src` | 64 | `io.read` |
| Manifest | 1 MiB | `io.read` |
| Identity ledger | 8,192 lines, 1 MiB | `project.ids_corrupt` |

These limits are fixed. Every code is listed in the [error code reference](../error-codes.md).
