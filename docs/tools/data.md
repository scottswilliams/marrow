# Data Tools

The `data` command family provides typed operator inspection of a project's
durable data. It resolves source paths through the checked project and reads
through typed store operations. It does not expose raw native-store keys or act
as an application data API.

## Syntax

```text
marrow data roots [--backup <artifact>] [--format text|json|jsonl] <projectdir>
marrow data stats [--backup <artifact>] [--format text|json|jsonl] <projectdir>
marrow data dump [--backup <artifact>] [--format text|json|jsonl] <projectdir>
marrow data integrity [--backup <artifact>] [--format text|json|jsonl] <projectdir>
marrow data get [--backup <artifact>] [--format text|json|jsonl] <projectdir> <path>
marrow data recover [--format text|json|jsonl] <projectdir>
```

`roots`, `stats`, `dump`, `integrity`, and `get` are read-only. They load and
check the project before reading data. With no `--backup`, they open an existing
native store read-only. A missing store reports an empty result and is not
created.

With `--backup`, those commands validate the archive, mount it in memory, and
inspect that snapshot without opening the live store or consulting its lock.
The archive must match the checked project's accepted source and catalog facts.

`recover` is the only write-capable subcommand. It is covered separately below
and in [Recovery](../operations/recovery.md).

## Inspection Commands

| Command | Result | Traversal |
|---|---|---|
| `roots` | Saved roots visible through the checked project. | Root inventory |
| `stats` | Root, saved-entity (`records`), and stored-value (`cells`) counts. | Full snapshot |
| `dump` | Every checked stored path/value pair. | Full snapshot |
| `get` | The direct value and presence state of one exact path. | Point-bounded |
| `integrity` | Decode, required-field, identity-reference, orphan, and store-corruption findings. | Full snapshot |

`stats`, `dump`, and `integrity` pin one read snapshot for their passes. Run
full scans outside latency-sensitive paths for large stores. Generated index
entries are derived data and are not emitted by `dump`.

Text values are rendered through their checked types. JSON and JSONL carry
stored value bytes as base64. JSONL streams `dump` cells and integrity findings,
then emits a summary record; commands with a single result emit one object.

If current source no longer declares stored cells, reduced inspections can emit
a `data.orphan` advisory on stderr while retaining their normal exit status.
`data integrity` owns the complete finding and exits nonzero when any integrity
problem exists.

## Data Paths

Text paths use ordinary durable path spelling:

- `^books` names a root;
- `^books(1)` names one keyed entry;
- `^books(1).title` names a member;
- `^enrollments("s1","c9")` supplies a composite key.

String keys are quoted; integer and boolean keys are bare; bytes use `0x<hex>`.
String path escapes include `\\`, `\"`, `\n`, `\r`, `\t`, and lowercase
`\xNN` for other control bytes. A malformed path is a usage failure. A
well-formed path with the wrong root, member, key type, or arity is
`data.unknown_path`.

`get` distinguishes four presence states in structured output: `absent`,
`exists`, `value_only`, and `children_only`. Absence is a successful read. Text
mode prints `(absent)`, `(exists; no value or children)`, or
`(no value; has children)` when there is no direct value.

## Integrity

`data integrity` checks that reachable stored values decode under their accepted
types, required members are present, identity values refer to existing records,
and no managed cells remain under undeclared roots or members. It also consumes
the store's structural and traversal checks, so physical corruption reports
under `store.*` rather than being misclassified as a schema problem.

Pending durable identity and pending defaulted members create no stored-data
obligation until accepted. Modeled repairs are ordinary checked Marrow code run
with explicit `--maintenance`; there is no raw mutation subcommand.

## Recovery Open

`data recover` loads `marrow.json`, locates an existing native store, and opens
it write-capably so the backend can replay an interrupted commit. Loading
`marrow.lock` is best-effort: a missing, unreadable, or corrupt lock does not
block physical recovery, so that run cannot use the lock as a lost-root witness.
Source checking is also not a precondition. When source does check, recover uses
those facts to verify data and index completeness before reporting success.

A missing store is treated as nothing to recover and is not created. Recover
does not repair modeled data, accept schema evolution, or turn a foreign or
damaged file into a Marrow store. After the write-capable pass it reopens and
traverses the store read-only; failure to converge is reported as corruption.
