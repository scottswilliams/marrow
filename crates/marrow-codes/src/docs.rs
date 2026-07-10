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
pub(crate) const INTERNAL_HEADING: &str = "### Internal Fail-Closed Codes";

fn rows(codes: &[Code]) -> String {
    codes
        .iter()
        .map(|c| format!("| `{}` | {} |", c.as_str(), c.meaning()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The internal fail-closed codes, in registry order, across every family.
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
Tool invocation and output formats are described in
[`tools/diagnostics.md`](tools/diagnostics.md). This page is generated from the
code registry and lists every current code.

## CLI Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Recoverable parse, check, capability, runtime, storage, project, or tool failure. |
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
  `surface`, `io`, `usage`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use lowercase dotted text such as `parse.syntax` or
`book.already_loaned`. Segments use lowercase letters, digits, and
underscores.

Storage errors include the failed operation and the capability or limit
involved. Machine-readable facts belong in `data`; clients do not parse
`message`. The store reports a `store.*` code:
`store.io`, `store.permission_denied`, `store.locked`, `store.format_version`,
`store.corruption`, `store.recovery_required`, `store.limit`, `store.cursor`,
`store.transaction`, and `store.read_only`.
Backends enforce no key or value size limit, so `store.limit` is produced only
when Marrow framing cannot encode a tree-cell metadata or value-codec length
above a `u32` field.

Managed-root protection raises `write.*` codes when code attempts maintenance
work without the maintenance capability: `write.requires_maintenance` for a
whole managed-root delete. Deleting a required field on its own raises
`write.required_field` outside maintenance.

`marrow data integrity` reports `data.*` codes (kind `tooling`) for the findings
it surfaces while verifying saved data against the project schema:
`data.decode` for a stored value that is not a canonical form of its declared
type, `data.key_type` for a stored key with a scalar type the schema does not
declare, `data.dangling_ref` for a canonical identity leaf pointing to no saved
record node, `data.incomplete` for an existing record or keyed-layer entry
missing an accepted required field, and `data.orphan` for a stored cell under a
root or member the schema no longer declares (an undecodable cell key is
reported as `store.corruption`).
`marrow evolve` reports `evolve.*` codes when a preview witness
cannot be applied exactly. A command run against a project whose `marrow.json`
is unreadable reports `io.read`; an invalid `marrow.json` reports
`config.invalid`.
`marrow doctor` wraps existing typed project, lock, store, fence, and data
facts in `doctor.*` findings. The live store is always the authority; `doctor`
reports a stale or colliding committed `marrow.lock` but repairs nothing. Each
finding carries the underlying code or typed facts in `data` when one exists and
names an exact next command or manual remedy.

## How `kind` Is Assigned

Tools derive `kind` from the first dotted segment of `code`:

| First segment | `kind` |
|---|---|
| `parse` | `parse` |
| `check`, `schema` | `check` |
| `run`, `value` | `runtime` |
| `store` | `storage` |
| `surface` | `surface` |
| `io` | `io` |
| everything else (`backup`, `config`, `project`, `catalog`, `data`, `doctor`, `evolve`, `fmt`, `write`, `test`, `restore`) | `tooling` |

## Code Reference

The family sections below list codes emitted by the current build. Legacy
surface codes are isolated near the end. Internal codes are separate from
ordinary user-facing diagnostics.

### `parse.*` — kind `parse`

Syntax errors from the lexer and parser. Reported by project `check` and by any
command that parses sources before running.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::ParseSyntax]),
        r#"
### `fmt.*` — kind `tooling`

Formatter refusals.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::FmtCommentLoss]),
        r#"
### `check.*` — kind `check`

