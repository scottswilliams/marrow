//! Byte-exact generation of `docs/error-codes.md` from the registry.
//!
//! The narrative prose lives here as raw-string segments; every per-code table row
//! is rendered from [`Code::meaning`]. The reserved-codes tables are driven from
//! [`Code::lifecycle`], so a lifecycle change moves the code between sections without
//! touching this file. The drift test regenerates and compares against the committed
//! page, so the registry is the single source of both code identity and documented
//! meaning; a coverage test asserts every registered code appears in its section.

use crate::{Code, Lifecycle};

/// The heading that opens the internal-codes section. `generate` emits it and the
/// coverage test splits the page on it, so the two cannot disagree.
pub(crate) const INTERNAL_HEADING: &str = "### Internal Codes";

fn rows(codes: &[Code]) -> String {
    codes
        .iter()
        .map(|c| format!("| `{}` | {} |", c.as_str(), c.meaning()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The internal codes, in registry order, across every family.
fn internal() -> Vec<Code> {
    Code::ALL
        .iter()
        .copied()
        .filter(|c| c.lifecycle() == Lifecycle::Internal)
        .collect()
}

/// Render the full `docs/error-codes.md` page from the registry.
pub fn generate() -> String {
    let parts: Vec<String> = vec![
        r#"# Errors

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

The envelope is a tooling representation of a failure. Source Marrow has no
throwable error-value channel; runtime faults terminate the invocation. Tools
may add fields such as `kind` and `source_span` when reporting a failure.

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
`store.transaction`, `store.read_only`, and `store.contract_changed`.
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
| `image` | `artifact` |
| `run` | `runtime` |
| `value` | `runtime` |
| `store` | `storage` |
| `io` | `io` |
| everything else (`cli`, `config`, `fmt`, `project`, `wire`, `runner`) | `tooling` |

## Code Reference

The family sections below list codes emitted by the current build. Internal
codes are separate from ordinary user-facing diagnostics.

### `parse.*` — kind `parse`

Syntax errors from the lexer and parser. Reported by project `check` and by any
command that parses sources before running.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::ParseSyntax]),
        r#"
### `fmt.*` — kind `tooling`

Formatter refusals.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::FmtCommentLoss]),
        r#"
### `cli.*` — kind `tooling`

Capabilities the CLI recognizes but cannot yet serve on this beta line: a command
whose owning capability is being refounded, and a durable `marrow run` whose
execution is in the trough.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::CliCommandUnsupported,
            Code::CliTransferExcluded,
            Code::CliDurableUnsupported,
            Code::CliInstallationDamaged,
            Code::CliCompilerResourceLimit,
        ]),
        r#"
### `check.*` — kind `check`

Static errors found while checking source.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::CheckNestingLimit,
            Code::CheckUnsupported,
            Code::CheckType,
            Code::CheckNameConflict,
            Code::CheckModulePath,
            Code::CheckImport,
            Code::CheckVisibility,
            Code::CheckRecursion,
            Code::CheckRequiresTransaction,
            Code::CheckTransactionOwnerCalled,
            Code::CheckTransactionEmpty,
            Code::CheckTransactionReopened,
            Code::CheckTransactionUncommitted,
            Code::CheckDurableAfterCommit,
            Code::CheckTransactionMisplaced,
            Code::CheckAssertOutsideTest,
            Code::CheckTestDriverMix,
            Code::CheckMatchNonexhaustive,
            Code::CheckMatchArm,
            Code::CheckInstantiationLimit,
            Code::CheckResourceLimit,
            Code::CheckDurableIdentity,
        ]),
        r#"
### `image.*` — kind `artifact`

Program-image decode and verification rejections, one per verifier phase. A
compiled image travels `bytes → verify → sealed image`; a hostile or malformed
image is rejected at the earliest phase whose invariant it violates, before the
VM can run it.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::ImageEnvelope,
            Code::ImageTable,
            Code::ImageFunction,
            Code::ImageClosure,
            Code::ImageFlow,
            Code::ImageTestEntry,
        ]),
        r#"
### `run.*` — kind `runtime`

