# CLI Reference

The `marrow` binary is the single entry point for the language and its built-in
database.

```
marrow check [--format text|json|jsonl] <file.mw | projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow run [--entry <entry>] [--maintenance] [--trace] [--dry-run] \
  [--format text|json|jsonl] <projectdir>
marrow test [--trace] [--format text|json|jsonl] <projectdir>
marrow data <roots|stats|dump|integrity> <projectdir>
marrow data get <projectdir> <path>
marrow explain [--format text|json|jsonl] <projectdir> <target>
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
object may appear on stdout. `--format` is also accepted by `check`, `explain`,
and every `data` subcommand; for `data roots`, `data stats`, and `data get`,
`jsonl` emits the same single object as `json` (there is nothing to stream).

---

## `marrow check`

```
marrow check [--format text|json|jsonl] <file.mw | projectdir>
```

Parse a single `.mw` file, or check a whole project directory, and report
diagnostics.

- Given a `.mw` file, it parses and checks that file in isolation. Module-wide
  rules that need a project are not applied.
- Given a project directory, it loads `marrow.json` and runs the project checker
  over every source root plus configured test files: parse, type, effect, and
  saved-path checks.

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
visible locals — and each managed write or delete, in execution order. Under text
the trace is an indented stream on stderr, leaving the program's stdout alone.
Under `json`/`jsonl` it emits `step` records and managed-write `write` records.

`--dry-run` runs the entry, reports the saved-data writes it would commit, then
rolls them back. The store is left byte-for-byte unchanged: the run rides one
outer savepoint that is always rolled back, so managed writes inside
`transaction` blocks stage and then discard with the rest. Only saved data is
rewound; host side effects such as `std::io` writes or `std::log` lines are not
rolled back.

`--dry-run` takes `--format`. Under text, planned writes are reported on stderr
as `would write <path>` / `would delete <path>` lines and a
`dry run: N write(s), M delete(s) (rolled back)` summary. Under `json`/`jsonl`,
the report is a `{"committed": false, "planned": […]}` envelope whose planned
entries carry the op, human path, and base64 value bytes.

`--trace` composes with `--dry-run`: the run is traced and its writes are then
discarded. Under `--format json`, stdout receives the trace object followed by
the dry-run envelope as separate top-level JSON objects. This previews explicit
maintenance or data-evolution work:
`marrow run --dry-run --maintenance --entry evolve::main ./proj`.

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
and each event carries the test label so consumer tooling can group it. The test
runner still prints its normal `ok`/`FAIL`/`ERROR` lines and summary on stdout;
under `--format json` or `jsonl`, those text lines appear after each test's trace
report.

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

---

## `marrow explain`

```
marrow explain [--format text|json|jsonl] <projectdir> <target>
```

Statically explain a target without running code. The target is either a saved
`^path` or a name. Saved-path explanation is a diagnostic/admin inspection
surface; typed production previews are deferred to the Lane 10 tooling protocol.

A `^path` target reports its path/index plan: the root and resource it names,
the resolved class — a scalar leaf and its type, a generated index entry, a
key-type mismatch, or an orphan — and, for a field, the indexes it participates
in. The classification is the same one `data integrity` applies per record, so
explain and integrity agree on what each path means.

A name target reports its resolution through the same resolver the checker and
runtime use: found (with owning module and kind), ambiguous (with candidate
modules), not visible (a private function reached by a qualified path), or
unresolved.

```console
$ marrow explain ./proj '^books(1).title'
^books(1).title resolves to field `title` of resource Book, type string
index plan: covered by `byTitle`(title) unique

$ marrow explain --format json ./proj shelf::add
{"target":"shelf::add","kind":"name","resolution":"found","module":"shelf","resolved_kind":"function"}
```

Exits `0` when it can explain the target, `1` if the project does not check, and
`2` on command-line usage errors or a malformed saved-path target.

## `marrow data`

```
marrow data roots     [--format text|json|jsonl] <projectdir>
marrow data stats     [--format text|json|jsonl] <projectdir>
marrow data dump      [--format text|json|jsonl] <projectdir>
marrow data integrity [--format text|json|jsonl] <projectdir>
marrow data get       [--format text|json|jsonl] <projectdir> <path>
```

Read-only diagnostic/admin inspection of a project's saved data. It never
creates or modifies the store; a project with no saved data on disk reports as
empty. See [data-tools.md](data-tools.md) for full output shapes and the path
syntax. These commands are not the production backup/restore or typed preview
contract; Lane 10 owns that protocol.

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
render raw — as UTF-8 text when valid, else `0x<hex>` — not schema-typed.
JSON/JSONL carry the path plus base64 of the exact path and value bytes. This is
not a production backup format.

```console
$ marrow data dump ./proj
^books(1).author	Terry Pratchett
^books(1).title	Small Gods

$ marrow data dump --format jsonl ./proj
{"path":"^books(1).author","path_b64":"…","value_b64":"…"}
{"path":"^books(1).title","path_b64":"…","value_b64":"…"}
{"kind":"summary","records":2}
```

### `data integrity`

Verify every stored value decodes against its declared schema type. It needs the
checked project, so it loads and checks the source first. It reports decode
mismatches (`data.decode`), orphan data under an unknown root or undeclared member
(`data.orphan`), and corrupt keys (`store.corrupt_path`). Exits `0` on a clean
store, `1` when any problem is found.

```console
$ marrow data integrity ./proj
ok: ./proj integrity verified (2 records)
```

### `data get`

Read one path's value for inspection. The value renders raw, like `dump`.
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

Run the Marrow data server: a long-lived owner of the project's saved data that
answers newline-delimited JSON requests over a loopback (`127.0.0.1`) TCP
connection. It is a read-only tooling surface and never writes managed data.

The bound address is printed on startup, then the server blocks accepting
connections one at a time:

```console
$ marrow serve --port 0 ./proj
marrow serve listening on 127.0.0.1:52224
```

`--port` chooses the TCP port; `--port 0` (the default) lets the OS pick a free
port, printed on the line above. The server runs until interrupted. Each request
is one JSON object per line; each reply is `{"id": …, "ok": …}` or `{"id": …,
"error": {"code": …, "message": …}}`.

```console
$ printf '{"id":1,"op":"saved_roots"}\n' | nc 127.0.0.1 52224
{"id":1,"ok":{"roots":["books"]}}
```

For the full request/reply protocol — the supported operations, the path-segment
encoding, and the `protocol.*` error codes — see
[serve-protocol.md](serve-protocol.md).

Exits `2` on a usage error, `1` if the project config cannot be loaded, the store
cannot be opened, or the address cannot be bound.
