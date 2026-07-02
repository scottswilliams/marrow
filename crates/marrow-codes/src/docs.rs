//! Byte-exact generation of `docs/error-codes.md` from the registry.
//!
//! The narrative prose lives here as raw-string segments; every per-code table row
//! is rendered from [`Code::meaning`]. The drift test regenerates and compares against
//! the committed page, so the registry is the single source of both code identity and
//! documented meaning.

use crate::Code;

fn rows(codes: &[Code]) -> String {
    codes
        .iter()
        .map(|c| format!("| `{}` | {} |", c.as_str(), c.meaning()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the full `docs/error-codes.md` page from the registry.
pub fn generate() -> String {
    let parts: Vec<String> = vec![
        r#"# Errors

Marrow errors are part of the product surface. A good error says what
happened, where it happened, and what to try next when Marrow knows.

Language-level error behavior is described in
[`language/control-flow-and-effects.md`](language/control-flow-and-effects.md).
This page describes the CLI and tooling contract.

## CLI Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Recoverable parse, check, capability, runtime, storage, project, or tool failure. |
| `2` | Command-line usage failed before the command body ran. |

## Error Envelope

Machine-readable surfaces use a stable envelope:

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

- `code`: stable machine code;
- `kind`: broad category such as `parse`, `check`, `runtime`, `storage`,
  `surface`, `io`, `usage`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use stable lowercase dotted text such as `parse.syntax` or
`book.already_loaned`. Segments use lowercase letters, digits, and
underscores.

Marrow surfaces use dotted Marrow error codes and typed error values.

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

Tools derive `kind` from the first dotted segment of `code`, so the kind of a
code is stable and predictable:

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

The main family sections below list codes emitted by the current build. The
Application Surfaces section marks which surface codes are active in the
transport-neutral runtime API and which remain reserved. Codes are grouped by
family, and each family description names where a developer first meets the
code: a project `check`/`run`/`test`, a managed write inside a running program,
the store, or a `data` maintenance command.

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
        rows(&[Code::CheckFailed, Code::CheckModulePath, Code::CheckDefaultEntry, Code::CheckDuplicateModule, Code::CheckMultipleScripts, Code::CheckDuplicateDeclaration, Code::CheckBuiltinCollision, Code::CheckSurfaceCollision, Code::CheckSurfaceTarget, Code::CheckSurfaceField, Code::CheckSurfaceAction, Code::CheckSurfaceComputedRead, Code::CheckUnresolvedImport, Code::CheckUnknownType, Code::CheckRecursiveKeyedEntry, Code::CheckReturnValue, Code::CheckMissingReturn, Code::CheckOperatorType, Code::CheckConditionType, Code::CheckCallArgument, Code::CheckReturnType, Code::CheckAssignmentType, Code::CheckLossyRoundTrip, Code::CheckRequiredAbsent, Code::CheckUninitializedVar, Code::CheckCommitAmplification, Code::CheckUntypedValue, Code::CheckKeyType, Code::CheckSequencePosition, Code::CheckUnresolvedName, Code::CheckUnknownField, Code::CheckUnknownRoot, Code::CheckLayerNotValue, Code::CheckUnresolvedCall, Code::CheckPrivateFunction, Code::CheckAmbiguousCall, Code::CheckNextIdRequiresSingleInt, Code::CheckNextIdCollision, Code::CheckRejectedSurface, Code::CheckCatalogIntent, Code::CheckLockCorrupt, Code::CheckLockMissing, Code::CheckStaleLock, Code::CheckStaleClient, Code::CheckDurableStoreRequired, Code::CheckUnresolvedOptional, Code::CheckUnannotatedAbsent, Code::CheckLiteralRange, Code::CheckStringEscape, Code::CheckBytesEscape, Code::CheckLoopControlFlow, Code::CheckCatchType, Code::CheckThrowType, Code::CheckMatchRequiresEnum, Code::CheckUnknownEnumMember, Code::CheckDuplicateMatchArm, Code::CheckNonexhaustiveMatch, Code::CheckAmbiguousMatchArm, Code::CheckScrutineeQualifiedMatchArm, Code::CheckAmbiguousMember, Code::CheckCategoryNotSelectable, Code::CheckIsRequiresEnum, Code::CheckIsType, Code::CheckInvalidAssignTarget, Code::CheckNonConstantConst, Code::CheckLoopMutatesTraversedLayer, Code::CheckNeighborUnsupported, Code::CheckKeyRequiresSingleKey, Code::CheckRange, Code::CheckRangeValue, Code::CheckCollectionUnsupported, Code::CheckReadOnlyExpressionContext, Code::CheckReadOnlyExpressionWrite, Code::CheckReadOnlyExpressionHostEffect, Code::CheckReadOnlyExpressionUnindexedLookup, Code::CheckPrivateEnum, Code::CheckExposedPrivateEnum, Code::CheckNestingLimit, Code::CheckEvolveTarget, Code::CheckEvolveType, Code::CheckEvolveTransform]),
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
|---|---|
| `test.none` | `marrow test` found no tests; check the `tests` paths in `marrow.json`. Exit code `1`. (Failing tests are reported per test with their own `run.assertion` or other `run.*` code, not a `test.*` code.)|

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

## Application Surfaces

`marrow data diff`/`data load` are deferred — see
[future/data-tools.md](future/data-tools.md). Restore replace is part of the
current CLI surface; restore merge/repair and cross-engine restore remain
deferred. No active command-output code family appears for a deferred surface
until that surface ships.

The `surface.*` family belongs to the application surface runtime and its
[Surface ABI](surface-abi.md). The transport-neutral `marrow-run`
node-read, collection-read, computed-read, generated create/update/delete, and
action APIs can emit the active codes below. `marrow serve` emits
sanitized code/message envelopes for HTTP serving in both default read-only mode
and `--write` mode, and adds `surface.auth` for remote HTTP authorization and
mode denial before request-body decoding.
Remote cursor-token mode maps opaque cursor strings onto the same active typed
runtime continuation value at the HTTP boundary.

### `surface.*` — kind `surface`

| Code | Meaning |
|---|---|"#.to_string(),
        rows(&[Code::SurfaceRequest, Code::SurfaceAuth, Code::SurfaceAbsent, Code::SurfaceCursor, Code::SurfaceStaleCursor, Code::SurfaceAbiMismatch, Code::SurfaceInvalidData, Code::SurfaceLimit, Code::SurfaceConflict, Code::SurfaceWrite, Code::SurfaceAction, Code::SurfaceComputed, Code::SurfaceIntegrity, Code::SurfaceStore]),
        r#"
### Reserved And Future Codes

The remaining `check.surface_*` names are reserved for future surface checker
diagnostics, including stable ABI export checks. They do not appear in v0.1
command output until those checks ship.

| Code | Reserved meaning |
|---|---|"#.to_string(),
        rows(&[Code::CheckSurfaceDecl, Code::CheckSurfaceCatalogPending, Code::CheckSurfaceOperation]),
        r#"
The `decode.*` family is reserved for future checked decode and repair reports.
These codes do not appear in v0.1 command output.

| Code | Reserved meaning |
|---|---|"#.to_string(),
        rows(&[Code::DecodeShape, Code::DecodeUnknownMember, Code::DecodeRequiredAbsent, Code::DecodeValue]),
        r#""#.to_string(),
    ];
    parts.join("\n")
}