Source-mapped runtime faults raised by the VM and the path kernel while running a
verified program: checked-arithmetic overflow, a zero division or remainder
divisor, a text bound, a reached `unreachable` invariant or `todo` deferral, call depth, an
execution budget, a nominal-interval violation, a temporal-domain overflow, an
authority denial, a required field left unset at commit, a unique-index
collision, an unconfirmed commit, a call whose reply was lost to the attached
runner's death (outcome unknown, never retried), and durable corruption. These
are not catchable inside the program.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::RunOverflow,
            Code::RunDivideByZero,
            Code::RunTextLimit,
            Code::RunUnreachable,
            Code::RunTodo,
            Code::RunAssert,
            Code::RunCallDepth,
            Code::RunBudget,
            Code::RunRange,
            Code::RunAuthority,
            Code::RunRequiredMissing,
            Code::RunUniqueIndex,
            Code::RunCommit,
            Code::RunOutcomeUnknown,
            Code::RunCorruption,
            Code::RunCollectionLimit,
            Code::RunTemporalOverflow,
        ]),
        r#"
### `value.*` — kind `runtime`

Value codec range faults raised while encoding a runtime value to its canonical
saved bytes for a durable write. Read-side decode and index-projection failures
are corruption faults instead. These terminate the invocation and are not source
values.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::ValueRange]),
        r#"
### `store.*` — kind `storage`

Store faults. The tree-cell facade produces `store.corruption` for malformed
tree-cell metadata, value codecs, index cells, or accepted catalog rows. A
persistent backend can also produce the I/O, locking, format, corruption,
recovery, limit, and read-only variants. Opening a damaged native store fails
closed with a typed code — never a process crash: a truncated or torn body is
`store.corruption`, and a store left needing repair by an unclean shutdown is
`store.recovery_required`.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::StoreIo,
            Code::StorePermissionDenied,
            Code::StoreLocked,
            Code::StoreFormatVersion,
            Code::StoreCorruption,
            Code::StoreRecoveryRequired,
            Code::StoreLimit,
            Code::StoreCursor,
            Code::StoreTransaction,
            Code::StoreReadOnly,
            Code::StoreContractChanged,
            Code::StoreDemandExceedsCeiling,
        ]),
        r#"
### `io.*` — kind `io`

Operational I/O faults from the CLI and runner. The CLI reports `io.read` when
it cannot read a project file (for example `marrow.toml`) and `io.thread` when
it cannot start its worker thread. Runner framing and output paths retain the
same read/write codes. Source Marrow exposes no I/O module or error-value channel.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::IoRead, Code::IoThread, Code::IoWrite]),
        r#"
### `config.*` — kind `tooling`

Configuration faults, including an invalid project manifest (`marrow.toml`) and
a non-UTF-8 command argument.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::ConfigInvalid]),
        r#"
### `project.*` — kind `tooling`

Project-capture faults raised while discovering a project's source under `src`
and reading its committed `marrow.ids` identity artifact: an invalid contained
path, a module-identity collision, an exceeded capture bound, a corrupt
identity artifact, or a failed identity mint.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::ProjectSourcePath,
            Code::ProjectModuleCollision,
            Code::ProjectCaptureLimit,
            Code::ProjectIdsCorrupt,
            Code::ProjectIdsMint,
        ]),
        r#"
### `wire.*` — kind `tooling`

Local-wire protocol rejections raised by the single wire owner while framing or
decoding a message between the generated client and the runner. A frame is
rejected at the earliest bound or grammar rule it violates — an oversized frame,
a too-deep or too-long value, an unrecognized protocol version, a malformed
body, or a non-canonical encoding — before its content is acted on.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::WireFrameTooLarge,
            Code::WireDepthLimit,
            Code::WireStringLimit,
            Code::WireUnsupportedVersion,
            Code::WireMalformed,
            Code::WireNoncanonical,
        ]),
        r#"
### `runner.*` — kind `tooling`

Runner request rejections raised while admitting a local-wire connection and
serving a request against the launched program image: a failed handshake, a
request naming an unknown export, arguments that do not match the export
signature, or a durable export the stock runner cannot yet execute.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::RunnerHandshake,
            Code::RunnerUnknownExport,
            Code::RunnerArgMismatch,
            Code::RunnerDurableUnsupported,
            Code::RunnerSpawn,
        ]),
        r#""#.to_string(),
        INTERNAL_HEADING.to_string(),
        r#"
These codes are emitted only by implementation-maintainer surfaces or as
defense-in-depth fail-closed guards over invariants the surrounding layers
already close. They are not ordinary user-facing diagnostics.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&internal()),
    ];
    format!("{}\n", parts.join("\n"))
}
