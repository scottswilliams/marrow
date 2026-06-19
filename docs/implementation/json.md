# Shared JSON

`marrow-json` owns small JSON DTOs that more than one CLI, tool, or application
boundary needs. It exists to keep `marrow run --format json`, trace, data
integrity, store-backed data inspection, and surface reads, updates, actions,
operation envelopes, and descriptor export from copying entry-return, saved-key,
data-snapshot, surface descriptor/result rendering, checked surface read
request-parameter decode, sparse update request-body decode, and action
argument/result rendering. Surface descriptors include checked read/action
aliases as labels for later route or client renderers; those aliases are not
operation identity. For surfaces it
also provides an in-process operation-tag execution boundary over
caller-supplied `CheckedProgram` and `TreeStore` references, read-only
execution helpers over `ProjectSurfaceReadSession`, point/singleton update
and action execution helpers over `ProjectSurfaceSession`, and
`surface.operation.v1` request/response/error envelope DTOs over project surface
sessions; it does not own serving, routes, or process lifetime policy.

The crate deliberately does not define a general `Value` JSON ABI. Its entry
return renderer preserves the current CLI-compatible result surface: scalars,
enums, identities, and sequences render; whole resources and local trees fault
at the CLI boundary as `run.entry_surface`. The enum form remains the existing
CLI numeric `enum_id` / `member_id` profile, and `int` values remain JSON
numbers for compatibility with current CLI consumers. Its data-snapshot DTO
renders the shared `store_snapshot` object for `marrow data roots|get`, including
the store UID, catalog digest, optional commit stamp, and checked source digest.
Its surface DTOs render `marrow-run` surface records, pages, values,
identities, and commit-bound typed cursors with accepted catalog IDs, store
commit IDs, typed keys, base64 bytes, and lossless strings for integers,
temporal nanoseconds, and decimals. Cursor/page rendering is context-aware:
enum and identity-typed index arguments render as branded surface arguments
instead of raw saved key bytes or plain member strings. Cursor decode preserves
the producing commit boundary so runtime page execution can reject stale
continuations after intervening writes.

`SurfaceAbiJson` renders the successful `marrow check --format json|jsonl`
`surface_abi` object from checker-owned facts. It includes display-only module
and surface labels, typed catalog status, stable read descriptors, optional
sparse update descriptors for stable surfaces with a non-empty update set, and
action descriptors that reuse `entry.invoke.v1` identity, parameter shapes, and
return shape, but only when their operation tags are callable through runtime
tag admission.
Duplicate stable operation tags are omitted from all read, update, or action
descriptors that share the tag. Source-only surfaces serialize blocker strings
and no operation descriptors. Duplicate-tag checker diagnostics remain future
work. Update fields expose `backing_required` only as backing-field metadata;
sparse update request bodies remain non-empty patches and no field is mandatory
on every request.

Inbound surface request parameters and sparse update bodies are checked against
the admitted runtime surface shape. `SurfacePointRequestJson`,
`SurfacePageRequestJson`, `SurfaceUniqueLookupRequestJson`,
`SurfaceArgumentJson`, and `SurfaceCursorJson` decode read identities, exact
index arguments, unique lookup keys, limits, and commit-bound typed cursor
boundaries into runtime `SavedKey` and cursor values.
`SurfacePointUpdateRequestJson`,
`SurfaceSingletonUpdateRequestJson`, `SurfaceUpdateFieldJson`, and
`SurfaceUpdateValueJson` decode update identities, field catalog IDs, and
canonical scalar/enum/identity values into runtime `SurfaceUpdateInput` values.
`SurfaceActionRequestJson` accepts `entry.invoke.v1` argument JSON values and
delegates decoding to `marrow-run`; `SurfaceActionResultJson` carries captured
program output and an optional surface action value DTO. That action value DTO
is deliberately separate from `marrow run --format json`: enum and identity
results carry accepted catalog IDs, not checker-local runtime IDs or source root
labels. The active action JSON shape covers scalars, enums, identities, and
scalar/enum sequences; resource trees, local trees, errors, unknown values, and
unsupported sequence elements are rejected by the checker before a surface action
descriptor is exported.
`SurfaceOperationRequestJson`, `SurfaceOperationResponseJson`,
`SurfaceOperationResultJson`, and `SurfaceOperationErrorJson` wrap those same
typed request and result bodies in the active `surface.operation.v1` profile.
JSON decode is structural and canonical only: runtime `SurfaceUpdate` owns
declared update-set authorization, duplicate and non-empty patch validation,
exact value shape checks, enum membership and selectability, identity store,
arity, and key-scalar validation, record presence, and post-patch footprint
validation. The linked-Rust `entry.invoke.v1` descriptor and `EntryInvocation`
path is owned by `marrow-check` and `marrow-run`; this crate only embeds that
argument JSON shape for surface action request bodies. HTTP routes, opaque
cursor tokens, generated clients, and create/delete body decode remain outside
this crate's current profile.

The operation-tag execution functions compose those DTOs with `marrow-run`
admission. Reads admit stable read tags, decode the point/page/unique request
DTO against the admitted handle, execute singleton, point, page, or unique
lookup reads, and return `SurfaceRecordJson`, `SurfacePageJson`, or
`Option<SurfaceRecordJson>`. The same read DTOs can execute against
`ProjectSurfaceReadSession` without exposing its private store handle. Updates
admit stable update tags, decode point or singleton sparse update DTOs, and
return the runtime `surface.*` error type directly over either caller-supplied
checked program/store references or `ProjectSurfaceSession`.
Actions admit stable action tags, decode entry arguments through
`entry.invoke.v1`, execute the resolved public function through
`ProjectSurfaceSession`, and return captured output plus an optional surface
action value. Runtime action failures are sanitized as `surface.action`;
argument decode failures are `surface.request`.

The operation envelope functions compose those same typed bodies into a single
project-session dispatch profile. `execute_project_surface_operation_read_only`
accepts read bodies through `ProjectSurfaceReadSession` and rejects update or
action bodies as an ABI mismatch. `execute_project_surface_operation` accepts
read, sparse-update, and action bodies through `ProjectSurfaceSession`, using a
zero-capability `Host::new()` for actions. Callers that need clock, context,
log, filesystem, or maintenance capabilities use
`execute_project_surface_operation_with_host`. Both helpers return a standard
response envelope with record, page, optional-record, updated, or action
results. Error envelopes contain only a stable code and public message. The
project helpers use the session's private store handle and do not add routes,
generated clients, create/delete bodies, or opaque cursor token codecs.
Wrong profile versions fail before tag admission; unknown tags fail through
runtime admission; wrong read/update/action shape requests remain
`surface.request`; cursor mismatches stay on the existing cursor error path.

## Read next

- `crates/marrow-json/src/lib.rs` — `entry_return_to_json`,
  `saved_key_to_json`, `data_snapshot_stamp_to_json`,
  `DataSnapshotJson`, and `DataCommitJson`.
- `crates/marrow-json/src/surface.rs` — surface ABI descriptor DTOs, surface
  read result DTOs, checked surface read request-parameter and sparse update
  request DTOs, action DTOs, operation envelope DTOs, descriptor alias
  rendering, and in-process
  operation-tag execution helpers.
- `crates/marrow/src/cmd_run.rs` — the run JSON envelope and `run.entry_surface`
  mapping.
- `crates/marrow/src/trace.rs` and `crates/marrow/src/cmd_data/integrity.rs` —
  saved-key tooling consumers.
- `crates/marrow/src/cmd_data.rs` and `crates/marrow/src/cmd_data/get.rs` —
  data inspection envelopes that carry shared `store_snapshot` rendering.
