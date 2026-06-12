# CLI Reference

The `marrow` binary is the single entry point for the language and its built-in
database.

```
marrow check [--data] [--format text|json|jsonl] <file.mw | projectdir>
marrow evolve <preview|apply> [--format text|json|jsonl] <projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow run [--entry <entry>] [--maintenance] [--trace] [--dry-run] \
  [--format text|json|jsonl] <projectdir>
marrow test [--trace] [--format text|json|jsonl] <projectdir>
marrow data <roots|stats|dump|integrity|recover> [--format text|json|jsonl] <projectdir>
marrow data get [--format text|json|jsonl] <projectdir> <path>
marrow backup [--format text|json|jsonl] <projectdir> <output-file>
marrow restore [--format text|json|jsonl] [--replace --count N] <projectdir> <backup-file>
marrow --version
marrow --help
```

A project directory contains a `marrow.json`; see
[project-config.md](project-config.md) for its fields. Every subcommand accepts
`--help` (or `-h`) and prints its own usage.

## Exit Codes

| Code | Meaning |
|---:|---|
| `0` | The command completed successfully. |
| `1` | A recoverable failure: parse/check diagnostics, a failing test, a runtime or storage error, a project or tooling failure. |
| `2` | A command-line usage error, detected before the command body ran: an unknown subcommand or flag, a missing or duplicated argument, or an invalid flag value. |

See [error-codes.md](error-codes.md) for the dotted error codes and the
machine-readable error envelope these commands emit.

## Output Formats

Commands that report diagnostics, saved data, or test results take `--format`:

- `text` (the default) — human-readable lines. Diagnostics and findings go to
  stderr; primary results go to stdout.
- `json` — one JSON object for the command's structured report.
- `jsonl` — one JSON object per line for streaming reports, ending with a
  `{"kind": "summary", …}` line where the report has many records.

Plain `run` output is the program's own `print`/`write` stream, which carries no
envelope and does not accept `--format`. `run --trace` and `run --dry-run`
accept `--format` for their tooling reports; those reports are written to
stderr, leaving stdout for the program's own output.

`marrow test --format json|jsonl` shapes the test pass/fail report on stdout.
With `--trace`, the trace is a separate tooling report on stderr using the same
format, while the test report stays on stdout.

---

## `marrow check`

```
marrow check [--data] [--format text|json|jsonl] <file.mw | projectdir>
```

Parse a single `.mw` file, or check a whole project directory, and report
diagnostics.

- Given a `.mw` file, it parses the file, then runs the full project checker
  over a synthesized one-file project, so type and module-path rules apply to a
  lone file. Only rules that need other project files are out of reach.
- Given a project directory, it loads `marrow.json` and runs the project checker
  over every source root plus configured test files: parse, type, effect, and
  durable-place checks. It binds durable identity from the committed
  `marrow.catalog.json` artifact, repairing that file from a committed store
  snapshot when the local store already has one; it never creates the store or
  freezes identity.
- `--data` is project-only. It opens the configured store read-only and runs the
  same data-attached evolution preview that `marrow evolve preview` uses. A
  repair-required or approval-required witness exits `1`.

Exits `0` when there are no errors, `1` when there are (or when the file or
`marrow.json` cannot be read).

```console
$ marrow check src/shelf.mw
ok: src/shelf.mw parsed (3 declarations)

$ marrow check ./proj
ok: ./proj checked

$ marrow check --format json src/broken.mw
{"file":"src/broken.mw","status":"failed","diagnostics":[{"code":"parse.syntax", …}],"declarations":0}
```

A failing check returns exit `1`:

```console
$ marrow check src/broken.mw
src/broken.mw:1:1: error: parse.syntax: expected function parameter list
$ echo $?
1
```

---

## `marrow evolve`

```
marrow evolve preview [--format text|json|jsonl] <projectdir>
marrow evolve apply [--maintenance] [--approve-retire <catalog-id>:<count>] \
  [--format text|json|jsonl] <projectdir>
```

`evolve preview` opens the configured store read-only, discharges source,
accepted catalog metadata, store snapshot, and engine metadata into an exact
witness, then reports the counts and blocking diagnostics.

`evolve apply` recomputes that preview witness over the live project and store,
requires an exact match, checks the activation window, and commits the data work
plus metadata stamp in one transaction. Like `run`, it records a project's
baseline durable identity first when the project has none yet, then applies the
evolution against the accepted catalog. The advanced accepted catalog rows commit
in that same store transaction as the data work and the slim commit stamp, so
the catalog never advances without the data behind it; after that commit, the
CLI renders `marrow.catalog.json` from the committed store snapshot. The command
output still renders receipt counts for defaults, transforms, retires, and
rebuilt indexes, but those counts are not persisted in commit metadata.
Destructive retire needs `--maintenance` and an approval whose catalog ID and
populated count match the preview.

