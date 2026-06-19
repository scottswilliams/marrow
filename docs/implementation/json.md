# Shared JSON

`marrow-json` owns small JSON DTOs that more than one CLI, tool, or application
boundary needs. It exists to keep `marrow run --format json`, trace, data
integrity, store-backed data inspection, and surface reads from copying
entry-return, saved-key, data-snapshot, surface result rendering, and checked
surface read request-parameter decode logic.

The crate deliberately does not define a general `Value` JSON ABI. Its entry
return renderer preserves the current CLI-compatible result surface: scalars,
enums, identities, and sequences render; whole resources and local trees fault
at the CLI boundary as `run.entry_surface`. The enum form remains the existing
CLI numeric `enum_id` / `member_id` profile, and `int` values remain JSON
numbers for compatibility with current CLI consumers. Its data-snapshot DTO
renders the shared `store_snapshot` object for `marrow data roots|get`, including
the store UID, catalog digest, optional commit stamp, and checked source digest.
Its surface DTOs render already-executed `marrow-run` surface records, pages,
values, identities, and typed cursors with accepted catalog IDs, typed keys,
base64 bytes, and lossless strings for integers, temporal nanoseconds, and
decimals. Cursor/page rendering is context-aware: enum and identity-typed index
arguments render as branded surface arguments instead of raw saved key bytes or
plain member strings.

Inbound surface read request parameters are checked against the admitted runtime
surface read shape. `SurfacePointRequestJson`, `SurfacePageRequestJson`,
`SurfaceUniqueLookupRequestJson`, `SurfaceArgumentJson`, and `SurfaceCursorJson`
decode identities, exact index arguments, unique lookup keys, limits, and typed
cursor boundaries into runtime `SavedKey` and cursor values. The runtime remains
the semantic owner of store identity, arity, scalar key types, enum membership,
identity index-key encoding, and cursor boundary shape. HTTP routes, opaque
cursor tokens, generated clients, entry-argument JSON decode, and generated
write-body decode remain outside this crate's current profile.

## Read next

- `crates/marrow-json/src/lib.rs` — `entry_return_to_json`,
  `saved_key_to_json`, `data_snapshot_stamp_to_json`,
  `DataSnapshotJson`, and `DataCommitJson`.
- `crates/marrow-json/src/surface.rs` — surface read result DTOs plus checked
  surface read request-parameter DTOs.
- `crates/marrow/src/cmd_run.rs` — the run JSON envelope and `run.entry_surface`
  mapping.
- `crates/marrow/src/trace.rs` and `crates/marrow/src/cmd_data/integrity.rs` —
  saved-key tooling consumers.
- `crates/marrow/src/cmd_data.rs` and `crates/marrow/src/cmd_data/get.rs` —
  data inspection envelopes that carry shared `store_snapshot` rendering.
