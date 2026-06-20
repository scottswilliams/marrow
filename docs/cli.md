# CLI Reference

The `marrow` binary is the single entry point for the language and its built-in
database.

```
marrow init <projectdir>
marrow check [--format text|json|jsonl] <projectdir>
marrow doctor [--format text|json|jsonl] <projectdir>
marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>
marrow evolve apply [--maintenance] [--approve-retire <catalog-id>:<count>] \
  [--backup <path> | --no-backup] [--format text|json|jsonl] <projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow run [--entry <entry>] [--arg name=value]... [--maintenance] \
  [--trace] [--dry-run] [--format text|json] <projectdir>
marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>
marrow surface serve [--write] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
marrow data <roots|stats|dump|integrity> [--backup <artifact>] [--format text|json|jsonl] <projectdir>
marrow data recover [--format text|json|jsonl] <projectdir>
marrow data get [--backup <artifact>] [--format text|json|jsonl] <projectdir> <path>
marrow backup <projectdir> <output-file>
marrow restore [--replace --count N] <projectdir> <backup-file>
marrow --version
marrow --help
```

A project directory contains a `marrow.json`; see
[project-config.md](project-config.md) for its fields. Every subcommand accepts
`--help` (or `-h`) and prints its own usage.

## Version

```
marrow --version
```

Print the CLI version and the storage engine profile tuple this binary writes:

```console
$ marrow --version
marrow 0.1.0 engine-profile=(key=v0, layout-epoch=0, digest=77944eb86c08b665)
```

The tuple names the key profile version, layout epoch, and engine-profile
digest. It is the same profile used by activation fencing and commit stamps.

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
  stderr unless a command owns a report stream such as `doctor`; primary
  results go to stdout.
- `json` — one JSON object for the command's structured report.
- `jsonl` — one JSON object per line for streaming reports, ending with a
  `{"kind": "summary", …}` line where the report has many records.

Plain `run` text output is the program's own `print` stream on stdout. With
`run --format json`, stdout becomes a result envelope that carries the captured
program output separately from the rendered return value. `run --dry-run`
accepts `--format text|json` for its tooling report, written to stderr.
`run --trace` is text-only and does not accept an explicit `--format` unless it
is combined with `--dry-run`.

`marrow test --format json|jsonl` shapes the test pass/fail report on stdout.
With `--trace`, the trace is a separate text stream on stderr while the test
report stays on stdout.

Structured JSON reports that include a `project` field render the canonical
absolute project directory, equivalent to `std::fs::canonicalize(<projectdir>)`,
not the raw directory argument.

---

## `marrow init`

```
marrow init <projectdir>
```

Create a new project directory with the quickstart scaffold: `marrow.json`,
`src/<name>/books.mw`, and `tests/books_test.mw`, where `<name>` is the target
directory's final path component.

The target directory must not already exist. Its final path component must parse
as one Marrow module identifier segment, because the scaffold uses it in
`run.defaultEntry`, `module <name>::books`, and `use <name>::books`.

The generated config is explicit: `sourceRoots` is `["src"]`,
`run.defaultEntry` is `<name>::books::main`, the store is
`{"backend":"native","dataDir":".marrow/data"}`, and `tests` is `["tests"]`.
No `.gitignore` or extra project files are created.

Exits `0` after writing the scaffold, `1` if the target name is invalid or the
target cannot be written safely, and `2` for usage errors.

```console
$ marrow init shelf
created shelf

$ cd shelf
$ marrow check .
ok: . checked
```

---

## `marrow check`

```
marrow check [--format text|json|jsonl] <projectdir>
```

Check a project directory containing `marrow.json` and report diagnostics.

- It loads `marrow.json` and runs the project checker over every source root
  plus configured test files: parse, type, effect, and durable-place checks. It
  binds durable identity from the committed
  `marrow.catalog.json` artifact, repairing that file from a committed store
  snapshot when the local store already has one; it never creates the store or
  freezes identity.
- Passing a bare `.mw` file is a usage error. Run `marrow check` on the project
  directory that contains `marrow.json`.
- When `marrow.json` sets `run.defaultEntry`, the check verifies it names a
  runnable zero-argument entry. A missing, private, ambiguous, or parameterized
  default entry is a `check.default_entry` error rather than a run-time fault.
  `marrow doctor` inherits this check.
