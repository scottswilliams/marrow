# Project Configuration

A Marrow project is described by a single file, `marrow.json`, at the project
root. Every project command — `check`, `run`, `test`, `fmt`, `data`, and `serve`
— reads `<projectdir>/marrow.json` first. The file
holds project choices only: source roots, a default entrypoint, the store
backend and its data directory, test patterns, and the accepted catalog metadata
path. It does not hold compiled schemas, index metadata, data-evolution history,
permissions, connection strings, or secrets.

Unknown keys are rejected, so a typo is an error rather than a silently ignored
setting.

## Complete Example

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::sample::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "acceptedCatalog": "marrow.catalog.json",
  "tests": ["tests/**/*.mw"]
}
```

The minimal valid file is just the required field:

```json
{ "sourceRoots": ["src"] }
```

With this minimal file there is no default entry (you must pass `--entry` to
`run`), the store is in-memory (nothing is persisted), and no tests are
discovered. The accepted catalog path defaults to `marrow.catalog.json`.

## Fields

| Key | Type | Required | Default |
|---|---|---|---|
| `sourceRoots` | array of strings | yes | — |
| `run.defaultEntry` | string | no | none |
| `store.backend` | `"memory"` \| `"native"` | no | in-memory |
| `store.dataDir` | string | only when `backend` is `"native"` | — |
| `acceptedCatalog` | string | no | `marrow.catalog.json` |
| `tests` | array of strings | no | `[]` |

All keys are camelCase. Any other top-level key, or any other key inside `run`
or `store`, is rejected (see [Validation](#validation)).

### `sourceRoots`

Directories searched for `.mw` library source, relative to the project root.
Listing at least one is required. Each entry must be a relative path that stays
under the project root — empty, absolute, and `..`-bearing values are rejected.

A `.mw` file's path under a source root determines the module it must declare:
`shelf/books.mw` under `src` must declare `module shelf::books`. A file directly
under a source root maps to a bare name (`src/books.mw` → `module books`).

Multiple roots are searched in order:

```json
{ "sourceRoots": ["src", "lib"] }
```

If two roots overlap (for example `src` and `src/sub`), a file reachable through
both is discovered once, named by the first root that lists it. A source root
that exists but holds no `.mw` files is valid; a source root that does not exist
on disk is an error (`project.source_root`) when a command walks it.

### `run.defaultEntry`

The qualified name of the `pub fn` that `marrow run` calls when no `--entry` is
given:

```json
{ "run": { "defaultEntry": "shelf::sample::main" } }
```

The entry must be a function in a declared module — a `module::function` name
such as `shelf::sample::main`. Functions in a module-less single-file script are
not part of the runnable program, so a bare script function cannot be a run
entry. A qualified name resolves to that exact module; a bare name (passed via
`--entry`) matches the first function of that name in any module.

If neither `--entry` nor `run.defaultEntry` is set, `run` reports
`run.no_entry` and exits non-zero.

`run` prints only what the program writes with `print`/`write`. Returning a
value does not print it.

### `store.backend` and `store.dataDir`

The storage selection. When `store` is omitted entirely, commands use an
in-memory store: nothing is persisted, and each `run` or `test` starts empty.

- `memory` — an in-memory store. Creates no files. `dataDir` is ignored if
  present (and may be omitted).

  ```json
  { "sourceRoots": ["src"], "store": { "backend": "memory" } }
  ```

- `native` — the persistent on-disk store. Requires a non-empty `dataDir`,
  a relative path under the project root. The store file is created at
  `<dataDir>/marrow.redb`. The data directory is created on first use by a
  command that writes (such as `run`); read-only inspection (`data`, `serve`)
  never creates it.

  ```json
  {
    "sourceRoots": ["src"],
    "store": { "backend": "native", "dataDir": ".marrow/data" }
  }
  ```

  With the example above, the store lives at `.marrow/data/marrow.redb`
  relative to the project root.

Code checks store capabilities, not backend names; `memory` and `native` are
configuration vocabulary, not a permission layer.

### `acceptedCatalog`

The generated accepted catalog metadata file, relative to the project root.
When omitted, commands read `marrow.catalog.json`.

The catalog file is committed project metadata. It records stable IDs, aliases,
lifecycle state, an epoch, and a digest for durable declarations such as
resources, stores, indexes, resource members, enums, and enum members. Source
checks read the file when present and return an in-memory replacement proposal
when it is missing or stale; they do not write the file.

Renames fail closed unless the accepted catalog already records the new
canonical path and the old path as an alias for the same stable ID.

### `tests`

Glob-style patterns selecting `.mw` test files. Each test file lives outside the
source roots — test files are scripts, not library modules — so a test file is
named from its project-root-relative path: `tests/books_test.mw` →
`tests::books_test`.

`marrow test` runs every `pub fn` with no parameters in a discovered test file,
each against a fresh in-memory store. Test entries follow the same under-root
rule as source roots (no empty, absolute, or `..`-bearing values).

Pattern forms (the directory-walk subset of globbing):

| Pattern | Matches |
|---|---|
| `tests/**/*.mw` | every `.mw` file under `tests`, recursively |
| `tests/**` | same — recursive walk of `tests` |
| `tests/*.mw` | `.mw` files directly in `tests` only (no recursion) |
| `tests` | bare directory; walked recursively |
| `tests/smoke.mw` | a single file, taken directly |

A pattern that matches nothing on disk contributes no tests — it is not an
error. With no `tests` key, `marrow test` finds no tests and reports so.

## Validation

`marrow.json` is parsed and validated before any command runs. Every failure
reports the `config.invalid` code (kind `tooling`) and exits with code `1`. The
rules:

- `sourceRoots` must list at least one directory.
- `store.backend` must be `"memory"` or `"native"`; any other value is
  rejected and the unknown name is named in the message.
- A `native` store must have a non-empty `dataDir`. (A `native` store cannot
  open without a directory, so this is rejected at parse time, not at open
  time.)
- Every path value — each `sourceRoots` entry, `dataDir`, `acceptedCatalog`, and
  each `tests` entry — must be relative and must not be empty, absolute, or
  contain a `..` component. Such a value would escape the project root, so it is
  rejected.
- Unknown top-level keys, and unknown keys inside `run` or `store`, are
  rejected.
- Malformed JSON is rejected.

A missing `marrow.json` is a separate, earlier failure: the command reports
`io.read` (it could not read the file) and exits `1`.

### Error examples

Run against the binary, these are the exact diagnostics (text format):

```
$ marrow check ./proj          # store.backend = "postgres"
config.invalid: unknown store backend `postgres`; expected `memory` or `native`

$ marrow check ./proj          # store.backend = "native", no dataDir
config.invalid: the `native` store backend requires a non-empty `dataDir`

$ marrow check ./proj          # sourceRoots missing or empty
config.invalid: `sourceRoots` must list at least one source directory

$ marrow check ./proj          # unknown top-level key "globals"
config.invalid: unknown field `globals`, expected one of `sourceRoots`, `run`, `store`, `tests`, `acceptedCatalog` at line 1 column 35

$ marrow check ./proj          # sourceRoots: ["/etc"]
config.invalid: `sourceRoots entry` `/etc` must be relative to the project root, not absolute

$ marrow check ./proj          # dataDir: "../data"
config.invalid: `dataDir` `../data` must not contain a `..` component

$ marrow check ./proj          # no marrow.json
io.read: failed to read proj/marrow.json: No such file or directory (os error 2)
```

The same `config.invalid` appears in the machine-readable envelope under
`--format json`:

```json
{
  "code": "config.invalid",
  "kind": "tooling",
  "message": "unknown store backend `postgres`; expected `memory` or `native`",
  "source_span": null
}
```

`config.invalid` is the single configuration error code; it covers every case
above except a missing file. See [Errors](error-codes.md) for the full envelope
and exit-code contract.
