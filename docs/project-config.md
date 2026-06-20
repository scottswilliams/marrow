# Project Configuration

A Marrow project is described by a single file, `marrow.json`, at the project
root. Every command that takes a project directory — `check`, `run`, `test`,
`fmt`, `data`, `evolve`, `backup`, and `restore` — reads
`<projectdir>/marrow.json` first. The file
holds project choices only: source roots, a default entrypoint, the store
backend and its data directory, and test paths. It does not hold compiled
schemas, the accepted catalog, index metadata, data-evolution history,
permissions, connection strings, or secrets.

Unknown keys are rejected, so a typo is an error rather than a silently ignored
setting.

## Complete Example

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::sample::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests"]
}
```

The minimal valid file selects a source root and a store backend:

```json
{ "sourceRoots": ["src"], "store": { "backend": "memory" } }
```

With this minimal file there is no default entry (you must pass `--entry` to
`run`), the explicit store is in-memory (nothing is persisted), and no tests are
discovered. The memory backend admits a `run` only for a program with no
durable declarations — see
[`store.backend`](#storebackend-and-storedatadir).

## Fields

| Key | Type | Required | Default |
|---|---|---|---|
| `sourceRoots` | array of strings | yes | — |
| `run.defaultEntry` | string | no | none |
| `store.backend` | `"memory"` \| `"native"` | yes | — |
| `store.dataDir` | string | only when `backend` is `"native"` | — |
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
Recursive source walks skip symlinked files and directories.

Multiple roots are searched in order:

```json
{ "sourceRoots": ["src", "lib"], "store": { "backend": "memory" } }
```

If two roots overlap (for example `src` and `src/sub`), a file reachable through
both is discovered once, named by the first root that lists it. A source root
that exists but holds no `.mw` files is valid; a source root that does not exist
on disk is an error (`project.source_root`) when a command walks it.

### `run.defaultEntry`

The entry name that `marrow run` calls when no `--entry` is given:

```json
{
  "sourceRoots": ["src"],
  "store": { "backend": "memory" },
  "run": { "defaultEntry": "shelf::sample::main" }
}
```

An entry must name a public function. A qualified name such as
`shelf::sample::main` resolves to that exact module. A bare name is accepted only
when it names one public function in the checked program; if two modules export
the same function name, qualify the entry. A module-less single-file script uses
bare entry names because its functions live in the script file rather than a
declared module.

If neither `--entry` nor `run.defaultEntry` is set, `run` reports
`run.no_entry` and exits non-zero.

`run` prints only what the program writes with `print`. Returning a
value does not print it.

### `store.backend` and `store.dataDir`

The required storage selection. `marrow test` always runs each test on a fresh
in-memory store. The memory backend admits only a program with no durable
declarations; a program that declares a durable surface (a `resource`, a saved
`store`, or an `enum`) needs a native store. The backend is statically known, so
`marrow check` rejects a durable program under the memory backend with
`check.durable_store_required`; `marrow run` enforces the same contract as
`run.durable_store_required`. The supported production saved-data backend is the
native redb store.

- `memory` — an in-memory store. Creates no files. `dataDir` is ignored if
  present (and may be omitted). This backend is not a production `^` durability
  profile; a durable program is rejected at check here.

  ```json
  { "sourceRoots": ["src"], "store": { "backend": "memory" } }
  ```

- `native` — the persistent on-disk store. Requires a non-empty `dataDir`,
  a relative path under the project root. The store file is created at
  `<dataDir>/marrow.redb`. The data directory is created on first use by a
  command that writes (such as `run`); read-only inspection (`data`) never
  creates it.

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

### `tests`

Plain project-relative paths selecting `.mw` test files. Each test file lives
outside the source roots — test files are scripts, not library modules — so a
test file is named from its project-root-relative path: `tests/books_test.mw` →
`tests::books_test`.

`marrow test` runs every `pub fn` with no parameters in a discovered test file,
each against a fresh in-memory store. Test entries follow the same under-root
rule as source roots (no empty, absolute, or `..`-bearing values). They also
reject glob metacharacters (`*`, `?`, `[`, `]`, `{`, `}`) so a typo cannot
silently drop coverage.

| Entry | Matches |
|---|---|
| `tests` | every `.mw` file under `tests`, recursively |
| `tests/smoke.mw` | a single file, taken directly |

A path that resolves to nothing on disk contributes no tests — it is not an
error. With no `tests` key, `marrow test` finds no tests and reports so.
Configured test symlinks are skipped, and recursive test walks skip symlinked
files and directories.

## Validation

`marrow.json` is parsed and validated before any command runs. Every failure
reports the `config.invalid` code (kind `tooling`) and exits with code `1`. The
rules:

- `sourceRoots` must list at least one directory.
- `store` is required and must be an object.
- `store.backend` must be `"memory"` or `"native"`; any other value is
  rejected and the unknown name is named in the message.
- A `native` store must have a non-empty `dataDir`. (A `native` store cannot
  open without a directory, so this is rejected at parse time, not at open
  time.)
- Every path value — each `sourceRoots` entry, `dataDir`, and each `tests`
  entry — must be relative and must not be empty, absolute, or contain a `..`
  component. Such a value would escape the project root, so it is rejected.
- `tests` entries must not contain glob metacharacters (`*`, `?`, `[`, `]`,
  `{`, `}`).
- A `tests` entry must not overlap a source root — it must not equal, sit under,
  or contain one. Test files are scripts that live outside the source roots; an
  overlap would load that root's library modules and run their `pub fn`s as
  tests.
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

$ marrow check ./proj          # store missing
config.invalid: `store` must select either "native" or "memory"; for a one-line memory store use "store": { "backend": "memory" }

$ marrow check ./proj          # unknown top-level key "globals"
config.invalid: unknown field `globals`, expected one of `sourceRoots`, `run`, `store`, `tests` at line 1 column 35

$ marrow check ./proj          # sourceRoots: ["/etc"]
config.invalid: `sourceRoots entry` `/etc` must be relative to the project root, not absolute

$ marrow check ./proj          # dataDir: "../data"
config.invalid: `dataDir` `../data` must not contain a `..` component

$ marrow check ./proj          # sourceRoots: ["src"], tests: ["src"]
config.invalid: `tests entry` `src` overlaps source root `src`; test files must live outside the source roots

$ marrow check ./proj          # no marrow.json
io.read: failed to read ./proj/marrow.json: No such file or directory (os error 2)
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
