# Errors

Marrow diagnostics use typed dotted codes. Human-readable messages explain
what happened, where it happened, and what to try next when Marrow knows.

Language-level error behavior is described in
[`language/errors-and-transactions.md`](language/errors-and-transactions.md).
Tool invocation is described in [`tools/cli.md`](tools/cli.md). This page is
generated from the code registry and lists every current code.

## CLI Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Recoverable parse, check, runtime, storage, or tool failure. |
| `2` | Command-line usage failed before the command body ran. |

## Error Envelope

Machine-readable commands use this envelope where their selected format calls
for a single diagnostic object:

```json
{
  "code": "parse.syntax",
  "kind": "parse",
  "message": "expected expression",
  "help": "Add an expression after return.",
  "source_span": {
    "file": "src/app.mw",
    "line": 12,
    "column": 16
  }
}
```

The envelope is a tooling representation of an error. In `.mw` code, thrown
errors are `Error` values as described in the language reference. Tools may
add fields such as `kind` and `source_span` when reporting the error outside
the running program.

Common fields:

- `code`: typed machine code;
- `kind`: broad category such as `parse`, `check`, `runtime`, `storage`,
  `io`, `usage`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use lowercase dotted text such as `parse.syntax`. Segments
use lowercase letters, digits, and underscores.

Only the dotted `code` is machine-stable for storage errors. Details such as the
operation, path, limit name, or invalid state may appear only in the current
human-readable message; their wording is not a machine contract. The store
reports a `store.*` code:
`store.io`, `store.permission_denied`, `store.locked`, `store.format_version`,
`store.corruption`, `store.recovery_required`, `store.limit`, `store.cursor`,
`store.transaction`, and `store.read_only`.
`store.limit` reports an exhausted finite representation bound: a store framing
length/count that does not fit its `u32` field, a record/problem/index count
overflow, or exhaustion of the `u64` commit-ID sequence.

A command run against a project whose `marrow.toml` is unreadable reports
`io.read`; an invalid `marrow.toml` reports `config.invalid`, and a
contained-discovery fault reports a `project.*` code.

## How `kind` Is Assigned

Tools derive `kind` from the first dotted segment of `code`:

| First segment | `kind` |
|---|---|
| `parse` | `parse` |
| `check` | `check` |
| `value` | `runtime` |
| `store` | `storage` |
| `io` | `io` |
| everything else (`cli`, `config`, `fmt`, `project`) | `tooling` |

## Code Reference

The family sections below list codes emitted by the current build. Internal
codes are separate from ordinary user-facing diagnostics.

### `parse.*` — kind `parse`

Syntax errors from the lexer and parser. Reported by project `check` and by any
command that parses sources before running.

| Code | Meaning |
|---|---|
| `parse.syntax` | The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected. |

### `fmt.*` — kind `tooling`

Formatter refusals.

| Code | Meaning |
|---|---|
| `fmt.comment_loss` | `marrow fmt` would drop a retained comment while rewriting the source, so the command refuses instead of publishing lossy formatted output. |

### `cli.*` — kind `tooling`

Commands recognized but not yet available on this beta line.

| Code | Meaning |
|---|---|
| `cli.command_unsupported` | A command name is recognized but not yet available on this beta line: its owning capability is being refounded and returns through a later lane. `marrow fmt`, `marrow --version`, and `marrow --help` are the currently available commands. |

### `check.*` — kind `check`

Static errors found while checking source.

| Code | Meaning |
|---|---|
| `check.nesting_limit` | Source nests expressions or statement blocks deeper than the fixed parser limit (256). Raised by the parser at the offending span so pathologically nested source fails closed rather than overflowing the stack; see [execution limits](language/execution-limits.md). |

### `value.*` — kind `runtime`

Value codec range faults raised at the store write/read boundary while encoding
a runtime value to its canonical saved bytes or projecting it to an
order-preserving key. These are catchable `Error` values inside a running
program.

| Code | Meaning |
|---|---|
| `value.range` | A `date` or `instant` reaching the store codec lies outside Marrow's supported calendar range, years 0001-9999. This is a store-boundary integrity guard, not a source-arithmetic fault: every `.mw` temporal path (the `date`/`instant` constructors, `std::clock` parse and `addDays` helpers, and `+`/`-` arithmetic) shares the same 0001-9999 envelope and already raises `run.temporal_overflow` before an out-of-range value can be produced, so no ordinary checked program reaches this code. It fires only if a value that bypasses those bounds reaches the canonical encoder or key projection. |

