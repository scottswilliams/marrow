# Store

`marrow-store` is the bottom of the stack: a typed tree-cell storage contract (`TreeStore`) over a physical ordered-byte key/value grammar, with two interchangeable engines behind a private `Backend` trait. It assigns no language meaning — callers pass opaque `CatalogId`s and typed `SavedKey`/`Scalar` values; this crate owns only the durable byte forms and the rules that keep them stable across reopen.

## The one big idea

Keys are order-preserving; values are not. `SavedKey` encodes scalars so byte-lexicographic order equals Marrow's typed key order (sign-flipped big-endian numbers/dates/durations/instants, `0x00`-escaped strings/bytes), so every range scan runs without a comparator. Values use a separate canonical-but-unordered codec, because the store sorts by tree-cell key, not by value. Both forms are round-trip-exact: decoders re-encode and require byte-identical output, so non-canonical spellings (`+1`, `01`, `1.50`, `-0`, trailing-zero fractions) are rejected rather than normalized.

## Parts

- **Engine contract** — the private `Backend` trait (read/write/delete/scan/begin/commit/rollback/snapshot) plus `ScanPage` and typed `StoreError`. Two engines implement it: `MemStore` (BTreeMap) and `RedbStore` (persistent). A shared `conformance` suite runs 17 laws against both.
- **Key grammar** — `cell` defines the v0 physical key layout (placement prefix, profile, family, store id, identity, segments); `key` defines the order-preserving scalar codec used in every record-key and index-key position.
- **Value forms** — `value` (canonical scalar codec, calendar years 0001–9999) and `decimal` (exact base-10, 34-digit/34-place envelope, half-to-even division).
- **Public facade** — `TreeStore` wraps a boxed `Backend` and exposes every typed write/read/navigation/transaction/snapshot/backup call other crates use.
- **Durable receipts** — `metadata` (`CommitMetadata`, `EngineProfile`, the source digest the activation fence binds).
- **Catalog table** — `catalog` persists the accepted `marrow_catalog::CatalogMetadata` as a header row plus one row per entry in its own physical family (`FAMILY_CATALOG`), written in the caller's transaction and invisible to data/index/meta access; a read verifies the stored header against the decoded rows, accepts the canonical order-insensitive digest or the legacy order-sensitive row-order digest, and returns a snapshot normalized to the canonical digest.
- **Backup** — `backup` streams the data family only; index and meta cells are restamped on restore, never archived.

## Modules

| File | Responsibility |
|------|----------------|
| `crates/marrow-store/src/lib.rs` | Crate root; gates the redb engine behind the `native` feature; re-exports the public cell/key/value/tree surfaces. |
| `crates/marrow-store/src/backend.rs` | `StoreError` (stable dotted codes), `ScanPage`, and the private `Backend` trait every engine implements. |
| `crates/marrow-store/src/key.rs` | `SavedKey` and its order-preserving byte codec; identity-payload encode/decode for record identity and index entries. |
| `crates/marrow-store/src/value.rs` | `Scalar`/`SavedValue`/`ScalarType`, language spellings, and the canonical (unordered) value codec with strict round-trip-only decode. |
| `crates/marrow-store/src/decimal.rs` | Exact canonical base-10 `Decimal`: parse, `to_text`, checked add/sub/mul, long-division `checked_div`, floor/abs/compare. |
| `crates/marrow-store/src/cell.rs` | The v0 physical key grammar: `CatalogId` validation, `CellKey` constructors per family, `DataPathSegment`, `MetaCell` tags, key decoders, `CellRange`. |
| `crates/marrow-store/src/codec.rs` | Shared bounds-checked reader for private length-prefixed store codecs. |
| `crates/marrow-store/src/tree.rs` | `TreeStore` facade over a boxed `Backend`: metadata, the catalog-table read/replace surface, typed writes/reads, child/record/index navigation, paged index scans, backup streaming, snapshots. |
| `crates/marrow-store/src/metadata.rs` | `EngineProfile`, `CommitMetadata`, and their length-prefixed binary codec with bounded-count guards. |
| `crates/marrow-store/src/catalog.rs` | The accepted-catalog table codec: header/entry rows under `FAMILY_CATALOG`, bounded paged scan, ordinal-contiguity, canonical digest normalization, legacy order-sensitive digest compatibility, and read/replace through `TreeStore`. |
| `crates/marrow-store/src/mem.rs` | `MemStore`: in-memory `Backend` with full-map clone savepoints and a frozen pinned-read snapshot. |
| `crates/marrow-store/src/redb.rs` | `RedbStore`: persistent `Backend` with format-version stamp, undo-journal nesting (not redb savepoints), batched prefix delete, pinned read snapshots. |
| `crates/marrow-store/src/traversal.rs` | Shared prefix-scan page driver both engines feed to build a bounded, prefix-clipped, truncation-flagged `ScanPage`. |
| `crates/marrow-store/src/backup.rs` | `TreeBackupCell(Buf)`: data-only backup cell, framed target+value codec, FNV-64 checksum, bounded read guards. |
| `crates/marrow-store/src/conformance.rs` | Private `Backend` conformance suite (`run_all`): 17 laws run by both engines. |

