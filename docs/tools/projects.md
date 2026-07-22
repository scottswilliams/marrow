# Projects

A Marrow project is a directory holding a manifest and a contained source tree.
The command-line tools capture a project into a single immutable project input
before doing any work with it, so file discovery, canonical file identity, and
path-derived source names have one owner and one meaning.

## Layout

```text
my_app/
  marrow.toml      manifest (required)
  .marrow/         committed project metadata (machine-written)
    ids            durable-identity ledger (present only when the project
                   declares durable data)
  src/             source root (required for any source file)
    main.mw        path-derived name `main`
    shelf/
      books.mw     path-derived name `shelf.books`
```

`.marrow` is the project's behind-the-scenes metadata directory. It holds
committed machine-written artifacts only — today, the identity ledger. Commit
the directory with the source, like `.github`; caches and stores never live in
it, so it is never ignored. Developers do not read or edit its contents.

Source lives under the fixed `src` directory. Every `.mw` file under `src` is
captured; nothing outside `src` is captured. A project needs no `src` directory
to be valid — it simply has no source files.

## Manifest

`marrow.toml` is a small closed-schema TOML file. Its only key is a required
`edition`, which fixes the language edition the project targets so parsing never
inherits a moving toolchain default.

```toml
edition = "2026"
```

The schema is closed: an unknown key, a missing `edition`, a non-string
`edition`, an unsupported edition, or malformed TOML each reject with
`config.invalid`. A malformed-TOML fault reports the offending line and column;
`2026` is the only edition this build supports.

The manifest holds project choices only. There is no store, backend, entry
point, source-root, test, or client configuration; those are not part of the
current schema.

## Source identity and modules

Each captured source file receives a name derived once from its path under
`src`, with the directory separators written as dots and the `.mw` extension
removed:

| Source path | Derived name | Matching module declaration |
|---|---|---|
| `src/main.mw` | `main` | `module main` |
| `src/shelf/books.mw` | `shelf.books` | `module shelf::books` |

The path-derived name identifies the source independently of its contents. To be
importable with `use`, a file carries a `module` declaration that matches that
name, using `::` between segments. A headerless file is a script: it is checked
in its path-derived namespace and is not importable, while its command-line
exports remain addressable under the dot-separated derived name. Because names
are relative to the project root, moving a project to a new location does not
change them. [Modules and functions](../language/modules-and-functions.md)
defines the import, visibility, and command-line export rules.

## Discovery bounds and faults

Discovery is deterministic — the captured source files are ordered by identity
regardless of the order the filesystem reports them — and physically bounded. The
project root, `marrow.toml`, `src`, and `.marrow/ids` are each admitted through an
opened handle whose observed kind and identity are checked before and after use;
capture never trusts path metadata and then reopens the path. A symbolic link at
`src`, `marrow.toml`, or `.marrow/ids`, or on a component leading to one, is
refused rather than followed (`project.source_path` for the source root,
`project.ids_corrupt` for the ledger, `io.read` for the manifest); a symbolic
link below `src` is skipped, so the walk cannot cycle or escape the tree. A
required file — the manifest, the identity ledger, or a selected source — that is
a special file (a FIFO, socket, or device) or a hard link with more than one link
count is refused before its body is read; a special file below `src` is ignored
like any other non-`.mw` entry.

The walk is bounded before retention: `marrow.toml` and `.marrow/ids` are each read
to a fixed byte ceiling plus one; the total number of directory entries visited
below `src` (65,536) and the source directory depth (64) are fixed; and a project
that exceeds a source capture bound (4,096 source files, 1 MiB per file, or 64 MiB
of source in total) reports `project.capture_limit`. These bounds are enforced by
the bounded physical adapter (`marrow-project-fs`) and conformance-tested through
the command-line capture path.

Two source files that resolve to the same module identity — the same derived
name, or paths differing only in case — report `project.module_collision`. A
path that cannot name a contained module reports `project.source_path`. These
codes are listed in the [Error Code Reference](../error-codes.md).

## The identity ledger

A project that declares durable data carries `.marrow/ids`: the durable-identity
ledger binding each durable declaration (the application, a store root, its key
column, the stored resource, and each stored field) to an opaque entropy-minted
id. **Commit it with the source** — the ledger is part of the program: a clone
or relocated checkout then reuses the committed identities exactly, and parallel
branches merge it line by line. The file is **machine-written only**; never
edit, copy, or cite its contents. After a mint inside a Git repository whose
index does not yet hold `.marrow/ids`, `marrow run` prints a one-line reminder
to commit it.

The ledger is a compile- and apply-time artifact and is never on the runtime
path: it scales with the declared schema — one row per durable declaration,
none per data row — so a store of any size leaves it unchanged. Its size is
bounded (8,192 rows, 1 MiB); parsing a full-bound 8,192-row artifact is
measured at about 2 ms, and an oversized file is rejected at the bound without
being read.

A merge that leaves conflict markers, two rows claiming one identity (the
signature of the same declaration minted independently on two branches), a
truncated file, or any other damage is rejected whole with
`project.ids_corrupt`; restore the file from version control rather than
repairing it by hand. The ledger has exactly one home: a ledger found at the
retired project-root path `marrow.ids` is refused with `project.ids_location`
and a one-line steer (`git mv marrow.ids .marrow/ids`), and a project with
files at both paths fails closed until they are reconciled.

In the ledger model the ledger is append-only about the past: a retired identity
is recorded as a tombstone and is never reissued, so removing a durable
declaration and re-adding its name yields a fresh identity rather than silently
adopting old data. Recording a removal as a tombstone is the accepted apply
action's job (future). The one mint today is **storeless** [`marrow run`](cli.md)
— run without `--store`: it is additive-only, adding a row for each missing anchor
and never tombstoning, so deleting a declaration and re-adding the same path
readopts the old id, and a rename leaves the old row live and orphaned. This is
bounded to development before a store exists. A persistent
[`marrow run … --store <dir>`](cli.md) does **not** mint: once a store is bindable
the additive mint could readopt an orphaned id or diverge from the store's
committed ledger, so a missing identity there is a precise `check.durable_identity`
failure the developer resolves deliberately (the tombstone-aware mint is the
accepted apply action's job). In ordinary development the ledger is invisible —
storeless `marrow run` mints missing identities automatically; every other command
requires them to be present and fails precisely with `check.durable_identity` when
one is missing. A storeless project has no `.marrow/ids`.

## Creating a project

[`marrow init`](cli.md) creates a fresh project directory with a manifest and a
starter headerless `src/main.mw` script. It creates no store.