---

## `marrow fmt`

```
marrow fmt [--check | --write] <file.mw | projectdir>
```

Format Marrow source. `marrow fmt` does not read from stdin.

- A single `.mw` file with no flag prints the formatted source to stdout.
- `--check` reports each file that is not already formatted and exits `1` if any
  differ; it writes nothing.
- `--write` rewrites changed files in place. Each changed file is written to an
  adjacent temporary file and replaces the original only after the new content is
  written successfully; a parse or write failure leaves the original file intact.
- A project directory formats every `.mw` file under its source roots, and
  requires `--check` or `--write`. Printing many files to stdout is meaningless,
  so a bare `marrow fmt <dir>` is a usage error (exit `2`).

Source that does not parse is reported and left untouched (exit `1`).

```console
$ marrow fmt src/shelf.mw          # print formatted source
module shelf
…

$ marrow fmt --check ./proj        # exit 1 if anything is unformatted
$ marrow fmt --write ./proj        # rewrite in place
```

Exit codes: `0` formatted/already-formatted; `1` a `--check` file differs or a
file failed to parse or write; `2` a directory with no `--check`/`--write`, an
unknown flag, or a `-` stdin argument.

---

## `marrow run`

```
marrow run [--entry <entry>] [--maintenance] [--trace] [--dry-run] \
  [--format text|json|jsonl] <projectdir>
```

Check a project, then run an entry function over the store its `marrow.json`
selects (see [project-config.md](project-config.md)). A project must check
cleanly before it runs. The in-memory default admits only a program with no
durable declarations; a program that declares a durable surface (a `resource`,
a saved `store`, or an `enum`) needs a configured `native` store and otherwise
fails with `run.durable_store_required`.

A clean run records the project's baseline durable identity if it has none yet:
the first run of a project with a durable surface freezes the accepted catalog
into the store transactionally as it commits, or republishes an already
committed `marrow.catalog.json` into an empty local store, then renders the file
from that committed snapshot. A project already past its baseline proposes no
change and the store's catalog rows are left untouched; if the store snapshot is
ahead of the file, or the file contains a torn non-conflict render, the command
repairs the file from the store and proceeds.
There is no separate acceptance step. See [data-evolution.md](data-evolution.md).

Opening a native store is fenced against its catalog activation stamp. A store
that holds saved records but no activation stamp is refused
(`run.store_unstamped`); run `marrow check --data` and `marrow evolve apply` to
stamp it first. When the source's shape drifted from the stamped schema, a
change that mutates no stored records (such as adding a sparse field) is
auto-applied through the production apply path and the run proceeds against the
advanced catalog; a change that would backfill, transform, or destructively
drop populated data is refused with `run.schema_drift`, naming the
`marrow evolve apply` step that discharges it.

The entry is `--entry` if given, otherwise the project's `run.defaultEntry`.
Qualified entries (`module::function`) resolve exactly. A bare entry name is
accepted only when it names one public function in the checked program; ambiguous
bare names fail with `run.ambiguous_function`. If neither entry source is
present, `run` fails with `run.no_entry` (exit `1`).

Output written with `print`/`write` goes to stdout. `std::log` output goes to
stderr. The run reads the real system clock, environment, and filesystem.

`--maintenance` grants the run the maintenance capability for data evolution and
repair tooling. It permits whole managed-root deletes and required-field deletes
that the default run rejects. An operator must type it; the default run and
`run.defaultEntry` can never inject it. Use it deliberately.

`--trace` reports each statement as it runs — file, line, call depth, and the
visible locals — and each managed write or delete, in execution order. The trace
is tooling output on stderr under every format, leaving the program's stdout for
its own `print`/`write` output: under text an indented stream, under `json`/`jsonl`
`step` records and managed-write `write` records.

In the human-readable text of a `--trace` or `--dry-run` write, the leaf value is
rendered as its declared typed scalar, not as raw codec bytes: a `bool` reads
`true`/`false`, an int/date/duration/instant reads its canonical typed text. The
machine-readable `value_b64` field in the JSON output stays the raw stored bytes.