Static errors found while checking source. Project checks run module-wide rules
over every configured source and test file.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::CheckFailed, Code::CheckModulePath, Code::CheckDefaultEntry, Code::CheckDuplicateModule, Code::CheckMultipleScripts, Code::CheckDuplicateDeclaration, Code::CheckBuiltinCollision, Code::CheckSurfaceCollision, Code::CheckSurfaceTarget, Code::CheckSurfaceField, Code::CheckSurfaceAction, Code::CheckSurfaceComputedRead, Code::CheckUnresolvedImport, Code::CheckUnknownType, Code::CheckRecursiveKeyedEntry, Code::CheckReturnValue, Code::CheckMissingReturn, Code::CheckOperatorType, Code::CheckConditionType, Code::CheckCallArgument, Code::CheckReturnType, Code::CheckAssignmentType, Code::CheckLossyRoundTrip, Code::CheckRequiredAbsent, Code::CheckUninitializedVar, Code::CheckCommitAmplification, Code::CheckUntypedValue, Code::CheckKeyType, Code::CheckSequencePosition, Code::CheckUnresolvedName, Code::CheckUnknownField, Code::CheckUnknownRoot, Code::CheckLayerNotValue, Code::CheckUnresolvedCall, Code::CheckPrivateFunction, Code::CheckAmbiguousCall, Code::CheckNextIdRequiresSingleInt, Code::CheckNextIdCollision, Code::CheckRejectedSurface, Code::CheckCatalogIntent, Code::CheckLockMissing, Code::CheckStaleLock, Code::CheckStaleClient, Code::CheckDurableStoreRequired, Code::CheckUnresolvedOptional, Code::CheckUnannotatedAbsent, Code::CheckLiteralRange, Code::CheckStringEscape, Code::CheckBytesEscape, Code::CheckLoopControlFlow, Code::CheckCatchType, Code::CheckThrowType, Code::CheckMatchRequiresEnum, Code::CheckUnknownEnumMember, Code::CheckDuplicateMatchArm, Code::CheckNonexhaustiveMatch, Code::CheckAmbiguousMatchArm, Code::CheckScrutineeQualifiedMatchArm, Code::CheckAmbiguousMember, Code::CheckCategoryNotSelectable, Code::CheckIsRequiresEnum, Code::CheckIsType, Code::CheckInvalidAssignTarget, Code::CheckNonConstantConst, Code::CheckLoopMutatesTraversedLayer, Code::CheckNeighborUnsupported, Code::CheckKeyRequiresSingleKey, Code::CheckRange, Code::CheckRangeValue, Code::CheckCollectionUnsupported, Code::CheckLoopHeadArity, Code::CheckLoopHeadViewCall, Code::CheckReadOnlyExpressionContext, Code::CheckReadOnlyExpressionWrite, Code::CheckReadOnlyExpressionHostEffect, Code::CheckReadOnlyExpressionUnindexedLookup, Code::CheckPrivateEnum, Code::CheckExposedPrivateEnum, Code::CheckNestingLimit, Code::CheckEvolveTarget, Code::CheckEvolveType, Code::CheckEvolveTransform]),
        r#"
### `schema.*` — kind `check`

Resource-schema rules. Reported during a project check alongside `check.*`.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::SchemaDuplicateMember, Code::SchemaCategoryLeaf, Code::SchemaParentNotCategory, Code::SchemaDuplicateRootOwner, Code::SchemaUnknownInSaved, Code::SchemaOptionalInStoredShape, Code::SchemaKeyMemberCollision, Code::SchemaUnknownIndexArg, Code::SchemaUnorderableKey, Code::SchemaNonscalarKey, Code::SchemaNonEnumNamedField, Code::SchemaIndexMissingIdentityKeys, Code::SchemaIndexRequiresKeyedRoot, Code::SchemaNestedIndexArg]),
        r#"
### `catalog.*` — kind `tooling`

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::CatalogInvalid, Code::CatalogLockCorrupt]),
        r#"
### `doctor.*` — kind `tooling`

Read-only triage findings from `marrow doctor`. They aggregate existing typed
facts and never repair, regenerate the lock, apply evolution, or run an unbounded
integrity scan. The live store is always the authority; `doctor` reports when the
committed `marrow.lock` is stale, missing, or collides with it, but the operator
regenerates the lock — `doctor` repairs nothing.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::DoctorConfigInvalid, Code::DoctorLockCorrupt, Code::DoctorCheckFailed, Code::DoctorStoreLocked, Code::DoctorStoreRecoveryRequired, Code::DoctorStoreUnavailable, Code::DoctorPopulatedUnstamped, Code::DoctorCatalogCollision, Code::DoctorStoreLockEpochMismatch, Code::DoctorStaleLock, Code::DoctorLockMissing, Code::DoctorFenceMismatch, Code::DoctorIntegritySampleFailed]),
        r#"
### `run.*` — kind `runtime`

