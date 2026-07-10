# Project File

A project has one `marrow.json` file at its root. Commands that accept a project
directory parse this file before loading source or durable data. The file names
source roots, an optional default entry, storage, and test paths.

## Example

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::books::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests"]
}
```

## Fields

| Field | JSON type | Requirement and default |
|---|---|---|
| `sourceRoots` | array of strings | Required and non-empty. |
| `run` | object | Optional. No default entry when absent. |
| `run.defaultEntry` | string | Optional qualified or uniquely resolvable public entry name. |
| `store` | object | Optional. Absence selects the memory/no-store profile. |
| `store.backend` | `"memory"` or `"native"` | Required whenever `store` is present. |
| `store.dataDir` | string | Required and non-empty for `native`; unused by `memory`. |
| `tests` | array of strings | Optional; defaults to `[]`. |
| `client` | string | Accepted only for the [legacy generated-client path](../legacy.md). |

Unknown fields are rejected at the top level and inside `run` and `store`.
`run` and `store`, when present, must be objects. Malformed JSON and type
mismatches are `config.invalid` failures.

## Project Paths

Each `sourceRoots` entry, `store.dataDir`, each `tests` entry, and the legacy
`client` value is interpreted relative to the project root. Every such value
must:

- be non-empty;
- be relative;
- contain no `..` path component;
- contain no NUL byte.

`run.defaultEntry` and `store.backend` also reject NUL bytes. These checks occur
while parsing the project file, before path joining or store access.

`sourceRoots` must contain at least one entry. Source roots are searched in
array order. If roots overlap, a file reachable through both is named by the
first root. Recursive source discovery skips symlinked files and directories.
A source file's path below its root determines its required module name: for
example, `src/shelf/books.mw` must declare `module shelf::books`.

Each `tests` value names one file or directory; it is not a glob. The characters
`*`, `?`, `[`, `]`, `{`, and `}` are rejected. Test paths must not equal, contain,
or sit below a source root. A configured test path that does not exist simply
selects no tests, and test discovery also skips symlinks.

## Storage Selection

Omitting `store` selects the memory/no-store profile. This permits projects
without durable declarations; a project that declares durable resources or
stores is rejected with `check.durable_store_required` until it selects
`native`.

The native backend requires `dataDir`. The current implementation stores its
private file beneath that directory and has no command-line storage override.
See [Native Store Operations](../operations/native-store.md) for ownership and
filesystem behavior.

## Related Artifacts

`marrow.lock` is not configuration. It is the committed projection of accepted
durable identity from the live store and can seed an absent disposable store.
Do not add generated catalog, index, migration, credential, or permission data
to `marrow.json`.
