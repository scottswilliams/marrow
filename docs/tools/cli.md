# CLI Reference

The installed command is `marrow`. A bare invocation and an unknown command are
usage failures (exit `2`). `marrow --help` prints the syntax implemented by the
current binary; `marrow --version` prints the package version.

The beta line's CLI is deliberately thin. `fmt`, `--help`, and `--version` are
the available commands; every other recognized command name belongs to a
capability being refounded and reports the typed code
`cli.command_unsupported` with exit `1`, so a script never mistakes absence for
success. [Project status](../status.md) states what returns through which
direction.

## Command index

| Command | Behavior today |
|---|---|
| `fmt` | Format a single `.mw` source file (this page). |
| `check`, `run`, `test`, `data`, `doctor`, `evolve`, `serve`, `client`, `backup`, `restore`, `init` | Recognized; report `cli.command_unsupported` until their refounding lanes land. |

## `marrow fmt`

```text
marrow fmt [--check | --write] <file.mw>
```

Formats one Marrow source file to canonical layout through the retained
formatter. With no flag, the formatted source is printed to stdout. `--check`
leaves the file unchanged and exits nonzero when its current bytes are not
canonical; `--write` replaces changed source in place through a temporary file
that preserves the original permissions. `marrow fmt` does not read from stdin,
and a directory target reports `cli.command_unsupported` (whole-project
formatting returns with the project owner).

Source that does not parse is left untouched and reported with located
`parse.syntax` diagnostics. Formatting that would drop a retained comment is
refused with `fmt.comment_loss` rather than published lossily.

## Usage and exit codes

Flags that take values use separate arguments, such as `--check`; the CLI does
not accept `--flag=value` forms. Dotted diagnostic codes are defined in the
[Error Code Reference](../error-codes.md).

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Typed failure: a diagnostic was reported, or the command is not yet available on this line. |
| `2` | Command-line usage failed before the command body ran. |
