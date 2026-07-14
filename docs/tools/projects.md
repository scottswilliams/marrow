# Projects

A Marrow project is a directory holding a manifest and a contained source tree.
The command-line tools capture a project into a single immutable project input
before doing any work with it, so file discovery and module identity have one
owner and one meaning.

## Layout

```text
my_app/
  marrow.toml      manifest (required)
  src/             source root (required for any module)
    main.mw        module `main`
    shelf/
      books.mw     module `shelf.books`
```

Source lives under the fixed `src` directory. Every `.mw` file under `src` is a
module; nothing outside `src` is captured. A project needs no `src` directory to
be valid — it simply has no modules.

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

## Module identity

A module's name is derived once from its path under `src`, with the directory
separators written as dots and the `.mw` extension removed:

| Source path | Module |
|---|---|
| `src/main.mw` | `main` |
| `src/shelf/books.mw` | `shelf.books` |

There is no in-source module header and no single-file fallback: the path is the
sole source of module identity. Because identities are relative to the project
root, moving a project to a new location does not change any module identity.

## Discovery bounds and faults

Discovery is deterministic — the captured modules are ordered by identity
regardless of the order the filesystem reports them — and bounded. Symlinks
under `src` are skipped, and a `src` that is itself a symlink is refused with
`project.source_path`, so the walk cannot cycle or escape the tree. A project
that exceeds a fixed capture bound (too many source files, one file too large,
or too much source in total) reports `project.capture_limit`.

Two source files that resolve to the same module identity — the same derived
name, or paths differing only in case — report `project.module_collision`. A
path that cannot name a contained module reports `project.source_path`. These
codes are listed in the [Error Code Reference](../error-codes.md).

## Creating a project

[`marrow init`](cli.md) creates a fresh project directory with a manifest and a
starter `src` module. It creates no store.
