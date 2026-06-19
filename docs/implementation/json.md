# Shared outbound JSON

`marrow-json` owns small, outbound JSON renderers that more than one CLI, tool,
or application boundary needs. It exists to keep `marrow run --format json`,
trace, data integrity, store-backed data inspection, and surface read results
from copying entry-return, saved-key, data-snapshot, and surface DTO rendering
logic.

The crate deliberately does not define a general `Value` JSON ABI. Its entry
return renderer preserves the current CLI-compatible result surface: scalars,
enums, identities, and sequences render; whole resources and local trees fault
at the CLI boundary as `run.entry_surface`. The enum form remains the existing
CLI numeric `enum_id` / `member_id` profile, and `int` values remain JSON
numbers for compatibility with current CLI consumers. Its data-snapshot DTO
renders the shared `store_snapshot` object for `marrow data roots|get`, including
the store UID, catalog digest, optional commit stamp, and checked source digest.
Its surface read-result DTOs render already-executed `marrow-run` surface
records, pages, values, identities, and typed cursors with accepted catalog IDs,
typed keys, base64 bytes, and lossless strings for integers, temporal
nanoseconds, and decimals.

Inbound request-parameter decoding is a separate checked operation. A safe
decoder needs the expected entry parameter type or surface operation fact, store
identity root, key arity, key scalar types, enum facts, and temporal validation
rules from the checked program. Generated clients, cursor tokens, and write-body
decode remain outside this crate's current profile.

## Read next

- `crates/marrow-json/src/lib.rs` — `entry_return_to_json`,
  `saved_key_to_json`, `data_snapshot_stamp_to_json`,
  `DataSnapshotJson`, and `DataCommitJson`.
- `crates/marrow-json/src/surface.rs` — `SurfaceRecordJson`,
  `SurfacePageJson`, `SurfaceCursorJson`, and the surface read DTO leaves.
- `crates/marrow/src/cmd_run.rs` — the run JSON envelope and `run.entry_surface`
  mapping.
- `crates/marrow/src/trace.rs` and `crates/marrow/src/cmd_data/integrity.rs` —
  saved-key tooling consumers.
- `crates/marrow/src/cmd_data.rs` and `crates/marrow/src/cmd_data/get.rs` —
  data inspection envelopes that carry shared `store_snapshot` rendering.