Runtime faults from the evaluator, surfaced by `run` and `test`. Deterministic
faults that the evaluator can recover from are raised as catchable `Error`
values: arithmetic faults, decimal envelope failures, recoverable type and
parse/range failures from builtins, and assertions keep their specific `run.*`
code. Runtime backstops for
unchecked or internal states, control-flow invariants, missing host
capabilities, unsupported constructs, storage failures, and traversal
conflicts are fatal runtime errors rather than catchable `Error` values. A
catchable fault that reaches the top of the program is reported under its own
code, except `run.uncaught_error` — see "Typed Errors In Running Programs".

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::RunType, Code::RunUnboundName, Code::RunOverflow, Code::RunDecimalOverflow, Code::RunTemporalOverflow, Code::RunDivideByZero, Code::RunNoEnclosingLoop, Code::RunUnknownFunction, Code::RunAmbiguousFunction, Code::RunPrivateFunction, Code::RunEntryArgument, Code::RunEntrySurface, Code::RunNoValue, Code::RunAbsentElement, Code::RunStore, Code::RunUnsupported, Code::RunCapability, Code::RunTransactionHostEffect, Code::RunAssertion, Code::RunUncaughtError, Code::RunTraversal, Code::RunDepth, Code::RunNoEntry, Code::RunDurableStoreRequired, Code::RunDryRunIsolation, Code::RunStoreEvolved, Code::RunStoreBehind, Code::RunSchemaDrift, Code::RunEngineProfile, Code::RunStoreUnstamped]),
        r#"
### `value.*` — kind `runtime`

Value codec range faults raised at the store write/read boundary while encoding
a runtime value to its canonical saved bytes or projecting it to an
order-preserving key. These are catchable `Error` values inside a running
program.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::ValueRange]),
        r#"
### `write.*` — kind `tooling`

Managed-write faults raised by the write planner inside a running program. They
surface to `run`/`test` as `Error` values, so code can catch them; an uncaught
one is reported under its own `write.*` code.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::WriteRequiredAbsent, Code::WriteTypeMismatch, Code::WriteIdentityMismatch, Code::WriteInvalidData, Code::WriteStore, Code::WriteUnknownField, Code::WriteUniqueConflict, Code::WriteUnknownLayer, Code::WriteNotALeafLayer, Code::WriteNotAGroupLayer, Code::WriteLayerKeyArity, Code::WriteIdOverflow, Code::WriteNextIdUnsupported, Code::WriteRequiredField, Code::WriteRequiresMaintenance, Code::WriteTransactionTooLarge]),
        r#"
### `store.*` — kind `storage`

Store faults. The tree-cell facade produces `store.corruption` for malformed
tree-cell metadata, value codecs, index cells, or accepted catalog rows. A
persistent backend can also produce the I/O, locking, format, corruption,
recovery, limit, and read-only variants. Opening a damaged native store fails
closed with a typed code — never a process crash: a truncated or torn body is
`store.corruption`, and a store left needing repair by an unclean shutdown is
`store.recovery_required`. A store fault met during a program read or write
travels as `run.store` or `write.store`; data tooling reports the `store.*` code
directly.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::StoreIo, Code::StorePermissionDenied, Code::StoreLocked, Code::StoreFormatVersion, Code::StoreCorruption, Code::StoreRecoveryRequired, Code::StoreLimit, Code::StoreCursor, Code::StoreTransaction, Code::StoreReadOnly]),
        r#"
### `io.*` — kind `io`

I/O faults spanning the CLI, `marrow serve`, the durable store, and the
`std::io` builtins. The CLI reports `io.read` when it cannot read a project file
(e.g. `marrow.json`), `io.listen` when a local listener cannot bind or accept,
`io.thread` when it cannot start its worker thread, `io.signal` when `marrow
serve` cannot install its shutdown-signal handler, and `io.entropy` when the OS
entropy source needed to stamp a durable store identity is unavailable. The
`std::io` builtins raise `io.read`/`io.write` as catchable `Error` values inside
a running program.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::IoRead, Code::IoListen, Code::IoThread, Code::IoSignal, Code::IoEntropy, Code::IoWrite]),
        r#"
### `config.*` and `project.*` — kind `tooling`

