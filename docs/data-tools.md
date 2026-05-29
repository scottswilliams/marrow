# Data Inspection And Repair Tools

`marrow data` reads a project's saved data directly from its store, without
running any `.mw` code. It is for inspecting what is on disk, diagnosing a
store, and verifying saved values against the schema.

```
marrow data roots     [--format text|json|jsonl] <projectdir>
marrow data stats     [--format text|json|jsonl] <projectdir>
marrow data dump      [--format text|json|jsonl] <projectdir>
marrow data integrity [--format text|json|jsonl] <projectdir>
marrow data get       [--format text|json|jsonl] <projectdir> <path>
```

Every subcommand takes one project directory (a directory containing
`marrow.json`). `get` also takes one path. See [cli.md](cli.md) for the full
command set and [error-codes.md](error-codes.md) for exit codes and the error
envelope.

## Read-only by default

Inspection never creates or modifies the store. It opens the configured native
store read-only, and if no store file exists yet it reports an empty result
rather than creating one. Running `roots`, `stats`, `dump`, `integrity`, or
`get` against an unseeded project prints `(no saved data)` (or `(absent)` for
`get`) and leaves the data directory untouched — no `marrow.redb` is written.

There is no in-place repair command. To rewrite a store, use `marrow restore`
with an archive (see [cli.md](cli.md)); restore's replace/merge/repair modes are
the maintenance write path. `data` itself only reads.

## What needs source, what does not

`roots`, `stats`, `dump`, and `get` work on raw saved bytes. They do not check
the project's `.mw` source, so they run even when the source has type errors —
useful for diagnosing a store whose schema no longer compiles.

`integrity` is typed: it verifies stored values against their declared schema
types, so it first checks the project. If the source does not check, `integrity`
fails with the check diagnostic and exits non-zero before touching the store.

A `data` command against a project with a missing or invalid `marrow.json`
reports the `config.*` family and exits `1`. (Exit `2` is reserved for a
command-line usage error — a missing directory, a bad flag, or an unparseable
`<path>` for `get` — detected before the command body runs.)

## Output formats

Each subcommand accepts `--format text` (default), `--format json`, or
`--format jsonl`. The flag is a separate argument; `--format=json` is not
accepted. Text is for reading; `json`/`jsonl` carry exact bytes losslessly via
base64, for machine consumers. An unknown format name exits `2`.

## `marrow data roots`

Lists the project's saved roots.

```
$ marrow data roots ./project
^counter
```

`--format json` (and `jsonl`, which carries no streaming meaning here and emits
the same single object):

```json
{"project":"/abs/project","roots":["counter"]}
```

`roots` is the bare root name without the `^` in JSON. An empty store prints
`(no saved data)` in text and `"roots":[]` in JSON.

## `marrow data stats`

Counts saved roots and records (one record is one stored `(path, value)` pair).

```
$ marrow data stats ./project
roots: 1
records: 1
```

```json
{"project":"/abs/project","records":1,"roots":1}
```

The record count is a full store scan; it is exact, not an estimate.

## `marrow data dump`

Prints every stored `(path, value)` in encoded order. Values render raw — as
their canonical stored bytes — not schema-typed, so `dump` works without source.

```
$ marrow data dump ./project
^counter(1).value	42
```

Text is tab-separated: the Marrow path, then the value rendered as UTF-8 text
when the bytes are valid UTF-8 (the common case, since canonical forms are
ASCII), else as `0x<hex>`.

`--format json` wraps all records in one object; each record carries the human
`path` plus base64 of the exact path and value bytes:

```json
{"project":"/abs/project","records":[{"path":"^counter(1).value","path_b64":"AWNvdW50ZXIAAgKAAAAAAAAAAQN2YWx1ZQA=","value_b64":"NDI="}]}
```

`--format jsonl` streams one record object per line, then a summary line:

