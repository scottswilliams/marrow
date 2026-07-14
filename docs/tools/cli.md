# CLI Reference

The installed command is `marrow`. A bare invocation and an unknown command are
usage failures (exit `2`). `marrow --help` prints the syntax implemented by the
current binary; `marrow --version` prints the package version.

The beta line's CLI is deliberately thin. `init`, `fmt`, `--help`, and
`--version` are the available commands; every other recognized command name
belongs to a capability being refounded and reports the typed code
`cli.command_unsupported` with exit `1`, so a script never mistakes absence for
success. [Project status](../status.md) states what returns through which
direction.

## Command index

| Command | Behavior today |
|---|---|
| `init` | Create a new project directory (this page). |
| `fmt` | Format a `.mw` file or every module of a project (this page). |
| `check`, `run`, `test`, `data`, `doctor`, `evolve`, `serve`, `client`, `backup`, `restore` | Recognized; report `cli.command_unsupported` until their refounding lanes land. |

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

## Usage and exit codes

Flags that take values use separate arguments, such as `--check`; the CLI does
not accept `--flag=value` forms. Dotted diagnostic codes are defined in the
[Error Code Reference](../error-codes.md).

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Typed failure: a diagnostic was reported, or the command is not yet available on this line. |
| `2` | Command-line usage failed before the command body ran. |
