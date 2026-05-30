# CLI Reference

The `marrow` binary is the single entry point for the language and its built-in
database.

```
marrow check [--format text|json|jsonl] <file.mw | projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow run [--entry <module::function>] [--maintenance] <projectdir>
marrow test <projectdir>
marrow backup <projectdir> <archive>
marrow restore <projectdir> <archive>
marrow data <roots|stats|dump|integrity> <projectdir>
marrow data get <projectdir> <path>
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
- `json` — a single JSON object on stdout.
- `jsonl` — one JSON object per line on stdout, ending with a `{"kind":
  "summary", …}` line. Useful for streaming many records or diagnostics.

`run` has no `--format`: its stdout is the program's own `print`/`write` output,
which carries no envelope. `--format` is accepted by `check` and every `data`
subcommand; for `data roots` and `data stats`, `jsonl` emits the same single
object as `json` (there is nothing to stream).

---

## `marrow check`

```
marrow check [--format text|json|jsonl] <file.mw | projectdir>
```

Parse a single `.mw` file, or check a whole project directory, and report
diagnostics.

- Given a `.mw` file, it parses that file in isolation and reports parse
  diagnostics. It does not type-check across modules.
- Given a project directory, it loads `marrow.json` and runs the project checker
  over every source root: parse, type, effect, and saved-path checks.

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
marrow run [--entry <module::function>] [--maintenance] <projectdir>
```

Check a project, then run an entry function over the store its `marrow.json`
selects. The store is the configured backend — a `native` redb store on disk, or
an in-memory store when none is configured (see
[project-config.md](project-config.md)). A project must check cleanly before it
runs.

The entry is `--entry` if given, otherwise the project's `run.defaultEntry`. If
neither is present, `run` fails with `run.no_entry` (exit `1`).

Output written with `print`/`write` goes to stdout. `std::log` output goes to
stderr. The run reads the real system clock, environment, and filesystem.

`--maintenance` grants the run the maintenance capability, for migration,
repair, and restore tooling. It permits whole managed-root deletes,
required-field deletes, and raw quoted-segment access that the default run
rejects. An operator must type it; the default run and `run.defaultEntry` can
never inject it. Use it deliberately.

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
marrow test <projectdir>
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

## `marrow backup`

```
marrow backup <projectdir> <archive>
```

Write the store's whole saved tree to a portable archive at `<archive>` — the
canonical ordered `(path, value)` stream behind a small framing, not an engine
file. Generated index trees are included. The store is opened for owned access.

Prints the record count and exits `0`; exits `1` on an I/O or store error.

```console
$ marrow backup ./proj ./shelf.mwa
backed up 2 records to ./shelf.mwa
```

See [data-tools.md](data-tools.md) for the archive format and how backup relates
to the `data` inspection commands.

---

## `marrow restore`

```
marrow restore <projectdir> <archive>
```

Restore a project's saved data from an archive into an empty store.
Empty-target restore is the only mode implemented today: if the target already
holds data, restore refuses with `restore.not_empty` (exit `1`) rather than
overwrite it.

Replace, merge, and repair restores (the non-empty cases) are deferred — see
[future/cli.md](future/cli.md).

Prints the record count and exits `0`; exits `1` if the target is non-empty or on
an I/O or store error.

```console
$ marrow restore ./fresh-proj ./shelf.mwa
restored 2 records from ./shelf.mwa

$ marrow restore ./proj ./shelf.mwa     # target already has data
restore.not_empty: restore target already holds data; restore writes into an empty store
$ echo $?
1
```

---

## `marrow data`

```
marrow data roots     [--format text|json|jsonl] <projectdir>
marrow data stats     [--format text|json|jsonl] <projectdir>
marrow data dump      [--format text|json|jsonl] <projectdir>
marrow data integrity [--format text|json|jsonl] <projectdir>
marrow data get       [--format text|json|jsonl] <projectdir> <path>
```

Read-only inspection of a project's saved data. It never creates or modifies the
store; a project with no saved data on disk reports as empty. See
[data-tools.md](data-tools.md) for full output shapes and the path syntax.

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

Print every stored `(path, value)` in encoded order. Values render raw — as UTF-8
text when valid, else `0x<hex>` — not schema-typed, so dump works without source.
JSON/JSONL carry the path plus base64 of the exact path and value bytes.

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

Read one path's value. The value renders raw, like `dump`. Absence is a valid
result (exit `0`): a path with no value but children prints `(no value; has
children)`, a truly absent path prints `(absent)`. An unparseable path is a usage
error (exit `2`).

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
