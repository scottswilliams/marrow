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
native store, take its file lock, regenerate `marrow.lock`, or write durable
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
    "profile_version": "data.generation.v1",
    "store_uid": "store_00000000000000000000000000000001",
    "catalog_digest": "sha256:...",
    "commit": {
      "commit_id": 1,
      "catalog_epoch": 1,
      "source_digest": "sha256:...",
      "layout_epoch": 0,
      "engine_profile_digest": "77944eb86c08b665"
    },
    "open_transaction": null,
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
{"project":"/absolute/path/to/project","records":1,"cells":1,"roots":1,"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}
```

Both counts are a full store scan; they are exact, not estimates.

## `marrow data dump`

Prints every declared data cell `(path, value)` from one read-only snapshot —
identities in key order, members in declaration order. Derived index cells and
engine metadata are excluded. This is an explicit operator/admin dump, so it is
allowed to walk the whole store. Any future production preview must use bounded
pages. Text renders each value through its checked leaf type: strings are quoted
and escaped, bytes are `0x<hex>`, `Id(^store)` references are saved paths, and
enum values are module-qualified member identities. A leaf whose stored value the
checked type can no longer decode is corruption, not a value: a `string` whose
bytes are not valid UTF-8 renders as `<undecodable string: 0x<hex>>`, and an enum
naming a member the current type no longer has renders as
`<undecodable enum: cat_<id>>` (the stored member catalog id). Both forms are
marked so they are never mistaken for a `0x<hex>` bytes value; `data integrity` is
the authority that reports them as `data.decode`.

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
{"project":"/absolute/path/to/project","store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."},"cells":[{"path":"^counter(1).value","value_b64":"NDI="}]}
```

`--format jsonl` streams one cell object per line, then a summary line:

```jsonl
{"path":"^counter(1).value","value_b64":"NDI="}
{"kind":"summary","cells":1,"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}
```

The JSON envelope and JSONL summary include `store_snapshot`, the versioned
Marrow data generation DTO for the store read.

Paths in text use Marrow path syntax: `^root` for a root, `.name` for a field
or layer, and `(key)` for a record identity or keyed-layer key. A composite
identity or keyed layer takes its keys as one comma-separated group, for example
`^enrolls("s1","c9")`; the per-key spelling `^enrolls("s1")("c9")` is also
accepted, and commands emit the comma form. String keys render quoted (e.g.
`^users("alice")`), int and bool keys bare, bytes keys as `0x<hex>`, and temporal
keys as their canonical ISO text. The text format's string escapes are the `.mw`
escapes `\\`, `\"`, `\n`, `\r`, `\t` plus `\xNN` (lowercase hex) for every other
control byte, such as `\x00` or `\x1b`. This is a total, round-trippable
vocabulary broader than a `.mw` string literal — a stored string may hold a
control byte the language gives no escaped spelling — so a dumped path carries no
raw control byte, always re-parses to the key it spells, and stays feedable to
`data get` as a process argument. The `.mw` string-literal grammar is unchanged.
Any escape outside this vocabulary is a malformed path, not a silently stripped
backslash, so a path never resolves a key other than the one it spells. A stored
key that does not decode is reported as store corruption by integrity and
traversal commands.

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
example) is distinct both from an identity that exists with neither value nor
children and from a truly absent path. An identity can exist structurally even
after every field is cleared, so emptiness is reported truthfully rather than as
a has-children claim:

```
$ marrow data get ./project '^counter(1)'
(no value; has children)

$ marrow data get ./project '^counter(2)'
(exists; no value or children)

$ marrow data get ./project '^counter(3).value'
(absent)
```

Absence is a valid result, not an error: `get` exits `0` whether the path is
present or absent. A path argument that does not parse fails before touching the
store and exits `2`. A path that parses but the schema cannot resolve — an
undeclared saved root or member, or an identity or member key of the wrong scalar
type or arity — is a typed resolution failure, not a usage error: it reports
`data.unknown_path` with the offending path in `source_span.path` and exits `1`,
the same recoverable-failure code as the storage faults.

`--format json` reports the path, a presence state, the base64 value (or
`null` when there is no direct value), and the store version read:

```json
{
  "path": "^counter(1).value",
  "presence": "value_only",
  "value_b64": "NDI=",
  "store_snapshot": {
    "profile_version": "data.generation.v1",
    "store_uid": "store_00000000000000000000000000000001",
    "catalog_digest": "sha256:...",
    "commit": {
      "commit_id": 1,
      "catalog_epoch": 1,
      "source_digest": "sha256:...",
      "layout_epoch": 0,
      "engine_profile_digest": "77944eb86c08b665"
    },
    "open_transaction": null,
    "checked_source_digest": "sha256:..."
  }
}
```