- On successful `json` or `jsonl` checks, the report includes
  `entry_footprints`, `surface_abi`, and `surface_routes`. `surface_routes` is
  the `surface.route.v1` manifest derived from exported surface descriptors:
  JSON `POST` operation-tag paths plus render aliases and request-body kinds.
  The manifest is data; `marrow surface serve` is the local serving profile that
  consumes it. Generated clients, create/delete profiles, remote serving, and
  opaque cursor tokens remain out of scope.

Exits `0` when there are no errors, `1` when there are diagnostics or
`marrow.json` cannot be read, and `2` for usage errors such as a non-directory
target.

```console
$ marrow check ./proj
ok: ./proj checked

$ marrow check --format json ./proj
{"project":"/absolute/path/to/proj","status":"failed","diagnostics":[{"code":"parse.syntax", …}]}
```

A failing check returns exit `1`:

```console
$ marrow check ./proj
./proj/src/broken.mw:1:1: error: parse.syntax: expected function parameter list
$ echo $?
1
```

---

## `marrow surface serve`

```
marrow surface serve [--write] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
```

Run the local HTTP serving profile for checked application surfaces. By
default the command opens the project through `ProjectSurfaceReadSession` and
serves read routes only. With `--write`, it opens `ProjectSurfaceSession` and
also exposes sparse-update and action routes. Both modes require an already
accepted native store and never create, freeze, migrate, repair, or auto-apply
saved data.

- The listener binds only loopback addresses. The default is
  `127.0.0.1:8080`; tests and tooling can pass `--addr 127.0.0.1:0` to let the
  OS choose a loopback port.
- `--cors-origin` enables browser CORS for one exact loopback origin such as
  `http://localhost:5173`, `http://127.0.0.1:5173`, or
  `http://[::1]:5173`. Non-loopback origins, URL paths, and wildcards are
  usage errors. Without this option, the server emits no CORS headers.
- On startup the command prints
  `surface serve listening on http://<addr>` to stdout, then handles requests
  until the process exits.
- The active route set is derived from the same `surface.route.v1` manifest
  exported by `marrow check --format json|jsonl`. Default mode serves only
  `/surface/v1/read/<operation-tag>` rows; `--write` additionally serves
  `/surface/v1/update/<operation-tag>` and
  `/surface/v1/action/<operation-tag>` rows.
- `--write` is single-owner and sequential through the native writer lock while
  the process is running. It excludes another writer and read-only inspection
  handle for the same store file.
- Served actions run with zero host capabilities. Actions that require clock,
  environment, logging, filesystem, or other host capabilities fail closed as
  `surface.action`; explicit-host action execution is a linked-Rust embedding
  API, not this HTTP profile.
- Operation requests must be HTTP/1.0 or HTTP/1.1 `POST` with
  `Content-Type: application/json`, exactly one `Content-Length`, no
  `Transfer-Encoding`, bounded headers/body, no query string, and an exact
  operation-tag path. The JSON body is a `surface.operation.v1` envelope whose
  `operation_tag` and request kind must match the selected route.
- With `--cors-origin`, matching browser preflight `OPTIONS` requests over a
  served route return `204` and `Access-Control-Allow-Origin` for that exact
  origin. Mismatched origins return `403` and no CORS allow-origin header.
- The server processes at most one request per connection, rejects trailing
  bytes already buffered after the declared body, returns `Connection: close`,
  and never reads a second request from the connection.
- Responses are JSON. Success returns the operation response envelope. Failures
  return a sanitized `{ "code": "surface.*", "message": "..." }` envelope with
  no source path, store path, or raw backend detail.

This is a dependency-free local tooling profile, not remote hosting,
authentication, generated clients, opaque cursor tokens, or create/delete CRUD.
Exits `2` for usage errors such as non-loopback `--addr` or `--cors-origin`,
`1` for project/session/listener failures, and otherwise runs until killed.

---

## `marrow doctor`

```
marrow doctor [--format text|json|jsonl] <projectdir>
```

Inspect a project for operator triage without repairing or writing anything.
`doctor` aggregates independent probes where possible:

- load `marrow.json`;
- validate the accepted `marrow.catalog.json` artifact digest;
- run the normal project check summary;
- open the configured native store read-only when a store file exists;
- report store lock/recovery/open failures as findings instead of stopping
  unrelated probes;
- read the store UID, commit stamp, current engine profile tuple, and activation
  fence classification;
- sample saved-data integrity with
  `DOCTOR_INTEGRITY_SAMPLE_LIMIT = 64` as one shared traversal cap.

`doctor` never creates the native data directory, never opens a write-capable
store handle, never renders or repairs `marrow.catalog.json`, and never runs the
full unbounded `marrow data integrity` scan.

Text and JSONL render one finding per line. Text output also prints a
non-finding guidance line when the integrity sample is truncated, naming the
full read-only `marrow data integrity` command to run next. JSON renders one
envelope:

```json
{
  "project": "/absolute/path/to/proj",
  "status": "findings",
  "findings": [
    {
      "code": "doctor.store_locked",
      "kind": "tooling",
      "message": "native store is locked",
      "remedy": "close the process holding the native store, then rerun the next command",
      "next_command": "marrow doctor ./proj",
      "data": {
        "underlying_code": "store.locked",
        "message": "the store file is held open by another process (a writer or a read-only inspection): /absolute/path/to/proj/.data/marrow.redb. Close the other process, then retry",
        "store": "/absolute/path/to/proj/.data/marrow.redb"
      },
      "source_span": null
    }
  ],
  "store": null,
  "fence": null,
  "integrity_sample": { "limit": 64, "items_checked": 0, "problems": 0, "truncated": false }
}
```

When the store opens, the JSON `store` object carries the stamp classification
(`stamped` or `unstamped`), store UID, commit metadata, and current engine
profile tuple. When the checked project and store are both available, `fence`
reports the activation-fence classification.

Exits `0` when no findings are reported, `1` when one or more findings are
reported, and `2` for usage errors.

---

## `marrow evolve`

```
marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>
marrow evolve apply [--maintenance] [--approve-retire <catalog-id>:<count>] \
  [--backup <path> | --no-backup] [--format text|json|jsonl] <projectdir>
```

`evolve preview` opens the configured store read-only, discharges source,
accepted catalog metadata, store snapshot, and engine metadata into an exact
witness, then reports the counts and blocking diagnostics. With
`--from-backup <artifact>`, preview validates the backup artifact, mounts it in
memory, and derives the witness from that point-in-time data instead of opening
the configured store; the mount is read-only and does not restore, activate, or
write catalog files. With `--scaffold`, text output is formatter-produced `.mw`
source containing one ready-to-paste `evolve` block per repairable obligation;
it never edits project source. JSON and JSONL keep the preview envelope and
include the scaffold string.

`evolve apply` recomputes that preview witness over the live project and store,
requires an exact match, checks the activation window, and commits the data work
plus metadata stamp in one transaction. Like `run`, it records a project's
baseline durable identity first when the project has none yet, then applies the
evolution against the accepted catalog. The advanced accepted catalog rows commit
in that same store transaction as the data work and the slim commit stamp, so
the catalog never advances without the data behind it; after that commit, the
CLI renders `marrow.catalog.json` from the committed store snapshot. Any
Retire-bearing apply also requires either `--backup <path>` or `--no-backup`: a
backup is written through the typed atomic backup path and validated before
apply mutates the store, while `--no-backup` records the explicit opt-out in the
rendered receipt. Evolve refuses backup paths that resolve to managed project
artifacts or subtrees: `marrow.json`, `marrow.catalog.json`, source roots, test
paths, and the native data directory/store file. The command output still
renders receipt counts for defaults, transforms, retires, rebuilt indexes, and
recovery-point choice, but those counts are not persisted in commit metadata.
Destructive retire also needs `--maintenance` and an approval whose catalog ID
and populated count match the preview.

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

All three modes agree on losslessness. A comment the formatter cannot re-emit —
one stranded on a continuation line inside an open delimiter — is refused
(`fmt.comment_loss`, exit `1`) in every mode, including the default stdout mode,
which prints nothing rather than emit comment-stripped source. `marrow fmt
file > file` therefore never silently discards content.
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

