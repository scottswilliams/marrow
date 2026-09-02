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

Every Marrow diagnostic carries a dotted code such as `check.type`. The code is
the stable part. The message beside it says what happened, where, and what to
try; its wording is not a machine contract.

A code's first segment names its family. `parse.*` and `check.*` are source
diagnostics, reported at a line and column. `image.*` rejects a program image
before it runs. `run.*` and `value.*` are runtime faults: a fault stops the
invocation at the source span of the operation, and a program cannot catch it.
The remaining families are operational errors from the store, the command line,
the project, and the runner.

Language-level error behavior is described in
[`language/errors-and-transactions.md`](language/errors-and-transactions.md).
Tool invocation is described in [`tools/cli.md`](tools/cli.md). This page is
generated from the code registry and lists every code the current build emits.

## Code Reference

### `parse.*`

Syntax errors from the lexer and parser, reported by every command that reads
source.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::ParseSyntax]),
        r#"
### `fmt.*`

Refusals from `marrow fmt`.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::FmtCommentLoss, Code::FmtDiagnosticLimit]),
        r#"
### `cli.*`

Refusals raised by the `marrow` command itself.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::CliCommandUnsupported,
            Code::CliInterfaceUnbuildable,
            Code::CliDurableUnsupported,
            Code::CliInstallationDamaged,
            Code::CliCeilingUnaccepted,
            Code::CliCompilerResourceLimit,
        ]),
        r#"
### `check.*`

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
### `image.*`

Program-image verification failures. An image is verified in phases before it
runs, and a malformed or altered image is rejected at the first phase that finds
a fault.

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
### `run.*`

Runtime faults raised while running a verified program.

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
### `value.*`

Faults raised while encoding a value for a durable write.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::ValueRange]),
        r#"
### `store.*`

Faults from a store. The message names the store path or operation; only the
code is stable.

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
### `io.*`

Operational I/O faults from the command line and the runner.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::IoRead, Code::IoThread, Code::IoWrite]),
        r#"
### `config.*`

Configuration faults, including an invalid project manifest.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[Code::ConfigInvalid]),
        r#"
### `project.*`

Faults from discovering a project's sources under `src` and reading its
identity ledger `.marrow/ids`.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&[
            Code::ProjectSourcePath,
            Code::ProjectModuleCollision,
            Code::ProjectCaptureLimit,
            Code::ProjectIdsCorrupt,
            Code::ProjectIdsMint,
            Code::ProjectIdsLocation,
            Code::ProjectIdsPublicationPending,
        ]),
        r#"
### `wire.*`

Rejections of a message between the generated client and the runner. A frame is
rejected at the first bound or grammar rule it breaks, before its content is
acted on.

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
### `runner.*`

Rejections from the runner that serves a launched program.

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
These codes guard invariants the surrounding layers already close. An ordinary
program does not reach them.

| Code | Meaning |
|---|---|"#
            .to_string(),
        rows(&internal()),
    ];
    format!("{}\n", parts.join("\n"))
}
