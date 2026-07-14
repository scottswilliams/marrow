# CLI Reference

The installed command is `marrow`. A bare invocation and an unknown command are
usage failures. `marrow --help` prints the complete syntax implemented by the
current binary; `marrow --version` prints the package version and engine profile.

## Command Index

| Command | Purpose | Detailed reference |
|---|---|---|
| `init` | Create the standard project scaffold. | This page |
| `check` | Parse and statically check a project. | [Diagnostics](diagnostics.md) |
| `doctor` | Read-only project and store triage. | [Diagnostics](diagnostics.md) |
| `fmt` | Check or write canonical source formatting. | This page |
| `run` | Check and invoke a project entry. | This page |
| `test` | Check and run configured test entries. | This page |
| `data` | Inspect typed durable data or recover an unclean store open. | [Data Tools](data.md) |
| `evolve` | Preview or apply supported populated-data changes. | [Evolution Tools](evolution.md) |
| `backup`, `restore` | Move durable state through a typed archive. | [Backup And Restore](backup-and-restore.md) |

The prototype surface/client/serve paths were deleted at B00; see
[Project status](../status.md).

## `marrow init`

```text
marrow init <projectdir>
```

The target must not exist. Its parent must exist, and its final path component
must be a Marrow module identifier: a letter or underscore followed by letters,
digits, or underscores. A relative argument must be a bare project name; an
absolute path may select an existing parent directory.

The scaffold contains `marrow.json`, a native-store library module, and a test.
It is the project used by the [Quickstart](../quickstart.md).

## `marrow fmt`

```text
marrow fmt [--check | --write] <file.mw | projectdir>
```

For one file, no mode prints formatted source to stdout. `--check` leaves the
file unchanged and exits nonzero when its current bytes are not canonical;
`--write` replaces changed source in place. A project directory requires one of
those two modes and applies it to `.mw` files under the configured source roots.
The formatter does not read stdin and refuses a rewrite that would lose a
retained comment.

## `marrow run`

```text
marrow run [--entry <entry>] [--arg name=value]... [--maintenance]
  [--trace] [--dry-run] [--format text|json] <projectdir>
```

`run` checks the project, opens the configured execution store, and invokes
`--entry` or `run.defaultEntry`. A bare entry name is accepted only when it
resolves uniquely; otherwise use `module::function`.

Each `--arg` supplies one named parameter and is decoded against the checked
entry signature. Repeated values collect only for supported sequence
parameters. Arguments are processed in command-line order.

In text mode, program `print` output goes to stdout. JSON mode captures program
output and the return value in the run report. `--trace` emits a text statement
and managed-write trace on stderr and is text-only.

`--dry-run` executes against an isolated store, reports managed writes on
stderr, and leaves the configured store unchanged. Effects outside saved data
are not rewound. JSON selects the dry-run report format when `--dry-run` is
present. Combining trace and dry-run remains text-only.

`--maintenance` grants the explicit capability required by modeled repair code,
including whole-root and required-field deletion. It cannot be supplied by
`marrow.json` or a default entry.

A native run may establish first-run durable identity or apply a schema change
that mutates no saved records. Changes that require backfill, transformation, or
destructive approval remain blocked; see [Evolution Tools](evolution.md).

## `marrow test`

```text
marrow test [--trace] [--format text|json|jsonl]
  [--filter <substring>] <projectdir>
```

Each zero-argument public function in a configured test file is a test. Every
test gets a fresh in-memory store, so tests do not share durable state with the
project or with one another. `--filter` selects qualified test names containing
the given substring and fails when none match. `--trace` writes a text trace to
stderr and cannot be combined with JSON or JSONL output.

Assertion failures are reported as failed tests; other runtime faults are
reported as errors. The command exits nonzero when any selected test fails or
errors.

## Usage And Reports

Flags that take values use separate arguments, such as `--format json`; the CLI
does not accept `--format=json`. Current exit classes are defined in
[Compatibility](../compatibility.md), and report formats and dotted codes are
defined in [Diagnostics](diagnostics.md).
