# Data Inspection And Repair Tools

Data tools inspect typed Marrow data. They do not define a second storage model
and they do not expose raw store keys, raw saved-path encoders, or backend byte
streams as production APIs.

The cross-tool product boundary is summarized in
[tooling-surfaces.md](tooling-surfaces.md): `data dump` and `data get` are
operator/admin inspection, not production data APIs.

The v0.1 tooling contract is:

- read through checked source, accepted catalog metadata, and typed tree-cell
  store APIs;
- render durable places from checked/catalog facts, not by decoding physical
  engine keys into source-shaped paths;
- page large results with opaque cursors;
- report typed data findings such as `data.decode`, `data.key_type`, and
  `data.orphan`;
- surface tree-cell store faults through the `store.*` family;
- keep repair as explicit maintenance code over modeled data.

Typed backup/restore is a tooling and backup-protocol contract, not a raw
archive replay: it carries a typed manifest binding the data to the source
digest, accepted catalog epoch, engine profile, and value-codec version it was
written under, and a restore validates that binding plus full data integrity,
including orphaned managed cells, before it activates. It is a separate command
pair, not a `data` subcommand — see
[`marrow backup` and `marrow restore`](cli.md#marrow-backup).
`data` itself only reads.

Inspection never creates or modifies the store. It opens the configured native
store read-only, and if no store file exists yet it reports an empty result
rather than creating one. Running `roots`, `stats`, `dump`, `integrity`, or
`get` against an unseeded project prints `(no saved data)` (or `(absent)` for
`get`) and leaves the data directory untouched — no `marrow.redb` is written.

A command that traverses the store more than once — `dump`, `stats`, and
`integrity` each make several passes — pins one store snapshot for the whole
command, so its output describes a single coherent version of the data.

There is no in-place repair command: repair is operator-authored maintenance code
run under `marrow run --maintenance`.

## What needs source, what does not

All `data` subcommands load and check the project first. The checked facts
provide root/member catalog IDs, key arity, and leaf types before the command
reads the tree-cell store. If the source does not check, the command fails with
the check diagnostic and exits non-zero before touching the store.

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

Prints every stored `(path, value)` in encoded order. Values render as canonical
payload bytes; text uses UTF-8 when possible and `0x<hex>` otherwise.

```
$ marrow data dump ./project
^counter(1).value	42
```

Text is tab-separated: the Marrow path, then the value rendered as UTF-8 text
when the bytes are valid UTF-8 (the common case, since canonical forms are
ASCII), else as `0x<hex>`.

`--format json` wraps all records in one object; each record carries the checked
path plus base64 of the value bytes:

```json
{"project":"/abs/project","records":[{"path":"^counter(1).value","value_b64":"NDI="}]}
```

`--format jsonl` streams one record object per line, then a summary line:

```jsonl
{"path":"^counter(1).value","value_b64":"NDI="}
{"kind":"summary","records":1}
```

Paths in text use Marrow path syntax: `^root` for a root, `.name` for a field
or layer, and `(key)` for a record identity or keyed-layer key. String keys render
quoted (e.g. `^users("alice")`), int and bool keys bare, bytes keys as
`0x<hex>`, and temporal keys as their canonical ISO text. A stored key that does
not decode is reported as store corruption by integrity and traversal commands.

## `marrow data get`

Reads one path's value for diagnostic/admin inspection. It is not a
backup/restore format.

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

The presence states are `absent`, `value_only`, and `children_only` — the same
spelling the serve protocol uses. `jsonl` emits the same single object as
`json`.

## `marrow data integrity`

Verifies that each checked, reachable stored value decodes against its declared
schema type, that no stored cell is left under a root or member the schema no
longer declares, and that typed store traversal does not report corruption. It is
read-only and typed: it needs the checked project to know each path's type. The
whole verdict reads one stable store snapshot, so it describes a single coherent
version of the data.

It exits `0` on a clean store and `1` when it finds any problem.

```
$ marrow data integrity ./project
ok: ./project integrity verified (1 records)
```

It surfaces three data findings plus typed store corruption:

- `data.decode` — a stored value is not a canonical form of its declared
  scalar type (e.g. a non-int byte sequence stored under an `int` field).

  ```
  ^note(1).body: data.decode: stored value is not a canonical int form
  ```

- `data.key_type` — a stored record key, keyed-layer key, or identity payload
  key has a scalar type the schema does not declare.

  ```
  ^counter("one").value: data.key_type: stored key is a string where the schema declares int
  ```

- `data.orphan` — an actual stored data cell is under a saved root or member the
  schema no longer declares, left behind by a dropped root or field. Beyond
  verifying declared cells, integrity enumerates the store's actual data cells
  and flags any the schema does not account for; derived index cells are never
  flagged.

  ```
  ^books(7).blurb: data.orphan: stored data is under a saved member the schema no longer declares
  help: run `marrow data integrity` after source-native evolution or maintenance repair
  ```

- `store.corruption` — a tree-cell key or payload cannot be decoded by the typed
  store contract; an actual stored data cell whose key does not decode under the
  tree-cell key grammar is reported here.

Generated index entries are maintained by the runtime and store, so integrity
verifies declared data cells and flags only undeclared data cells, not derived
index cells.

Text prints one `path: code: message` line per problem to stderr; a clean store
prints a single `ok:` line to stdout. `--format json` wraps the findings in an
envelope; `--format jsonl` streams one envelope per finding plus a summary:

```json
{"problems":[{"code":"data.decode","kind":"tooling","message":"stored value is not a canonical int form","source_span":{"path":"^counter(1).value"}}],"project":"/abs/project","records":1}
```

```jsonl
{"code":"data.decode","kind":"tooling","message":"stored value is not a canonical int form","source_span":{"path":"^counter(1).value"}}
{"kind":"summary","problems":1,"records":1}
```

These findings have no source line, so the location is a `path` field rather
than a span. The `data.*` codes carry kind `tooling`. See
[error-codes.md](error-codes.md) for the full code list.

When integrity reports orphaned managed cells, correct the schema, run
source-native `evolve preview`/`evolve apply`, or repair modeled data through
explicit maintenance code, then run `marrow data integrity` again. There is no
in-place fix.

## Deferred: `diff` and `load`

`marrow data diff` and `marrow data load` are not implemented — see
[future/data-tools.md](future/data-tools.md). Bulk data movement is
[`marrow backup` and `marrow restore`](cli.md#marrow-backup), not a `data`
subcommand.

## See also

- [cli.md](cli.md) — all `marrow` commands.
- [error-codes.md](error-codes.md) — exit codes and the error envelope.
- [backend-contract.md](backend-contract.md) — typed tree-cell operations,
  presence states, child-key ordering, and store responsibilities the `data`
  commands read through.
