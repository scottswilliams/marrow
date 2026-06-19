# Shared JSON

`marrow-json` owns small JSON DTOs that more than one CLI, tool, or application
boundary needs. It exists to keep `marrow run --format json`, trace, data
integrity, store-backed data inspection, and surface reads, updates, and
descriptor export from copying entry-return, saved-key, data-snapshot, surface
descriptor/result rendering, and checked surface read request-parameter and
sparse update request-body decode logic. For surfaces it also provides an
in-process operation-tag execution boundary over caller-supplied
`CheckedProgram` and `TreeStore` references; it does not own serving or store
lifetime policy.

The crate deliberately does not define a general `Value` JSON ABI. Its entry
return renderer preserves the current CLI-compatible result surface: scalars,
enums, identities, and sequences render; whole resources and local trees fault
at the CLI boundary as `run.entry_surface`. The enum form remains the existing
CLI numeric `enum_id` / `member_id` profile, and `int` values remain JSON
numbers for compatibility with current CLI consumers. Its data-snapshot DTO
renders the shared `store_snapshot` object for `marrow data roots|get`, including
the store UID, catalog digest, optional commit stamp, and checked source digest.
Its surface DTOs render `marrow-run` surface records, pages, values,
identities, and typed cursors with accepted catalog IDs, typed keys, base64
bytes, and lossless strings for integers, temporal nanoseconds, and decimals.
Cursor/page rendering is context-aware: enum and identity-typed index arguments
render as branded surface arguments instead of raw saved key bytes or plain
member strings.

`SurfaceAbiJson` renders the successful `marrow check --format json|jsonl`
`surface_abi` object from checker-owned facts. It includes display-only module
and surface labels, typed catalog status, stable read descriptors, and an
optional sparse update descriptor for stable surfaces with a non-empty update
set. Source-only surfaces serialize blocker strings and no operation
descriptors. Update fields expose `backing_required` only as backing-field
metadata; sparse update request bodies remain non-empty patches and no field is
mandatory on every request.

Inbound surface request parameters and sparse update bodies are checked against
the admitted runtime surface shape. `SurfacePointRequestJson`,
`SurfacePageRequestJson`, `SurfaceUniqueLookupRequestJson`,
`SurfaceArgumentJson`, and `SurfaceCursorJson` decode read identities, exact
index arguments, unique lookup keys, limits, and typed cursor boundaries into
runtime `SavedKey` and cursor values. `SurfacePointUpdateRequestJson`,
`SurfaceSingletonUpdateRequestJson`, `SurfaceUpdateFieldJson`, and
`SurfaceUpdateValueJson` decode update identities, field catalog IDs, and
canonical scalar/enum/identity values into runtime `SurfaceUpdateInput` values.
JSON decode is structural and canonical only: runtime `SurfaceUpdate` owns
declared update-set authorization, duplicate and non-empty patch validation,
exact value shape checks, enum membership and selectability, identity store,
arity, and key-scalar validation, record presence, and post-patch footprint
validation. HTTP routes, opaque cursor tokens, generated clients,
entry-argument JSON decode, and create/delete body decode remain outside this
crate's current profile.

The operation-tag execution functions compose those DTOs with `marrow-run`
admission. Reads admit stable read tags, decode the point/page/unique request
DTO against the admitted handle, execute singleton, point, page, or unique
lookup reads, and return `SurfaceRecordJson`, `SurfacePageJson`, or
`Option<SurfaceRecordJson>`. Updates admit stable update tags, decode point or
singleton sparse update DTOs, and return the runtime `surface.*` error type
directly. Wrong-profile or unknown tags fail through runtime admission; wrong
read/update shape requests remain `surface.request`; cursor mismatches stay on
the existing cursor error path.

## Read next

- `crates/marrow-json/src/lib.rs` — `entry_return_to_json`,
  `saved_key_to_json`, `data_snapshot_stamp_to_json`,
  `DataSnapshotJson`, and `DataCommitJson`.
- `crates/marrow-json/src/surface.rs` — surface ABI descriptor DTOs, surface
  read result DTOs, checked surface read request-parameter and sparse update
  request DTOs, and in-process operation-tag execution helpers.
- `crates/marrow/src/cmd_run.rs` — the run JSON envelope and `run.entry_surface`
  mapping.
- `crates/marrow/src/trace.rs` and `crates/marrow/src/cmd_data/integrity.rs` —
  saved-key tooling consumers.
- `crates/marrow/src/cmd_data.rs` and `crates/marrow/src/cmd_data/get.rs` —
  data inspection envelopes that carry shared `store_snapshot` rendering.