`--dry-run` runs the entry against an isolated store and reports the saved-data
writes it would commit. Native-store dry runs copy the configured store after the
normal run setup has opened, fenced, and applied any zero-mutation schema drift;
the entry then runs against the copy, so user `transaction` blocks cannot consume
the dry-run boundary. Only saved data is isolated; host side effects such as
`std::io` writes or `std::log` lines are not.

`--dry-run` takes `--format`. A plain run's stdout is the program's own output
and takes no format, so `--format` without `--trace` or `--dry-run` is a usage
error (exit `2`). The report is tooling output on stderr under every format,
off the program's stdout stream. Under text, planned writes are
`would write <path>` / `would delete <path>` lines and a
`dry run: N write(s), M delete(s) (not committed)` summary. Under `json`/`jsonl`,
the report is a `{"committed": false, "planned": […]}` envelope whose planned
entries carry the op, human path, and base64 value bytes.

`--trace` composes with `--dry-run`: the run is traced while its saved writes are
isolated from the configured store. The trace and the dry-run report both go to stderr — under `--format
json` the trace object followed by the dry-run envelope as separate top-level JSON
objects — so the program's own stdout output stays uninterrupted. For source-native
data evolution use `marrow evolve preview`; `run --maintenance --dry-run` is for
explicit repair/admin code.

Exits `0` on success, `1` if the project does not check, the store cannot be
opened, there is no entry, or the run raises an error. An uncaught runtime fault
is reported on stderr located at the source it was raised in,
`file:line:col: code: message`, the same form `check` and `test` use.

```console
$ marrow run ./proj
added 1: Small Gods

$ marrow run --entry shelf::main ./proj
added 2: Small Gods

$ marrow run --maintenance --entry shelf::repair ./proj
```

---

## `marrow test`

```
marrow test [--trace] [--format text|json|jsonl] <projectdir>
```

Check a project, then run its tests: every `pub fn` with no parameters in a test
file (the `tests` glob patterns in `marrow.json`). Each test runs against a fresh
in-memory store. A test's `std::log` output is discarded so it stays out of the
report.

In text format, each result is printed as `ok`, `FAIL` (a `std::assert::*`
failure, code `run.assertion`), or `ERROR` (any other runtime error), located at
the test's source position, followed by a summary line.

Under `--format json`, stdout is one test report envelope:

```json
{"project":"./proj","tests":[{"kind":"test","name":"tests::smoke_test::add_runs","status":"passed","location":{"file":"tests/smoke_test.mw","line":1,"column":1}}],"summary":{"total":1,"passed":1,"failed":0,"errored":0}}
```

Under `--format jsonl`, stdout is one test-result record per line followed by a
summary record:

```jsonl
{"kind":"test","name":"tests::smoke_test::add_runs","status":"passed","location":{"file":"tests/smoke_test.mw","line":1,"column":1}}
{"kind":"summary","total":1,"passed":1,"failed":0,"errored":0}
```

Failed and errored JSON records also carry the runtime fault `code` and
`message`. Passing result locations point at the test function declaration;
failed and errored result locations point at the runtime fault.

Exits `0` only when every test passes. It exits `1` if any test fails or errors,
if the project does not check, or if no test is found (`test.none`).

With `--trace`, every test runs under an execution trace attributed to that test
by name. The trace is tooling output on stderr; the test report stays on stdout,
so the two streams never interleave. Text trace events stream as they run. Under
`--format json`, stderr is one JSON envelope with a `traces` array, one entry per
test. Under `--format jsonl`, stderr remains a newline-delimited stream of trace
events and per-test summary records.

```console
$ marrow test ./proj
ok    tests::smoke_test::add_runs
FAIL  tests::shelf_test::title_is_set
      tests/shelf_test.mw:7:5: run.assertion: assertion failed: isTrue(false)

2 tests: 1 passed, 1 failed, 0 errored
$ echo $?
1
```

The implemented assertions are `std::assert::isTrue`, `isFalse`, `absent`, and
`fail`.

## `marrow data`

`marrow data` is the typed inspection and repair-tooling boundary. It must read
through checked source, accepted catalog metadata, and typed tree-cell store
APIs. It does not expose raw backend keys, raw saved-path encoders, or archive
streams as production CLI behavior.

There is no `marrow explain` command in v0.1. Checked access, path, and name
facts are internal compiler/tooling facts surfaced through diagnostics,
`marrow data integrity`, dry-run reports, editor features, or future
accepted tooling surfaces. They are not exposed as query-plan, optimizer, or
standalone explanation output.