Exit codes: `0` formatted/already-formatted; `1` a `--check` file differs, a
format would discard retained comments, or a file failed to parse or write; `2` a
directory with no `--check`/`--write`, an unknown flag, or a `-` stdin argument.

---

## `marrow run`

```
marrow run [--entry <entry>] [--arg name=value]... [--maintenance] \
  [--trace] [--dry-run] [--format text|json] <projectdir>
```

Check a project, then run an entry function over the store its `marrow.json`
selects (see [project-config.md](project-config.md)). A project must check
cleanly before it runs. The explicit memory backend admits only a program with no
durable declarations; a program that declares a durable surface (a `resource`,
a saved `store`, or an `enum`) needs a configured `native` store and otherwise
fails with `run.durable_store_required`. Omitting `store` is a `config.invalid`
project configuration error.

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
(`run.store_unstamped`); run `marrow evolve preview` to inspect the required
work and `marrow evolve apply` to stamp it first. When the source's shape
drifted from the stamped schema, a change that mutates no stored records (such
as adding a sparse field) is auto-applied through the production apply path and
the run proceeds against the advanced catalog; a change that would backfill,
transform, or destructively drop populated data is refused with
`run.schema_drift`, naming the `marrow evolve apply` step that discharges it.

The entry is `--entry` if given, otherwise the project's `run.defaultEntry`.
Qualified entries (`module::function`) resolve exactly. A bare entry name is
accepted only when it names one public function in the checked program; ambiguous
bare names fail with `run.ambiguous_function`. If neither entry source is
present, `run` fails with `run.no_entry` (exit `1`).

`--arg name=value` supplies one entry parameter value. Repeat `--arg` in argv
order. The CLI parser only splits at the first `=`; signature-driven decoding
belongs to the checked entry call. Scalar and enum arguments use the same
textual spellings accepted by runtime literals and checked enum facts. `string`
values are the raw text after the first `=`. Sequence parameters whose element
type is scalar or enum collect repeated values in argv order; `--arg name=[]`
spells an empty sequence. Single-component `Id(^store)` parameters decode
through the same identity-key guards used by saved data. Composite identity
keys, resource-shaped parameters, group entries, local trees, and other
unsupported entry surfaces fail with `run.entry_argument` (exit `1`). There is
no `--args-json`; it is an unknown option and exits `2`.

Output written with `print` goes to stdout. `std::log` output goes to
stderr. The run reads the real system clock, environment, and filesystem.

`--maintenance` grants the run the maintenance capability for data evolution and
repair tooling. It permits whole managed-root deletes and required-field deletes
that the default run rejects. An operator must type it; the default run and
`run.defaultEntry` can never inject it. Use it deliberately.

`--trace` reports each statement as it runs — file, line, call depth, and the
visible locals — and each managed write or delete, in execution order. The trace
is a text-only tooling stream on stderr, leaving the program's stdout for its
own `print` output. Combining `--trace` with any explicit `--format` is a usage
error unless `--dry-run` is also present.

In the human-readable text of a `--trace` or `--dry-run` write, the leaf value is
rendered as its declared typed scalar, not as raw codec bytes: a `bool` reads
`true`/`false`, an int/date/duration/instant reads its canonical typed text. The
machine-readable `value_b64` field in dry-run JSON output stays the raw stored
bytes.

`--format json` on a non-dry run moves the program's `print` stream into the
`output` field of a stdout envelope. The envelope also carries `return` when the
entry's return value has a JSON surface, `signature_digest: null`,
`raises: null`, and `store_stamp` with `store_uid`, `catalog_epoch`, and
`commit_id`. A sibling `committed: true` appears only when this invocation
committed a write; read-only runs omit `committed`. Identity returns use the
same JSON identity form as `marrow data` JSON surfaces. Resource-shaped returns
are outside the run surface and fail with `run.entry_surface` (exit `1`). If
return rendering or a later runtime fault fails after a durable write has
committed, stderr carries the runtime fault JSON with `store_stamp` and
`committed: true`; stdout does not carry a successful result envelope. If an
uncaught `Error` reaches the top of a JSON run, stderr carries the runtime fault
JSON and includes the original error code as `data.code`.

