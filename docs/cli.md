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
marrow data <typed inspection subcommand> <projectdir>
marrow data dump [--format text|json|jsonl] [--limit <n>] \
  [--cursor <opaque>] <projectdir>
marrow backup [--format text|json|jsonl] <projectdir> <output-file>
marrow restore [--format text|json|jsonl] <projectdir> <backup-file>
marrow lsp
marrow serve [--port <port>] <projectdir>
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

Commands that report diagnostics or saved data take `--format`:

- `text` (the default) — human-readable lines. Diagnostics and findings go to
  stderr; primary results go to stdout.
- `json` — one JSON object per tooling report on stdout.
- `jsonl` — one JSON object per line for streaming tooling reports, ending
  with a `{"kind": "summary", …}` line where the report has many records.

Plain `run` output is the program's own `print`/`write` stream, which carries no
envelope. `run --trace`, `run --dry-run`, and `test --trace` accept `--format`
for their tooling reports; when reports compose, more than one top-level JSON
object may appear on stdout. `--format` is also accepted by `check` and typed
`data` subcommands.

---

## `marrow check`

```
marrow check [--data] [--format text|json|jsonl] <file.mw | projectdir>
```

Parse a single `.mw` file, or check a whole project directory, and report
diagnostics.

- Given a `.mw` file, it parses and checks that file in isolation. Module-wide
  rules that need a project are not applied.
- Given a project directory, it loads `marrow.json` and runs the project checker
  over every source root plus configured test files: parse, type, effect, and
  durable-place checks.
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
evolution against the accepted catalog. The accepted catalog file is published
only after the store commit carries verifiable activation evidence for defaults,
transforms, retires, and rebuilt indexes. That evidence is bounded receipt data:
digests, affected IDs, counts, approvals, commit IDs, and source/catalog/engine
facts, not proposal catalog bodies or executable migration history. Destructive
retire needs
`--maintenance` and an approval whose catalog ID and populated count match the
preview.

---

## `marrow fmt`

```
marrow fmt [--check | --write] <file.mw | projectdir>
```

Format Marrow source. `marrow fmt` does not read from stdin.

- A single `.mw` file with no flag prints the formatted source to stdout.
- `--check` reports each file that is not already formatted and exits `1` if any
  differ; it writes nothing.
- `--write` rewrites changed files in place.
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
selects. The store is the configured backend — a `native` redb store on disk, or
an in-memory store when none is configured (see
[project-config.md](project-config.md)). A project must check cleanly before it
runs.

A clean run records the project's baseline durable identity if it has none yet:
the first run of a project with a durable surface writes the accepted catalog
file transparently before touching the store. A project already past its
baseline proposes no change and the file is left untouched. There is no separate
acceptance step. See [data-evolution.md](data-evolution.md).

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

`--dry-run` runs the entry, reports the saved-data writes it would commit, then
rolls them back. No saved data changes: the run rides one outer savepoint that is
always rolled back, so managed writes inside `transaction` blocks stage and then
discard with the rest. The guarantee is logical saved-data stability — the same
records read back afterward — not native-file byte identity, since aborting the
store transaction can still rewrite backend metadata. Only saved data is rewound;
host side effects such as `std::io` writes or `std::log` lines are not.

`--dry-run` takes `--format`. The report is tooling output on stderr under every
format, off the program's stdout stream. Under text, planned writes are
`would write <path>` / `would delete <path>` lines and a
`dry run: N write(s), M delete(s) (rolled back)` summary. Under `json`/`jsonl`,
the report is a `{"committed": false, "planned": […]}` envelope whose planned
entries carry the op, human path, and base64 value bytes.

`--trace` composes with `--dry-run`: the run is traced and its writes are then
discarded. The trace and the dry-run report both go to stderr — under `--format
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

Each result is printed as `ok`, `FAIL` (a `std::assert::*` failure, code
`run.assertion`), or `ERROR` (any other runtime error), located at the test's
source position, followed by a summary line.

Exits `0` only when every test passes. It exits `1` if any test fails or errors,
if the project does not check, or if no test is found (`test.none`).

With `--trace`, every test runs under an execution trace attributed to that test
by name. The trace events have the same text/json/jsonl shapes as `run --trace`,
and each event carries the test label so consumer tooling can group it. The trace
is tooling output on stderr; the test runner's `ok`/`FAIL`/`ERROR` lines and
summary stay on stdout, so the two streams never interleave.

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
`marrow data integrity`, dry-run reports, LSP/editor features, or future
accepted tooling surfaces. They are not exposed as query-plan, optimizer, or
standalone explanation output.

Read-only diagnostic/admin/operator inspection of a project's saved data. The
v0.1 decision is to keep `get` and `dump` as `marrow data` subcommands, not
production app APIs. They never create or modify the store; a project with no
saved data on disk reports as empty. `get` is exact-path and point-bounded.
`dump` is snapshot-bound and must stream or page rather than materializing
unbounded data. See
[data-tools.md](data-tools.md) for full output shapes and the path syntax. These
commands are not production app APIs and not a production backup/restore format.

`data diff` and `data load` are deferred — see
[future/data-tools.md](future/data-tools.md).

All `data` commands exit `2` on a usage error (missing directory, bad flag, an
unparseable `<path>` for `get`), and `1` on a config or store error. `roots`,
`stats`, `dump`, and `get` exit `0` otherwise; `integrity` exits `1` when it
finds a problem.

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

Print every stored `(path, value)` in encoded order for inspection. Values
render as canonical payload bytes — UTF-8 text when valid, else `0x<hex>`.
JSON/JSONL carry the checked path plus base64 of the value bytes. This is not a
production backup format.

```console
$ marrow data dump ./proj
^books(1).author	Terry Pratchett
^books(1).title	Small Gods