Project-loading faults from `marrow.json` and source discovery.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::ConfigMissing, Code::ConfigNotAProject, Code::ConfigInvalid, Code::ConfigDataDir, Code::ConfigClientWithoutSurface, Code::ProjectSourceRoot]),
        r#"
### `data.*` — kind `tooling`

Findings from the read-only `data` inspection commands. Most are surfaced by
`marrow data integrity`, which verifies saved values against the project schema;
`data.unknown_path` is surfaced by `marrow data get`. None modifies the store.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::DataDecode, Code::DataKeyType, Code::DataDanglingRef, Code::DataIncomplete, Code::DataOrphan, Code::DataUnknownPath]),
        r#"
### `evolve.*` — kind `tooling`

Source-native data-evolution preview/apply faults.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::EvolveNoAcceptedCatalog, Code::EvolveRepairRequired, Code::EvolveDrift, Code::EvolveCatalogDrift, Code::EvolveMaintenanceRequired, Code::EvolveApprovalRequired, Code::EvolveApprovalMismatch, Code::EvolveApprovalTargetUnknown, Code::EvolveRequiresBackup, Code::EvolveBackupPathManaged, Code::EvolveTransformFaulted]),
        r#"
### `test.*` — kind `tooling`

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::TestNone]),
        r#"
### `backup.*` — kind `tooling`

| Code | Meaning |
| --- | --- |"#.to_string(),
        rows(&[Code::BackupCatalogSerialization, Code::BackupCellTooLarge, Code::BackupManifestSerialization, Code::BackupStoreUidMissing]),
        r#"
### `restore.*` — kind `tooling`

Faults from `marrow restore` when a backup cannot be replayed into the project's
store, and from backup-backed data inspection or evolution preview when the
artifact cannot be mounted as the selected read target. `marrow backup` reports
`io.write` for a file it cannot write, a `store.*` code for a read fault, or a
`backup.*` code for backup-specific faults.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::RestoreFormatVersion, Code::RestoreCorruptChunk, Code::RestoreNotEmpty, Code::RestoreEngineRecompileRequired, Code::RestoreSourceMismatch, Code::RestoreCatalogMismatch, Code::RestoreDataInvalid]),
        r#"
## Typed Errors In Running Programs

In `.mw` code an error is an `Error` value with its own dotted `code`, raised by
`throw` and caught by `catch`. Builtins, managed writes, and deterministic
runtime faults raise typed errors too when the fault is recoverable: a failed
`std::io::readText` raises `io.read`, a rejected write raises a `write.*` code,
arithmetic raises specific numeric and temporal `run.*` codes, and value range
failures raise `value.*` codes. These typed raises are catchable in code. Fatal runtime
backstops for unchecked/internal states and host/tooling failures are not
`Error` values and can surface at the top level under their own `run.*` code.
When a language `throw` or `std::io` error is *not* caught and reaches the top of
the program, `run`/`test` report it as `run.uncaught_error`. Text carries the
original code in the message, while JSON run envelopes carry it in
`diagnostics[0].data.code`, for example:

```
run.uncaught_error: uncaught error [io.read]: std::io::readText failed for `/no/such/file`: No such file or directory (os error 2)
```

## Legacy Surface Codes

The implemented surface/client/server stack is legacy and intentionally absent
from the main language and tool references. Its reachable runtime paths emit
the codes below until the stack is deleted. They are current implementation
facts, not a v1 protocol commitment or compiler-integrated authorization model.

### `surface.*` — kind `surface`

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::SurfaceRequest, Code::SurfaceAuth, Code::SurfaceAbsent, Code::SurfaceCursor, Code::SurfaceStaleCursor, Code::SurfaceAbiMismatch, Code::SurfaceInvalidData, Code::SurfaceLimit, Code::SurfaceConflict, Code::SurfaceWrite, Code::SurfaceAction, Code::SurfaceComputed, Code::SurfaceStore]),
        r#""#.to_string(),
        INTERNAL_HEADING.to_string(),
        r#"
These codes are emitted, but only as defense-in-depth fail-closed guards over an
invariant the surrounding layers already close. A lower layer classifies every
publicly reachable case first, so an internal code has no public product repro.
It stands as an independent gate rather than a user-facing diagnostic.

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&internal()),
        r#""#.to_string(),
    ];
    parts.join("\n")
}