Diagnostic/admin/operator access to a project's saved data. The v0.1 decision is
to keep `get` and `dump` as `marrow data` subcommands, not production app APIs.
The inspection subcommands never create or modify the store; a project with no
saved data on disk reports as empty. `recover` is the only write-capable `data`
subcommand: it opens an existing native store so the backend can replay an
interrupted commit. `get` is exact-path and point-bounded. `dump` is
snapshot-bound and must stream or page rather than materializing unbounded data.
See [data-tools.md](data-tools.md) for full output shapes and the path syntax.
These commands are not production app APIs and not a production backup/restore
format.

`data diff` and `data load` are deferred — see
[future/data-tools.md](future/data-tools.md).

All `data` commands exit `2` on a usage error (missing directory, bad flag, an
unparseable `<path>` for `get`), and `1` on a config or store error. `roots`,
`stats`, `dump`, `recover`, and `get` exit `0` otherwise; `integrity` exits `1`
when it finds a problem.

### `data roots`

List the project's saved roots, one `^root` per line (or `(no saved data)`).

```console
$ marrow data roots ./proj
^books
```

### `data stats`

Count the saved roots and records.

```console
$ marrow data stats ./proj
roots: 1
records: 2

$ marrow data stats --format json ./proj
{"project":"./proj","records":2,"roots":1}
```

### `data dump`

Print every stored `(path, value)` for inspection: records in identity-key
order, each record's fields in declaration order. Text renders values through
their checked leaf type: strings are quoted and escaped, bytes are `0x<hex>`,
`Id(^store)` references are saved paths, and enum values are module-qualified
member identities. JSON/JSONL carry the checked path plus base64 of the value
bytes. This is not a production backup format.

```console
$ marrow data dump ./proj
^books(1).title	"Small Gods"
^books(1).author	"Terry Pratchett"
^books(1).loanedTo	^authors(1)
^books(1).state	app::Status::archived

$ marrow data dump --format jsonl ./proj
{"path":"^books(1).title","value_b64":"…"}
{"path":"^books(1).author","value_b64":"…"}
{"path":"^books(1).loanedTo","value_b64":"…"}
{"path":"^books(1).state","value_b64":"…"}
{"kind":"summary","records":4}
```

### `data integrity`

Verify each checked, reachable stored value decodes against its declared schema
type, verify required-field completeness for existing records and keyed-layer
entries, verify that canonical identity leaves point to existing saved record
nodes, and verify that no actual stored data cell is left under a root or member
the schema no longer declares. It needs the checked project, so it loads and
checks the source first. It reports decode mismatches (`data.decode`), key type
mismatches (`data.key_type`), dangling identity leaves (`data.dangling_ref`),
missing required fields (`data.incomplete`), orphaned managed cells
(`data.orphan`), and corrupt typed tree-cell keys (`store.corruption`). Exits
`0` on a clean store, `1` when any problem is found.
Pending or defaulted members without an accepted catalog id create no stored-data
completeness obligation.

```console
$ marrow data integrity ./proj
ok: ./proj integrity verified (2 records)
```

### `data recover`

Open the configured native store write-capably so the backend can replay an
interrupted commit after a read-only command reported `store.recovery_required`.
It reads only `marrow.json` to find the store path; it does not check source
files first. A missing native store is treated as nothing to recover and is not
created. An existing file that is not a Marrow store, including an empty file,
is `store.corruption`. If replay/open finds damage beyond recovery, the command
reports the store error such as `store.corruption`.

```console
$ marrow data recover ./proj
store open/repair completed: ./proj/.data/marrow.redb
```

### `data get`

Read one path's value for inspection. The value renders as checked text, like
`dump`: strings are quoted and escaped, bytes are `0x<hex>`, references are
saved paths, and enum values are member identities.
Absence is a valid result (exit `0`): a path with no value but children prints
`(no value; has children)`, a truly absent path prints `(absent)`. An
unparseable path is a usage error (exit `2`).

```console
$ marrow data get ./proj '^books(1).title'
"Small Gods"

$ marrow data get ./proj '^books(1).loanedTo'
^authors(1)

$ marrow data get --format json ./proj '^books(1).title'
{"path":"^books(1).title","presence":"value_only","value_b64":"U21hbGwgR29kcw=="}

$ marrow data get ./proj '^books(99).title'
(absent)
```

---

## `marrow backup`

```
marrow backup [--format text|json|jsonl] <projectdir> <output-file>
```