The presence states are `absent`, `exists`, `value_only`, and `children_only`;
`exists` is a structurally-existing identity with no direct value and no
children. `jsonl`
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
longer declares, that each root holds exactly the cells its commit digest
recorded, and that typed store traversal does not report corruption. It is
read-only and typed: it needs the checked project to know each path's type. The
whole verdict reads one stable store snapshot, so it describes a single coherent
version of the data.

Because the data cells are their own derivation, a backend page that silently
drops a cell, truncates a record range, or rewrites a stored value with bytes that
still decode shifts every enumeration with no structural fault. The durable
per-root structural digest each commit stamps is the independent oracle: it covers
every committed cell's identity and value, so a root whose live cells disagree with
its digest — a dropped cell, a torn value, or a moved field — is reported as
`store.corruption`. The same digest gates `backup` and `data recover`, so neither
archives nor blesses a truncated or tampered store.

Because a range scan and a point lookup take different paths through the store's
btree, the same store-open witness also re-reads every committed cell through the
point-lookup descent typed reads use, so an interior-node flip that misroutes a
lookup past a committed cell the scan — and the digest — still cover is reported as
`store.corruption` rather than read absent, and it decodes the accepted
commit-metadata cell so a present-but-corrupt commit stamp fails closed regardless
of which command reads it or in which output format. The derived index btree is
reconciled the same way: every committed index entry the linear scan yields must be
reachable by the point-lookup descent an index read navigates, so an
interior-separator flip that misroutes a bounded index seek past a contiguous
subtree fails closed rather than letting `^root.index(key)` silently under-return.
It also reconciles each entry's stored identity against its redundant copy — the
trailing keys of a non-unique tuple or a unique entry's value — so a flip
diverging the two fails closed at store open rather than surfacing later as a
typed program fault at an innocent source span.
The runtime store-open also runs the schema-driven index-completeness cross-check
`data integrity`, `backup`, and `data recover` run — the entry count the data
records derive against the entries the index enumerates — so a truncated index
fails `run` and `serve` closed exactly as it fails those commands, never read
under-returning or written onto. `data integrity`, `data stats`, `data dump`,
`backup`, `data recover`, and the runtime store-open share this one witness.

The anchor cannot witness a rollback that drops the anchor itself. The committed
`marrow.lock` is the second, independent witness: it records the accepted catalog
roots and the epoch each became active at, so a **present** store missing a root
its own epoch covers — a store rolled back to its empty initial commit, a partial
root drop, or a uid-only store crashed mid-creation — has lost data and is
reported as `store.corruption`. A store still below a root's recorded activation
legitimately never held it: that is the store-behind case, which the advance
paths resolve by activating the store, and the inspections read such a store
clean at its own epoch. A root with no recorded activation always reads as a
loss when missing, the fail-closed default. An **absent** store body under a
committed lock is the disposable-store case, not a loss: the next write-capable
run, `evolve apply`, or `serve --write` seeds an empty store from the committed
identity (announced loudly), so the read-only inspections, `backup`, `doctor`,
and `data recover` treat it as a clean first run rather than corruption. Every
read-only inspection (`data integrity`, `data stats`, `data roots`, `data dump`,
`data get`), `doctor`, `backup`, and `data recover` run this lock-root
cross-check against a present store, so none blesses, counts, reads, archives,
or repairs a store missing a root its epoch covers. A project with no committed
lock — a genuine first run — is the separate missing-lock case rather than
corruption. A backup mounted with `--backup` is self-contained and is inspected
regardless of the live project's lock. The activation rule cannot see a rollback
that resets the whole store body to an old epoch: such a store is locally
indistinguishable from a checkout that never advanced past that epoch, so a root
activated later reads as legitimately absent even when the rollback destroyed
its records. Store-side commit records are the mechanism that closes that hole;
this witness does not claim to.

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
{"project":"/absolute/path/to/project","cells":1,"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."},"problems":[{"code":"data.decode","help":null,"kind":"tooling","message":"stored value is not a canonical int form","source_span":{"path":"^counter(1).value"}}]}
```

```jsonl
{"code":"data.decode","help":null,"kind":"tooling","message":"stored value is not a canonical int form","source_span":{"path":"^counter(1).value"}}
{"kind":"summary","problems":1,"cells":1,"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}
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
store open/repair completed: ./project/.marrow/data/marrow.redb
```

`--format json` and `jsonl` emit the same single status object:

```json
{"project":"/absolute/path/to/project","status":"opened","store":"./project/.marrow/data/marrow.redb"}
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