`--dry-run` classifies the run through the checked project and store fences
without freezing first-run durable identity into the native store and without
auto-applying zero-mutation schema drift. If a real run would freeze the
baseline, apply schema drift, or fence, the dry-run report contains
tooling content for that action and exits `0`; JSON reports spell these booleans
as `would_freeze`, `would_apply`, and `would_fence`. When a fence would not
pass, the entry is not executed. Otherwise the entry runs against an isolated
store, so user `transaction` blocks cannot consume the dry-run boundary. Only
saved data is isolated; host side effects such as `std::io` writes or
`std::log` lines are not.

`--dry-run` takes `--format text|json`. The report is tooling output on stderr
under every format, off the program's stdout stream. Under text, planned writes
are `would write <path>` / `would delete <path>` lines, followed by per-target
create/write/delete counts and a `dry run: N write(s), M delete(s) (not
committed)` summary. Under `json`, the report object contains `committed`,
`writes`, `deletes`, `messages`, `would_freeze`, `would_apply`, `would_fence`,
`planned`, and `write_counts`. Planned entries carry the op, human path, base64
value bytes, and a structured `target`. Target identities, index keys, and keyed
data path segments use the same typed saved-key JSON objects as `marrow data`.
`write_counts.roots` and `write_counts.indexes` are objects keyed by root or
index name; each leaf is `{ "creates": N, "writes": N, "deletes": N }`. `creates`
counts records the run would newly create: a record establishes one create
regardless of how many field assignments touch it, and a write to a record that
already exists is a write, not a create. The `writes`/`deletes` summary equals
the sum of the per-target counts.

`--trace` composes with `--dry-run`: the run is traced while its saved writes are
isolated from the configured store. This composition is text-only: trace events
and the dry-run report both go to stderr, and the program's own stdout output
stays uninterrupted. For source-native data evolution use `marrow evolve
preview`; `run --maintenance --dry-run` is for
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
marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>
```

Check a project, then run its tests: every `pub fn` with no parameters in a test
file selected by the `tests` paths in `marrow.json`. Each test runs against a
fresh in-memory store. A test's `std::log` output is discarded so it stays out
of the report.

In text format, each result is printed as `ok`, `FAIL` (a `std::assert::*`
failure, code `run.assertion`), or `ERROR` (any other runtime error), located at
the test's source position, followed by a summary line.
`--filter <substring>` runs only tests whose qualified name contains the
substring; a filter that selects nothing fails with `test.none`.

Under `--format json`, stdout is one test report envelope:

```json
{"project":"/absolute/path/to/proj","tests":[{"kind":"test","name":"tests::smoke_test::add_runs","outcome":"passed","file":"tests/smoke_test.mw","span":{"line":1,"column":1}}],"summary":{"total":1,"selected":1,"passed":1,"failed":0,"errored":0}}
```

Under `--format jsonl`, stdout is one test-result record per line followed by a
summary record:

```jsonl
{"kind":"test","name":"tests::smoke_test::add_runs","outcome":"passed","file":"tests/smoke_test.mw","span":{"line":1,"column":1}}
{"kind":"summary","total":1,"selected":1,"passed":1,"failed":0,"errored":0}
```

Failed and errored JSON records also carry the runtime fault `code` and an
`output` field. `output` is the test's bounded pre-fault `print` output as a
string, or `null` when the test produced no output. Passing records omit
`output`. Passing result spans point at the test function declaration; failed
and errored result spans point at the runtime fault.

Exits `0` only when every test passes. It exits `1` if any test fails or errors,
if the project does not check, or if no test is found (`test.none`).

With `--trace`, every test runs under an execution trace attributed to that test
by name. The trace is tooling output on stderr; the test report stays on stdout,
so the two streams never interleave. Trace events are text-only and stream as
they run; combining `--trace` with `--format json|jsonl` is a usage error.

```console
$ marrow test ./proj
ok    tests::smoke_test::add_runs
FAIL  tests::shelf_test::title_is_set
      tests/shelf_test.mw:7:5: run.assertion: assertion failed: isTrue(false)

