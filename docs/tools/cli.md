# CLI Reference

The installed command is `marrow`. A bare invocation and an unknown command are
usage failures (exit `2`). `marrow --help` prints the syntax implemented by the
current binary; `marrow --version` prints the package version.

The beta line's CLI is deliberately thin. `init`, `fmt`, `run`, `test`,
`--help`, and `--version` are the available commands; every other recognized
command name belongs to a capability being refounded and reports the typed code
`cli.command_unsupported` with exit `1`, so a script never mistakes absence for
success. [Project status](../status.md) states what returns through which
direction.

## Command index

| Command | Behavior today |
|---|---|
| `init` | Create a new project directory (this page). |
| `fmt` | Format a `.mw` file or every module of a project (this page). |
| `run` | Compile, verify, and run an exported function (this page). |
| `test` | Discover and run `test` declarations (this page; see [tests](tests.md)). |
| `check`, `data`, `doctor`, `evolve`, `serve`, `client`, `backup`, `restore` | Recognized; report `cli.command_unsupported` until their refounding lanes land. |

## `marrow init`

```text
marrow init <projectdir>
```

Creates a new [project](projects.md): a `marrow.toml` manifest declaring the
current edition and a contained `src` tree with a starter `main` module. The
target directory must not already exist — `init` claims it with an exclusive
create, so two concurrent runs cannot both win, and an existing directory
reports `config.invalid`. `init` creates no store.

## `marrow fmt`

```text
marrow fmt [--check | --write] <file.mw | projectdir>
```

Formats Marrow source to canonical layout through the retained formatter. The
target is either a single `.mw` file or a [project](projects.md) directory, in
which case every captured module is formatted through the project input.

With no flag on a single file, the formatted source is printed to stdout; on a
project directory, no flag checks without writing. `--check` leaves files
unchanged and exits nonzero when any is not canonical; `--write` replaces changed
source in place through a temporary file that preserves the original permissions.
`marrow fmt` does not read from stdin.

Source that does not parse is left untouched and reported with located
`parse.syntax` diagnostics. Formatting that would drop a retained comment is
refused with `fmt.comment_loss` rather than published lossily. A directory whose
manifest or source tree is invalid reports the matching `config.invalid` or
`project.*` code.

## `marrow run`

```text
marrow run <export> [--format text | jsonl] [-- <args>...]
```

Runs one exported (`pub fn`) function of the [project](projects.md) at the
working directory. The project is captured, compiled to a reproducible program
image, and independently verified into a sealed image before the VM runs the
export; the compiler opens no store and cannot mint a verified image. Arguments
after `--` are decoded positionally against the export's scalar parameter types
(`int`, `bool`, `string`).

Only a storeless export runs. A durable export — one whose verified demand reads
or writes durable data — compiles, verifies, and completes its durable identity,
but persistent execution is in the trough: the CLI no longer opens a store in
process, so a durable `run` reports the typed `cli.durable_unsupported` outcome
(exit `1`) rather than executing. Durable execution has returned for source tests
against a fresh ephemeral attachment (see [`marrow test`](#marrow-test)); the
persistent `run` path waits for the companion runner; see
[Project status](../status.md).

When a fresh durable declaration has no identity in the project's
[identity ledger](projects.md#the-identity-ledger), `run` still mints one from OS
entropy and publishes the updated `marrow.ids` atomically before compiling
again — commit that file — and then parks the durable export. Because an identity
is durable once minted, `run` mints and persists it even when the program still
has unrelated errors: the mint is not gated on an otherwise-clean compile, and
the recompile then reports whatever genuinely remains. This convenience is
`run`-only (every other command fails precisely with `check.durable_identity`)
and is superseded by the accepted change-review `apply` action when that lane
lands; a retired identity is never minted over.

Output is text by default — the returned value, or `absent` for a vacant
optional. `--format jsonl` prints one canonical JSON object: an outcome of
`value`, `diagnostic`, `artifact_rejected`, `fault`, or `error`, keeping the
four failure families distinct. A source diagnostic (`check.*`, `parse.*`), an
image rejection (`image.*`), a source-mapped runtime fault (`run.*`), and an
operational error (`store.*`, `io.*`) never collapse into one another.

Exit `0` carries the value; exit `1` is any failure family (including a durable
export parked in the trough); exit `2` is a usage error (an unknown export or a
bad argument).

## `marrow test`

```text
marrow test [--format text | jsonl] [--filter <substring>]
```

Discovers every `test "name"` declaration in the [project](projects.md) at the
working directory, compiles them into a separately verified image carrying a
closed test-entry table, and runs each one through the VM. A test
whose every `assert` holds passes; a false `assert` (`run.assert`) fails it; any
other runtime fault errors it. A test that touches no durable data runs storeless;
a test that reads or writes durable data runs against its own fresh ephemeral
attachment, so no test opens a persistent store or observes another test's writes.

`--filter` selects tests whose name contains the given substring and fails when
none match. Output is human text by default — one line per test and a summary —
or, with `--format jsonl`, one `kind: "test"` object per test and a final
`kind: "summary"` object. The command exits `0` when every selected test passes,
`1` when any fails or errors, and `2` on a usage error. See
[Tests](tests.md) for the report grammar and the `test`/`assert` language.

## Usage and exit codes

Flags that take values use separate arguments, such as `--check`; the CLI does
not accept `--flag=value` forms. Dotted diagnostic codes are defined in the
[Error Code Reference](../error-codes.md).

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Typed failure: a diagnostic was reported, or the command is not yet available on this line. |
| `2` | Command-line usage failed before the command body ran. |
