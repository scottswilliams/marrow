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
- keep operator/admin full-store scans explicit;
- allow inspection subcommands to use a validated backup artifact as the read
  target without opening the live store;
- report typed data findings such as `data.decode`, `data.key_type`,
  `data.dangling_ref`, `data.incomplete`, and `data.orphan`;
- surface tree-cell store faults through the `store.*` family;
- keep repair as explicit maintenance code over modeled data.

`marrow data dump` is the operator/admin exception: it intentionally walks the
whole typed store snapshot and prints every stored path/value pair. It is not a
production preview API.

Typed backup/restore is a tooling and backup-format contract, not a raw
archive replay: the archive carries a typed manifest, the accepted-catalog
rows, and the canonical data-cell stream, and a restore validates the manifest
binding plus the schema checks required for activation — including orphaned
managed cells — before it
activates. It is a separate command pair, not a `data` subcommand — see
[`marrow backup` and `marrow restore`](cli.md#marrow-backup).
Within `data`, `recover` is the explicit write-capable native-store repair/open
verb. The other subcommands only read.

Inspection subcommands never create or modify the store. By default they open
the configured native store read-only, and if no store file exists yet they
report an empty result rather than creating one: `roots` and `dump` print
`(no saved data)`, `stats` reports zero roots and cells, `integrity` reports
`ok` over zero records, and `get` prints `(absent)`. The data directory is left
untouched — no `marrow.redb` is written.

`roots`, `stats`, `dump`, `integrity`, and `get` accept
`--backup <artifact>`. The flag selects the backup as the read target for the
checked project, validates it through the restore artifact contract, replays it
into memory, and inspects that memory store. It does not open the configured
native store, take its file lock, render `marrow.catalog.json`, or write durable
state. Backup validation reports the same typed `restore.*` refusals as restore
for unsupported format, corrupt chunks, engine/value-codec mismatch,
source/catalog mismatch, and invalid data. `data recover` does not accept
`--backup` because recover is specifically a native-store repair/open command.

A command that traverses the store more than once — `dump`, `stats`, and
`integrity` each make several passes — pins one store snapshot for the whole
command, so its output describes a single coherent version of the data.

`marrow data recover` is an in-place native-store repair/open command. It opens
an existing store write-capably so the backend can replay an interrupted commit.
It does not repair modeled data; that remains operator-authored maintenance code
run under `marrow run --maintenance`.

## What needs source, what does not

Inspection subcommands load and check the project first; the live-store path
opens the store read-only to bind the accepted-catalog snapshot. With
`--backup`, the check binds to the accepted catalog carried by the backup and
does not open the live store. The checked facts provide root/member catalog IDs,
key arity, and leaf types before the command reads tree-cell data. If the source
does not check, the command fails with the check diagnostic and exits non-zero
before reading any data cells.

Inspection renders a stored value by the leaf type the **accepted catalog**
recorded it under — the epoch the data was written under — not the current
source spelling. When source has drifted from the committed store (a blocked
populated-leaf retype, such as `int` to `string`), `dump` and `get` still show
the real stored type, so an operator sees the data as it must be migrated rather
than as the uncommitted proposal would mistype it. The override is render-only;
the lossless `json`/`jsonl` `value_b64` carries the unchanged raw bytes either
way.

`recover` is different: it reads only `marrow.json` and the configured native
store binding, then opens the existing store write-capably. It does not load or
check source before opening the store.

A `data` command against a project with a missing `marrow.json` reports
`io.read`; an unparseable or invalid one reports `config.invalid`. Both exit
`1`. (Exit `2` is reserved for a command-line usage error — a missing
directory, a bad flag, or an unparseable `<path>` for `get` — detected before
the command body runs.)

## Output formats

Each subcommand accepts `--format text` (default), `--format json`, or
`--format jsonl`. The flag is a separate argument; `--format=json` is not
accepted. Text is for reading; `json`/`jsonl` carry exact bytes losslessly via
base64, for machine consumers. An unknown format name exits `2`.

Structured JSON reports that include a `project` field render the canonical
absolute project directory, equivalent to `std::fs::canonicalize(<projectdir>)`,
not the raw directory argument.

## `marrow data roots`

Lists the project's saved roots.

```
$ marrow data roots ./project
^counter
```

`--format json` (and `jsonl`, which carries no streaming meaning here and emits
the same single object):

```json
{
  "project": "/absolute/path/to/project",
  "roots": ["counter"],
  "store_snapshot": {
    "store_uid": "store_00000000000000000000000000000001",
    "catalog_digest": "sha256:...",
    "commit": {
      "commit_id": 1,
      "catalog_epoch": 1,
      "source_digest": "sha256:...",
      "layout_epoch": 0,
      "engine_profile_digest": "77944eb86c08b665"
    },
    "checked_source_digest": "sha256:..."
  }
}
```

`roots` is the bare root name without the `^`. An empty store prints `(no saved
data)` in text and
`"roots":[]` in JSON. `store_snapshot` identifies the store version read by
`roots`; it is `null` when no store-backed read occurs. Inside a present
snapshot, store metadata fields such as `store_uid`, `catalog_digest`, and
`commit` may be `null` when the store has not recorded that metadata.

## `marrow data stats`

Counts saved roots, records, and cells. One record is one saved entity, an
identity tuple such as `^counter(1)`, and can contain multiple cells; one cell
is one stored `(path, value)` pair. The record count is the same number `marrow
backup` reports and `restore --replace --count N` confirms.

```
$ marrow data stats ./project
roots: 1
records: 1
cells: 1
```

```json
{"project":"/absolute/path/to/project","records":1,"cells":1,"roots":1}
```

Both counts are a full store scan; they are exact, not estimates.

## `marrow data dump`

Prints every declared data cell `(path, value)` from one read-only snapshot —
identities in key order, members in declaration order. Derived index cells and
engine metadata are excluded. This is an explicit operator/admin dump, so it is
allowed to walk the whole store. Any future production preview must use bounded
pages. Text renders each value through its checked leaf type: strings are quoted
and escaped, bytes are `0x<hex>`, `Id(^store)` references are saved paths, and
enum values are module-qualified member identities.

```
$ marrow data dump ./project
^counter(1).value	42
```

Text is tab-separated: the Marrow path, then one leaf value. String values use
the same quoting and escaping as string keys, so tabs, newlines, and path-like
text stay inside one TSV field. Bytes values always render as `0x<hex>`.
References render as their referent path, for example `^authors(1)`. Enum values
render as one member identity, for example `app::Status::archived`.

`--format json` wraps all field cells in one object; each cell carries the checked
path plus base64 of the value bytes:

```json
{"project":"/absolute/path/to/project","cells":[{"path":"^counter(1).value","value_b64":"NDI="}]}
```

`--format jsonl` streams one cell object per line, then a summary line:

```jsonl
{"path":"^counter(1).value","value_b64":"NDI="}
{"kind":"summary","cells":1}
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

The default text renderer uses the same value contract as `data dump`: strings
are quoted and escaped, bytes are `0x<hex>`, references are saved paths, and enum
values are member identities. Use `--format json` when a caller needs the exact
stored bytes.

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

`--format json` reports the path, a presence state, the base64 value (or
`null` when there is no direct value), and the store version read:

```json
{
  "path": "^counter(1).value",
  "presence": "value_only",
  "value_b64": "NDI=",
  "store_snapshot": {
    "store_uid": "store_00000000000000000000000000000001",
    "catalog_digest": "sha256:...",
    "commit": {
      "commit_id": 1,
      "catalog_epoch": 1,
      "source_digest": "sha256:...",
      "layout_epoch": 0,
      "engine_profile_digest": "77944eb86c08b665"
    },
    "checked_source_digest": "sha256:..."
  }
}
```

The presence states are `absent`, `value_only`, and `children_only`. `jsonl`
emits the same single object as `json`. `store_snapshot` is `null` when the
read has no store-backed version, such as a missing store or an uncommitted
durable identity. Inside a present snapshot, store metadata fields such as
`store_uid`, `catalog_digest`, and `commit` may be `null` when the store has not
recorded that metadata.

## `marrow data integrity`

Verifies that each checked, reachable stored value decodes against its declared
schema type, that each canonical identity leaf points to an existing saved
record node, that each existing record or keyed-layer entry carries its accepted
required fields, that no stored cell is left under a root or member the schema no
longer declares, and that typed store traversal does not report corruption. It
is read-only and typed: it needs the checked project to know each path's type.
The whole verdict reads one stable store snapshot, so it describes a single
coherent version of the data.

Catalog state is not store corruption. A saved root or member whose durable
identity is still pending is treated as absent until a run or evolution apply
records it, and an in-flight `evolve default` does not create a stored-data
completeness obligation. If source carries an in-flight catalog-intent
diagnostic, data commands report that original `check.*` diagnostic in the
requested format before inspecting the store. Only malformed tree-cell bytes,
malformed catalog rows, or backend damage report `store.corruption`.

It exits `0` on a clean store and `1` when it finds any problem.

```
$ marrow data integrity ./project
ok: ./project integrity verified (1 cells)
```

It surfaces five data findings plus typed store corruption:

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

- `data.dangling_ref` — a stored `Id(^root)` leaf is canonical and key-typed
  but points to no saved record node in the referenced root.

  ```
  ^books(7).authorId: data.dangling_ref: stored `Id(^authors)` reference points to no saved record
  ```

- `data.incomplete` — an existing record or keyed-layer entry is missing an
  accepted required field. A completely absent record or absent keyed-layer entry
  is not incomplete.

  ```
  ^books(7).title: data.incomplete: required saved member is absent
  ```

- `data.orphan` — an actual stored data cell is under a saved root or member the
  schema no longer declares, left behind by a dropped root or field. Beyond
  verifying declared cells, integrity enumerates the store's actual data cells
  and flags any the schema does not account for; derived index cells are never
  flagged. Restore rejects this condition before activation instead of treating
  it as production archive replay.

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
{"project":"/absolute/path/to/project","cells":1,"problems":[{"code":"data.decode","help":null,"kind":"tooling","message":"stored value is not a canonical int form","source_span":{"path":"^counter(1).value"}}]}
```

```jsonl
{"code":"data.decode","help":null,"kind":"tooling","message":"stored value is not a canonical int form","source_span":{"path":"^counter(1).value"}}
{"kind":"summary","problems":1,"cells":1}
```

These findings have no source line, so the location is a `path` field rather
than a span. Every finding carries a `help` key, `null` when there is no hint.
The `data.*` codes carry kind `tooling`. `data.incomplete` findings also carry
`store_catalog_id`, `record_identity`, `parent_path`, and
`missing_member_catalog_id`; `data.dangling_ref` findings carry
`containing_identity`, `field_catalog_id`, `referenced_root`, and
`referenced_identity`. These are typed catalog/key fields for automation, while
`source_span.path` is only the operator display path. See
[error-codes.md](error-codes.md) for the full code list.

When integrity reports incomplete records or orphaned managed cells, correct the
schema, run source-native `evolve preview`/`evolve apply`, or repair modeled data
through explicit maintenance code, then run `marrow data integrity` again. There
is no `data` command for modeled-data fixes.
When it reports a dangling reference, create the referenced record or rewrite the
stored identity value through modeled maintenance code, then run integrity again.

## `marrow data recover`

Opens the configured native store write-capably so the backend can replay an
interrupted commit after a read-only command reported `store.recovery_required`.
It reads only `marrow.json` to find the store binding; it does not load or check
source files first.

A missing native store is treated as nothing to recover and is not created. An
existing file that is not a Marrow store, including an empty file, is
`store.corruption`. If replay/open finds damage beyond recovery, the command
reports the store error such as `store.corruption`.

```
$ marrow data recover ./project
store open/repair completed: ./project/.data/marrow.redb
```

`--format json` and `jsonl` emit the same single status object:

```json
{"project":"/absolute/path/to/project","status":"opened","store":"./project/.data/marrow.redb"}
```

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