2 tests: 1 passed, 1 failed, 0 errored
$ echo $?
1
```

The implemented assertions are `std::assert::isTrue`, `isFalse`, `equal`,
`absent`, and `fail`.

## `marrow data`

`marrow data` is the typed inspection and repair-tooling boundary. It must read
through checked source, accepted catalog metadata, and typed tree-cell store
APIs. It does not expose raw backend keys, raw saved-path encoders, or archive
streams as production CLI behavior.

There is no `marrow explain` command in v0.1. Checked access, path, and name
facts are internal compiler/tooling facts surfaced through diagnostics,
`marrow data integrity`, dry-run reports, editor features, or future
accepted tooling surfaces. They are not exposed as optimizer or standalone
explanation output.

Diagnostic/admin/operator access to a project's saved data. The v0.1 decision is
to keep `get` and `dump` as `marrow data` subcommands, not production app APIs.
The inspection subcommands never create or modify the store; a project with no
saved data on disk reports as empty. `recover` is the only write-capable `data`
subcommand: it opens an existing native store so the backend can replay an
interrupted commit. `get` is exact-path and point-bounded. `dump` is
snapshot-bound and must stream or page rather than materializing unbounded data.
`roots`, `stats`, `dump`, `integrity`, and `get` also accept
`--backup <artifact>` to inspect a validated backup through an ephemeral
in-memory mount instead of opening the configured store; `recover` does not
accept that flag.
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

$ marrow data roots --format json ./proj
{"project":"/absolute/path/to/proj","roots":["books"],"store_snapshot":{"store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"checked_source_digest":"sha256:..."}}
```

`store_snapshot` is `null` when no store-backed read occurs. Inside a present
snapshot, store metadata fields such as `store_uid`, `catalog_digest`, and
`commit` may be `null` when the store has not recorded that metadata.

### `data stats`

Count the saved roots, records, and cells. One record is one saved entity, an
identity tuple such as `^books(1)`; one cell is one stored `(path, value)` pair.
The record count is the same number `marrow backup` reports, `restore --replace
--count N` confirms, and evolution counts; the cell count matches the `data dump`
line count.

```console
$ marrow data stats ./proj
roots: 1
records: 1
cells: 2

$ marrow data stats --format json ./proj
{"project":"/absolute/path/to/proj","records":1,"cells":2,"roots":1}
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
{"kind":"summary","cells":4}
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
ok: ./proj integrity verified (2 cells)
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
{"path":"^books(1).title","presence":"value_only","value_b64":"U21hbGwgR29kcw==","store_snapshot":{"store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"checked_source_digest":"sha256:..."}}

$ marrow data get ./proj '^books(99).title'
(absent)
```

`store_snapshot` is `null` when the read has no store-backed version. Inside a
present snapshot, store metadata fields such as `store_uid`, `catalog_digest`,
and `commit` may be `null` when the store has not recorded that metadata.

---

## `marrow backup`

```
marrow backup <projectdir> <output-file>
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
`archive_checksum`; this shape is backup `format_version` 6. The data stream
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

The reported count is the saved records (entities), the same number `data stats
records:` reports and `restore --replace --count N` confirms.

```console
$ marrow backup ./proj ./proj-backup.mwbackup
ok: backed up 12 record(s) to ./proj-backup.mwbackup
```

Exits `0` on success, `1` if the project does not check, the store cannot be
read, or the output file cannot be written, and `2` on a command-line usage
error.

## `marrow restore`

```
marrow restore [--replace --count N] <projectdir> <backup-file>
```

Replay a backup into the project's native store. Restore checks the project
against the accepted catalog the backup carries, validates the backup against
it (`restore.source_mismatch`, `restore.catalog_mismatch`,
`restore.engine_recompile_required`). By default it refuses a target that
already holds saved data, generated indexes, or an accepted catalog
(`restore.not_empty`), so a normal restore writes into an empty store only.
`--replace --count N` is the explicit destructive mode: restore counts the live
target's saved records (entities, the same count `data stats records:` reports)
before mutation and proceeds only when that count equals `N`. A mismatch reports
`restore.not_empty` with the expected and found record counts and leaves the
target data and catalog unchanged. `--replace`
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

Exits `0` on success, `1` on any validation, checksum, store, or i/o failure, and
`2` on a command-line usage error. See [error-codes.md](error-codes.md) for the
`restore.*` family.