### `store.*` — kind `storage`

Store faults. The tree-cell facade produces `store.corruption` for malformed
tree-cell metadata, value codecs, index cells, or accepted catalog rows. A
persistent backend can also produce the I/O, locking, format, corruption,
recovery, limit, and read-only variants. Opening a damaged native store fails
closed with a typed code — never a process crash: a truncated or torn body is
`store.corruption`, and a store left needing repair by an unclean shutdown is
`store.recovery_required`.

| Code | Meaning |
|---|---|
| `store.io` | An I/O operation on a persistent backend failed. |
| `store.permission_denied` | The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry. |
| `store.locked` | The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry. |
| `store.format_version` | The store's recorded format version is not the one this build supports. |
| `store.corruption` | The store file or a tree-cell record is corrupt and could not be opened or decoded, including a truncated or torn store body. |
| `store.recovery_required` | The store was not shut down cleanly, so a read-only open is refused until a write-capable open replays the interrupted commit. The recovery command returns with the refounded durable lifecycle; recovery is attempted, not guaranteed, and a store damaged beyond replay surfaces `store.corruption`. |
| `store.limit` | Marrow exhausted a fixed representational bound: a store framing length/count did not fit its `u32` field, a record/problem/index count overflowed, or the `u64` commit-ID sequence was exhausted. |
| `store.cursor` | A bounded scan cursor does not belong to the scan being resumed. |
| `store.transaction` | A transaction or snapshot operation was requested in an invalid store state. |
| `store.read_only` | A write-capability operation was requested through a read-only store handle. |

### `io.*` — kind `io`

I/O faults spanning the CLI, the durable store, and the `std::io` builtins. The
CLI reports `io.read` when it cannot read a project file (e.g. `marrow.toml`)
and `io.thread` when it cannot start its worker thread. The `std::io` builtins
raise `io.read`/`io.write` as catchable `Error` values inside a running program.

| Code | Meaning |
|---|---|
| `io.read` | A read failed: a project source file or `marrow.toml` could not be read, or `std::io::readText`/`readBytes` failed. |
| `io.thread` | The CLI could not spawn the worker thread it uses for parsing, checking, and running. |
| `io.write` | `std::io::writeText`/`writeBytes` failed. |

### `config.*` — kind `tooling`

Configuration faults, including an invalid project manifest (`marrow.toml`) and
a non-UTF-8 command argument.

| Code | Meaning |
|---|---|
| `config.invalid` | A configuration input or project-setup precondition is invalid: the project manifest `marrow.toml` is malformed TOML, declares an unknown key, or declares no supported `edition`; a command argument is not valid UTF-8; or `marrow init` targets a directory that already exists. A malformed-manifest fault carries its `marrow.toml` line and column in `source_span`; a validation fault with no single source point carries none. |

### `project.*` — kind `tooling`

Project-capture faults raised while discovering a project's source under `src`:
an invalid contained path, a module-identity collision, or an exceeded capture
bound.

| Code | Meaning |
|---|---|
| `project.source_path` | A captured source file path is not a valid contained module identity: it is absolute, escapes the source root with `..`, is not a canonical forward-slash path, contains a NUL or ASCII control character, lives outside the fixed `src` source root, or is not a `.mw` file with a non-empty name. A project whose `src` root is itself a symlink is refused with this code before discovery. |
| `project.module_collision` | Two captured source files collide on module identity: they derive the same module name, or their paths differ only in case and would name the same file on a case-insensitive filesystem. The message names both files. |
| `project.capture_limit` | A project capture exceeded a fixed bound: too many source files, one source file too large, or the source files together too large. The bound guards the compiler against an unbounded project tree. |

### Internal Codes

These codes are emitted only by implementation-maintainer surfaces or as
defense-in-depth fail-closed guards over invariants the surrounding layers
already close. They are not ordinary user-facing diagnostics. None are defined
in the current build.

| Code | Meaning |
|---|---|
