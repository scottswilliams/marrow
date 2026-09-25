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
pub(crate) const INTERNAL_HEADING: &str = "### Internal codes";

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

/// The page opening: the families a code's first segment names, and where the
/// language and tool references carry the behavior behind a code.
const PREAMBLE: &str = r#"# Errors

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

## Code reference"#;

/// One family section of the page: the `### ` heading and prose that open it,
/// then the codes whose rows fill its table, in page order.
struct Section {
    prose: &'static str,
    codes: &'static [Code],
}

/// Every family section in page order. A new code joins the family it belongs to
/// here, and a coverage test fails while one is registered but unlisted.
const SECTIONS: &[Section] = &[
    Section {
        prose: r#"
### `parse.*`

Syntax errors from the lexer and parser, reported by every command that reads
source.

| Code | Meaning |
|---|---|"#,
        codes: &[Code::ParseSyntax],
    },
    Section {
        prose: r#"
### `fmt.*`

Refusals from `marrow fmt`.

| Code | Meaning |
|---|---|"#,
        codes: &[Code::FmtCommentLoss, Code::FmtDiagnosticLimit],
    },
    Section {
        prose: r#"
### `cli.*`

Refusals raised by the `marrow` command itself.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::CliInterfaceUnbuildable,
            Code::CliDurableUnsupported,
            Code::CliInstallationDamaged,
            Code::CliCeilingUnaccepted,
            Code::CliArgumentLimit,
            Code::CliCompilerResourceLimit,
        ],
    },
    Section {
        prose: r#"
### `check.*`

Static errors found while checking source.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::CheckNestingLimit,
            Code::CheckUnsupported,
            Code::CheckType,
            Code::CheckNameConflict,
            Code::CheckModulePath,
            Code::CheckImport,
            Code::CheckVisibility,
            Code::CheckRecursion,
            Code::CheckRequiresTransaction,
            Code::CheckRequiresPresence,
            Code::CheckTransactionOwnerCalled,
            Code::CheckTransactionEmpty,
            Code::CheckTransactionReopened,
            Code::CheckTransactionUncommitted,
            Code::CheckTransactionConditional,
            Code::CheckDurableAfterCommit,
            Code::CheckTransactionMisplaced,
            Code::CheckAssertOutsideTest,
            Code::CheckTestDurableOperation,
            Code::CheckMatchNonexhaustive,
            Code::CheckMatchArm,
            Code::CheckInstantiationLimit,
            Code::CheckResourceLimit,
            Code::CheckDurableIdentity,
        ],
    },
    Section {
        prose: r#"
### `image.*`

Program-image verification failures. An image is verified in phases before it
runs, and a malformed or altered image is rejected at the first phase that finds
a fault.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::ImageEnvelope,
            Code::ImageTable,
            Code::ImageFunction,
            Code::ImageClosure,
            Code::ImageFlow,
            Code::ImageTestEntry,
        ],
    },
    Section {
        prose: r#"
### `run.*`

Runtime faults raised while running a verified program.

| Code | Meaning |
|---|---|"#,
        codes: &[
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
            Code::RunUniqueIndex,
            Code::RunCommit,
            Code::RunOutcomeUnknown,
            Code::RunCorruption,
            Code::RunCollectionLimit,
            Code::RunTemporalOverflow,
        ],
    },
    Section {
        prose: r#"
### `value.*`

Faults raised while encoding a value for a durable write.

| Code | Meaning |
|---|---|"#,
        codes: &[Code::ValueRange],
    },
    Section {
        prose: r#"
### `store.*`

Faults from a store. The message names the store path or operation; only the
code is stable.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::StoreIo,
            Code::StorePublicationUncertain,
            Code::StoreRestoreCommit,
            Code::StoreActivationUncertain,
            Code::StoreActivationRequired,
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
            Code::StoreApplyUnsupported,
            Code::StoreCeilingUnaccepted,
            Code::StoreDemandExceedsCeiling,
            Code::StoreImageNotActive,
            Code::StoreAuditUndecodable,
            Code::StoreAuditOutsideSchema,
            Code::StoreAuditRequiredMissing,
            Code::StoreAuditOrphanLeaf,
            Code::StoreAuditMarkerInvalid,
            Code::StoreAuditIndexOrphan,
            Code::StoreAuditIndexStale,
            Code::StoreAuditIndexMissing,
            Code::StoreAuditWitnessInvalid,
        ],
    },
    Section {
        prose: r#"
### `io.*`

Operational I/O faults from the command line and the runner.

| Code | Meaning |
|---|---|"#,
        codes: &[Code::IoRead, Code::IoThread, Code::IoWrite],
    },
    Section {
        prose: r#"
### `config.*`

Configuration faults, including an invalid project manifest.

| Code | Meaning |
|---|---|"#,
        codes: &[Code::ConfigInvalid],
    },
    Section {
        prose: r#"
### `project.*`

Faults from discovering a project's sources under `src`, resolving the local
dependencies its manifest declares, and reading its identity ledger
`.marrow/ids`.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::ProjectSourcePath,
            Code::ProjectModuleCollision,
            Code::ProjectCaptureLimit,
            Code::ProjectDependencyAlias,
            Code::ProjectDependencyPath,
            Code::ProjectIdsCorrupt,
            Code::ProjectIdsMint,
            Code::ProjectIdsLocation,
            Code::ProjectIdsPublicationPending,
        ],
    },
    Section {
        prose: r#"
### `wire.*`

Rejections of a message between the generated client and the runner. A frame is
rejected at the first bound or grammar rule it breaks, before its content is
acted on.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::WireFrameTooLarge,
            Code::WireDepthLimit,
            Code::WireStringLimit,
            Code::WireUnsupportedVersion,
            Code::WireMalformed,
            Code::WireNoncanonical,
        ],
    },
    Section {
        prose: r#"
### `runner.*`

Rejections from the runner that serves a launched program.

| Code | Meaning |
|---|---|"#,
        codes: &[
            Code::RunnerHandshake,
            Code::RunnerUnknownExport,
            Code::RunnerArgMismatch,
            Code::RunnerDurableUnsupported,
            Code::RunnerSpawn,
            Code::RunnerTerminated,
        ],
    },
];

/// The prose beneath [`INTERNAL_HEADING`]. Its codes come from the registry's
/// lifecycle, not a list, so a reclassification moves a code by itself.
const INTERNAL_PROSE: &str = r#"
These codes guard invariants the surrounding layers already close. An ordinary
program does not reach them.

| Code | Meaning |
|---|---|"#;

/// Render the full `docs/error-codes.md` page from the registry. The parts join with
/// one newline and the page ends with one, so each section's prose opens with the
/// blank line that separates it from the table above.
pub fn generate() -> String {
    let mut parts = Vec::with_capacity(2 * SECTIONS.len() + 4);
    parts.push(PREAMBLE.to_string());
    for section in SECTIONS {
        parts.push(section.prose.to_string());
        parts.push(rows(section.codes));
    }
    parts.push(format!("\n{INTERNAL_HEADING}"));
    parts.push(INTERNAL_PROSE.to_string());
    parts.push(rows(&internal()));
    format!("{}\n", parts.join("\n"))
}