Write a typed portable backup of a project's saved data. The backup is a Marrow
artifact, not a raw engine-file copy: a small header, a typed manifest, the
accepted-catalog section, and the project's canonical ordered data-cell stream.
The catalog section carries the accepted catalog rows, so a restored store is
self-contained and can render the committed `marrow.catalog.json` artifact. The
manifest binds the data to the program that wrote it — its source digest,
accepted catalog epoch and digest, engine profile, value-codec version,
data-stream digest, store UID, and one integrity checksum over the manifest,
catalog section, and data cells — so a later restore
can refuse data it cannot faithfully reproduce. The manifest fields are
`source_digest`, `catalog_epoch`, `catalog_digest`, `state_digest`, `store_uid`,
reserved-empty `parent_snapshot_digest`, `engine`, `commit`, `record_count`, and
`archive_checksum`; this shape is backup `format_version` 5. The data stream
carries the store's data cells only; the generated indexes are derived, so a
restore rebuilds them rather than replaying them. Commit descriptors carry only
the slim commit stamp, not activation receipt counts or effect digests.

Backup cell targets derive from catalog stable IDs, so backups are
byte-identical only when the accepted catalog facts, engine profile, value
codec, and stored data match. Stable IDs are random opaque values that freeze
when accepted, so divergent catalog histories may still freeze distinct
accepted IDs for source that looks equivalent.

The store is read through one stable snapshot for the backup traversal. Backup
opens the store read-only and never modifies it; a project with no saved data
yet writes a valid empty backup.

The output archive is written to an adjacent temporary file and then renamed over
`<output-file>` only after the complete backup has been written successfully. A
failed backup preserves any prior archive at that path and removes its temporary
file. No overwrite flag is exposed: the path is replaced on success and preserved
on failure.

```console
$ marrow backup ./proj ./proj-backup.mwbackup
ok: backed up 12 record(s) to ./proj-backup.mwbackup
```

Exits `0` on success, `1` if the project does not check, the store cannot be
read, or the output file cannot be written, and `2` on a command-line usage
error.

## `marrow restore`

```
marrow restore [--format text|json|jsonl] [--replace --count N] <projectdir> <backup-file>
```

Replay a backup into the project's native store. Restore checks the project
against the accepted catalog the backup carries, validates the backup against
it (`restore.source_mismatch`, `restore.catalog_mismatch`,
`restore.engine_recompile_required`). By default it refuses a target that
already holds saved data, generated indexes, or an accepted catalog
(`restore.not_empty`), so a normal restore writes into an empty store only.
`--replace --count N` is the explicit destructive mode: restore counts the live
target through the checked data tooling before mutation and proceeds only when
that count equals `N`. A mismatch reports `restore.not_empty` with the expected
and found counts and leaves the target data and catalog unchanged. `--replace`
without `--count`, `--count` without `--replace`, negative or non-integer counts,
and duplicate restore flags are usage errors.

Source mismatch reports print both the backup and project source digests. Catalog
mismatch reports print the backup catalog epoch/digest and the project catalog
epoch/digest. The replay writes the backup's catalog rows alongside its data
cells and mints a fresh store UID, so the restored store carries its accepted
identity and runs immediately. A non-empty `parent_snapshot_digest` is rejected;
v0.1 accepts only the empty reserved sentinel.
The whole replay runs in one transaction: a checksum mismatch or trailing bytes
(`restore.corrupt_chunk`), restored data that does not decode against the schema,
or an orphaned managed cell in the restored stream (`restore.data_invalid`) rolls
the target back to its prior state, so it either gains the whole backup or is
left unchanged. Because the replay is a single transaction, its memory use is
proportional to the backup size — a known v0.1 bound. Restore rebuilds the
generated indexes from the restored data inside the same transaction. In replace
mode the transaction first clears data, generated indexes, accepted catalog rows,
and restore-owned metadata, so a backup without a catalog cannot leave stale
catalog rows behind. A different engine, layout, or codec reports
`restore.engine_recompile_required`; applying that recompile is future work.

```console
$ marrow restore ./proj ./proj-backup.mwbackup
ok: restored 12 record(s) from ./proj-backup.mwbackup
```

```console
$ marrow restore --replace --count 12 ./proj ./proj-backup.mwbackup
ok: restored 12 record(s) from ./proj-backup.mwbackup; receipt: mode=replace expected_live_records=12 replaced_live_records=12
```

JSON and JSONL success output include a `receipt` object. Empty-only restores
report `{"mode":"empty","restored_records":...}`; replace restores report
`{"mode":"replace","expected_live_records":...,"replaced_live_records":...,"restored_records":...}`.

Exits `0` on success, `1` on any validation, checksum, store, or i/o failure, and
`2` on a command-line usage error. See [error-codes.md](error-codes.md) for the
`restore.*` family.