## Invariants worth knowing before you touch bytes

- `0x00` is the only structural separator. Strings/bytes/ids escape it (`0x00` → `0x00 0x01`) and terminate with `0x00 0x00`; the typed-key tag band is held by compile-time asserts (`KEY_STR == 0x07`, `KEY_DATE == KEY_INT + 1`).
- Transactions are atomic across the whole staged plan. A mid-plan fault rolls the entire bracket back with no surviving write and no metadata stamp. `MemStore` clones the map; `RedbStore` runs one long-lived write transaction with per-level undo journals.
- A pinned read snapshot is mutually exclusive with writes on the same handle (offenders get `store.transaction`) and is non-reentrant, so a multi-page backup sees one coherent version.
- Scans are always bounded and resumable. A truncated page with no resume key is treated as corrupt scan state (`store.cursor`), never as the end.
- Catalog snapshot integrity is independent of declaration order. New headers store a digest over entries sorted by kind tag, path, stable ID, aliases, lifecycle tag, accepted key shape, and accepted structural signature. Reads also accept the legacy order-sensitive row-order digest when it matches the decoded rows, then normalize the returned snapshot to the canonical digest.
- Corruption is fail-closed and typed: malformed keys, value frames, metadata, or backup frames decode to `StoreError::Corruption`, never partial values. Decoders enforce exact-length consumption and bounded list counts.
- On-disk format is version-gated with no auto-migration: a mismatched `FORMAT_VERSION` is refused, a non-empty redb file with no `marrow.meta` table is corruption, a second writer is `store.locked`.
- Native open fails closed, never crashes: `RedbStore::open`/`open_read_only` run the redb open and its structural probe under a panic backstop (`catch_open`), so a truncated or torn body that drives redb into a layout assertion or btree `unreachable!()` becomes `store.corruption` instead of aborting the process. Redb open errors map by damage (`map_open_error`): a torn body to corruption, an unclean-shutdown repair-needed file to the typed `store.recovery_required` (a write-capable open attempts the replay and reports whether the store opened), a second writer to `store.locked`, everything else to `store.io`.
- Diagnostics are render-only: tests assert codes and typed payloads, not prose.

## Code-reality notes

- Every key carries an empty placement-prefix byte (`EMPTY_PLACEMENT_PREFIX`, `0x00`) — a reserved slot with no current variation, undocumented in the module prose.
- `decode_digest` exists in both `metadata.rs` and `tree.rs` as duplicate 8-byte try-into-or-corrupt helpers.
- `value.rs` maps `ErrorCode` and `string` both to `ScalarType::Str`; `from_scalar_name("ErrorCode")` resolves to `Str` but `name(Str)` returns `string`, so `ErrorCode` is a one-way alias.
- `TreeStore::open_read_only` is redb-only. The in-memory engine has no read-only mode, and the `Backend` trait does not express the capability — read-only is enforced at the redb layer.

## Read next

- `crates/marrow-store/src/tree.rs` — `TreeStore::memory` / `open` / `open_read_only` to construct; `write_data_value` / `read_data_value` / `delete_data_subtree` for the write/read primitives.
- `crates/marrow-store/src/tree.rs` — `scan_children_until` / `next_child_after` / `for_each_page_entry`: the one paged-scan-plus-decode engine all navigation routes through.
- `crates/marrow-store/src/cell.rs` — `decode_data_cell_key` / `CellKey::data_path_value` / `family`: the authoritative v0 key grammar.
- `crates/marrow-store/src/key.rs` — `encode_key_into` / `encode_escaped_bytes`: why stored byte order equals typed key order.
- `crates/marrow-store/src/redb.rs` — `RedbStore::mutate` / `commit` / `rollback`: the undo-journal model behind atomic rollback on the persistent engine.