```jsonl
{"path":"^counter(1).value","path_b64":"AWNvdW50ZXIAAgKAAAAAAAAAAQN2YWx1ZQA=","value_b64":"NDI="}
{"kind":"summary","records":1}
```

Paths in text use Marrow path syntax: `^root` for a root, `.name` for a field,
child layer, or index, and `(key)` for a record or index key. String keys render
quoted (e.g. `^users("alice")`), int and bool keys bare, bytes keys as
`0x<hex>`, and temporal keys as their canonical ISO text. A stored key that does
not decode renders as `?<hex>`.

## `marrow data get`

Reads one path's value.

```
$ marrow data get ./project '^counter(1).value'
42
```

A path with no direct value but with children (a record identity node, for
example) is distinct from a truly absent path:

```
$ marrow data get ./project '^counter(1)'
(no value; has children)

$ marrow data get ./project '^counter(2).value'
(absent)
```

Absence is a valid result, not an error: `get` exits `0` whether the path is
present or absent. A path argument that does not parse fails before touching the
store and exits `2`.

`--format json` reports the path, a presence state, and the base64 value (or
`null` when there is no direct value):

```json
{"path":"^counter(1).value","presence":"value_only","value_b64":"NDI="}
{"path":"^counter(1)","presence":"children_only","value_b64":null}
{"path":"^counter(2).value","presence":"absent","value_b64":null}
```

The four presence states are `absent`, `value_only`, `children_only`, and
`value_and_children` — the same spelling the serve protocol uses. `jsonl` emits
the same single object as `json`.

## `marrow data integrity`

Verifies that every stored value decodes against its declared schema type, and
that every stored key is well-formed and accounted for by the schema. It is
read-only and typed: it needs the checked project to know each path's type.

It exits `0` on a clean store and `1` when it finds any problem.

```
$ marrow data integrity ./project
ok: ./project integrity verified (1 records)
```

It surfaces three findings:

- `data.decode` — a stored value is not a canonical form of its declared
  scalar type (e.g. a non-int byte sequence stored under an `int` field).

  ```
  ^note(1).body: data.decode: stored value is not a canonical int form
  ```

- `data.orphan` — saved data lives under a root the schema does not declare,
  or names a member the schema does not declare.

  ```
  ^counter(1).value: data.orphan: saved data under an unknown root or undeclared member
  ```

- `store.corrupt_path` — a stored key is not a well-formed saved path. This
  is the store's own code, surfaced through integrity.

Generated index entries are raw by design and are not flagged.

Text prints one `path: code: message` line per problem to stderr; a clean store
prints a single `ok:` line to stdout. `--format json` wraps the findings in an
envelope; `--format jsonl` streams one envelope per finding plus a summary:

```json
{"problems":[{"code":"data.orphan","kind":"tooling","message":"saved data under an unknown root or undeclared member","source_span":{"path":"^counter(1).value"}}],"project":"/abs/project","records":1}
```

```jsonl
{"code":"data.orphan","kind":"tooling","message":"saved data under an unknown root or undeclared member","source_span":{"path":"^counter(1).value"}}
{"kind":"summary","problems":1,"records":1}
```

These findings have no source line, so the location is a `path` field rather
than a span. The `data.*` codes carry kind `tooling`. See
[error-codes.md](error-codes.md) for the full code list.

A typical repair flow when integrity reports problems: back up the store
(`marrow backup`), correct the schema or the archive, then `marrow restore` into
a fresh store. There is no in-place fix.

## Deferred: `diff` and `load`

`marrow data diff` and `marrow data load` are not implemented — see
[future/data-tools.md](future/data-tools.md). Until then, use `marrow backup` and
`marrow restore` for bulk data movement.

## See also

- [cli.md](cli.md) — all `marrow` commands, including `backup` and `restore`.
- [error-codes.md](error-codes.md) — exit codes and the error envelope.
- [backend-contract.md](backend-contract.md) — path/value operations, presence
  states, child-key ordering, and store responsibilities the `data` commands
  read through.