$ marrow data dump --format jsonl ./proj
{"path":"^books(1).author","value_b64":"…"}
{"path":"^books(1).title","value_b64":"…"}
{"kind":"summary","records":2}
```

### `data integrity`

Verify each checked, reachable stored value decodes against its declared schema
type, and verify that no actual stored data cell is left under a root or member
the schema no longer declares. It needs the checked project, so it loads and
checks the source first. It reports decode mismatches (`data.decode`), key type
mismatches (`data.key_type`), orphaned managed cells (`data.orphan`), and corrupt
typed tree-cell keys (`store.corruption`). Exits `0` on a clean store, `1` when
any problem is found.

Integrity walks the values that are actually stored. It does not verify
required-field completeness: a record missing a required field has no stored
cell to flag, so an incomplete record passes integrity. Completeness is enforced
on the write path and by data evolution.

```console
$ marrow data integrity ./proj
ok: ./proj integrity verified (2 records)
```

### `data get`

Read one path's value for inspection. The value renders as canonical payload
bytes, like `dump`.
Absence is a valid result (exit `0`): a path with no value but children prints
`(no value; has children)`, a truly absent path prints `(absent)`. An
unparseable path is a usage error (exit `2`).

```console
$ marrow data get ./proj '^books(1).title'
Small Gods

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
artifact, not a raw engine-file copy: a small header, a typed manifest, and the
project's canonical ordered data-cell stream. The manifest binds the data to the
program that wrote it — its source digest, accepted catalog epoch, engine
profile, value-codec version, and a checksum over the cell stream — so a later
restore can refuse data it cannot faithfully reproduce. The backup carries the
store's data cells only; the generated indexes are derived, so a restore rebuilds
them rather than replaying them.
When the store has activation receipts, the manifest carries the receipt facts as
evidence only. It records proposal/catalog digests, affected IDs, counts, and
bounded effect digests, including retire receipt digests, never proposal catalog
bodies or per-record default ledgers.

Backup cell targets derive from catalog stable IDs, so backups are
byte-identical only when the accepted catalog facts, engine profile, value
codec, and stored data match. Stable IDs are random opaque values that freeze
when accepted, so divergent catalog histories may still freeze distinct
accepted IDs for source that looks equivalent.

The store is read through one stable snapshot for the backup traversal. Backup
opens the store read-only and never modifies it; a project with no saved data
yet writes a valid empty backup.

```console
$ marrow backup ./proj ./proj-backup.mwbackup
ok: backed up 12 record(s) to ./proj-backup.mwbackup
```

Exits `0` on success, `1` if the project does not check, the store cannot be
read, or the output file cannot be written, and `2` on a command-line usage
error.

## `marrow restore`

```
marrow restore [--format text|json|jsonl] <projectdir> <backup-file>
```

Replay a backup into the project's native store. Restore compiles the project,
validates the backup against it (`restore.source_mismatch`,
`restore.catalog_mismatch`, `restore.engine_recompile_required`), and refuses a
non-empty target (`restore.not_empty`) — v0.1 restores into an empty store only.
The whole replay runs in one transaction: a checksum mismatch or trailing bytes
(`restore.corrupt_chunk`), restored data that does not decode against the schema,
or an orphaned managed cell in the restored stream (`restore.data_invalid`) rolls
the target back to empty, so it either gains the whole backup or is left
unchanged. Because the replay is a single transaction, its memory use is
proportional to the backup size - a known v0.1 bound. Restore rebuilds the
generated indexes from the restored data inside the same transaction. A different
engine, layout, or codec reports `restore.engine_recompile_required`; applying
that recompile is future work.

```console
$ marrow restore ./proj ./proj-backup.mwbackup
ok: restored 12 record(s) from ./proj-backup.mwbackup
```

Exits `0` on success, `1` on any validation, checksum, store, or i/o failure, and
`2` on a command-line usage error. See [error-codes.md](error-codes.md) for the
`restore.*` family.

---

## `marrow lsp`

```
marrow lsp
```

Run the Marrow language server over stdio: JSON-RPC 2.0 with `Content-Length`
framing. It handles the `initialize`/`shutdown`/`exit` lifecycle, tracks open
documents, and publishes diagnostics for open `.mw` documents on every
`didOpen`/`didChange`. With a valid project `rootUri`, diagnostics come from the
project checker using open-buffer overlays; otherwise they fall back to parsing
the open buffer. Point an LSP-capable editor at this command; it is not meant to
be run by hand. It takes no arguments — any flag other than `--help` is a usage
error (exit `2`).

This is the editor language server, distinct from `marrow serve`.

---

## `marrow serve`

```
marrow serve [--port <port>] <projectdir>
```

Run the Marrow debug/admin loopback inspection server. It answers
newline-delimited JSON requests over a loopback (`127.0.0.1`) TCP connection,
reads checked saved data, and never writes managed data. It is not a production
app server, sync protocol, generated API, or remote database.

The bound address is printed on startup, then the server blocks accepting
connections one at a time:

```console
$ marrow serve --port 0 ./proj
marrow serve listening on 127.0.0.1:52224
```

`--port` chooses the TCP port; `--port 0` (the default) lets the OS pick a free
port, printed on the line above. The server runs until interrupted. Its v0.1
protocol exposes `debug_data_roots`, `debug_data_get`, `debug_data_children`,
and `debug_data_walk` for local inspection. See
[serve-protocol.md](serve-protocol.md).

Exits `2` on a usage error, `1` if the project config cannot be loaded, the store
cannot be opened, or the address cannot be bound.
