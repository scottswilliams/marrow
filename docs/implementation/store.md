# Store

`marrow-store` is the bottom of the stack: a typed tree-cell storage contract (`TreeStore`) over a physical ordered-byte key/value grammar, with two interchangeable engines behind a private `Backend` trait. It assigns no language meaning — callers pass opaque `CatalogId`s and typed `SavedKey`/`Scalar` values; this crate owns only the durable byte forms and the rules that keep them stable across reopen.

## The one big idea

Keys are order-preserving; values are not. `SavedKey` encodes scalars so byte-lexicographic order equals Marrow's typed key order (sign-flipped big-endian numbers/dates/durations/instants, `0x00`-escaped strings/bytes), so every range scan runs without a comparator. Values use a separate canonical-but-unordered codec, because the store sorts by tree-cell key, not by value. Both forms are round-trip-exact: decoders re-encode and require byte-identical output, so non-canonical spellings (`+1`, `01`, `1.50`, `-0`, trailing-zero fractions) are rejected rather than normalized.

## Parts

- **Engine contract** — the private `Backend` trait (read/write/delete/scan/begin/commit/rollback/snapshot) plus `ScanPage` and typed `StoreError`. Two engines implement it: `MemStore` (BTreeMap) and `RedbStore` (persistent). A shared `conformance` suite runs 19 laws against both.
- **Key grammar** — `cell` defines the v0 physical key layout (placement prefix, profile, family, store id, identity, segments); `key` defines the order-preserving scalar codec used in every record-key and index-key position.
- **Value forms** — `value` (canonical scalar codec, calendar years 0001–9999) and `decimal` (exact base-10, 34-digit/34-place envelope, half-to-even division and scale rounding).
- **Public facade** — `TreeStore` wraps a boxed `Backend` and exposes every typed write/read/navigation/transaction/snapshot/backup call other crates use.
- **Durable stamp metadata** — `metadata` (`CommitMetadata`, `EngineProfile`, `StoreUid`, and the source digest the activation fence binds).
- **Catalog table** — `catalog` persists the accepted
  `marrow_catalog::CatalogMetadata` as a header row plus one row per entry in
  its own physical family (`FAMILY_CATALOG`), written in the caller's
  transaction and invisible to data/index/meta access. A read verifies the stored
  header against the decoded rows, recognizes the canonical digest and matching
  row-order header digest, and returns a snapshot normalized to the canonical
  digest.
- **Backup** — `backup` streams data-family record nodes, keyed group-entry path nodes, and value cells; index cells are rebuilt and commit metadata is restamped from the manifest on restore, never archived.

## Modules

| File | Responsibility |
|------|----------------|
| `crates/marrow-store/src/lib.rs` | Crate root; gates the redb engine behind the `native` feature; re-exports the public cell/key/value/tree surfaces. |
| `crates/marrow-store/src/backend.rs` | `StoreError` (stable dotted codes), `ScanPage`, and the private `Backend` trait every engine implements. |
| `crates/marrow-store/src/key.rs` | `SavedKey` and its order-preserving byte codec; identity-payload encode/decode for record identity and index entries. |
| `crates/marrow-store/src/value.rs` | `Scalar`/`SavedValue`/`ScalarType`, language spellings, and the canonical (unordered) value codec with strict round-trip-only decode. |
| `crates/marrow-store/src/decimal.rs` | Exact canonical base-10 `Decimal`: parse, `to_text`, checked add/sub/mul, long-division `checked_div`, half-even `round_to_scale`, floor/abs/compare. |
| `crates/marrow-store/src/cell.rs` | The v0 physical key grammar: `CatalogId` validation, `CellKey` constructors per family, `DataPathSegment`, `MetaCell` tags, key decoders, `CellRange`. |
| `crates/marrow-store/src/codec.rs` | Shared bounds-checked reader for private length-prefixed store codecs. |
| `crates/marrow-store/src/tree.rs` | `TreeStore` facade over a boxed `Backend`: metadata, the catalog-table read/replace surface, typed writes/reads, node-backed record navigation, child/index navigation, paged index scans, backup streaming, snapshots. |
| `crates/marrow-store/src/metadata.rs` | `EngineProfile`, `CommitMetadata`, `StoreUid`, and their length-prefixed binary codecs with bounded-count guards. |
| `crates/marrow-store/src/catalog.rs` | Accepted-catalog table codec: header/entry rows, bounded paged scan, ordinal checks, digest normalization, row-order header recognition, and read/replace through `TreeStore`. |
| `crates/marrow-store/src/mem.rs` | `MemStore`: in-memory `Backend` with one full-map clone for the open flat transaction and a frozen pinned-read snapshot. |
| `crates/marrow-store/src/redb.rs` | `RedbStore`: persistent `Backend` with explicit immediate-durability redb transactions, format-version stamp plus parent-directory sync on fresh creation, joined transaction depth, batched prefix delete, pinned read snapshots. |
| `crates/marrow-store/src/traversal.rs` | Shared prefix-scan page driver both engines feed to build a bounded, prefix-clipped, truncation-flagged `ScanPage`. |
| `crates/marrow-store/src/backup.rs` | `TreeBackupCell(Buf)`: data-only backup cell, framed target+value codec, FNV-64 checksum, bounded read guards. |
| `crates/marrow-store/src/conformance.rs` | Private `Backend` conformance suite (`run_all`): 19 laws run by both engines. |

## Invariants worth knowing before you touch bytes

- `0x00` is the only structural separator. Strings/bytes/ids escape it (`0x00` → `0x00 0x01`) and terminate with `0x00 0x00`; the typed-key tag band is held by compile-time asserts (`KEY_STR == 0x07`, `KEY_DATE == KEY_INT + 1`).
- Transactions are atomic across the whole staged plan. A mid-plan fault rolls the entire bracket back with no surviving write and no metadata stamp. `MemStore` snapshots once per flat transaction; `RedbStore` runs one long-lived write transaction and aborts it on rollback.
- Native redb commits pin immediate durability and keep redb's one-phase commit posture. Fresh store creation fsyncs the containing directory after the first format-stamp commit.
- A pinned read snapshot is mutually exclusive with writes on the same handle (offenders get `store.transaction`) and is non-reentrant, so a multi-page backup sees one coherent version.
- Scans are always bounded and resumable. A truncated page with no resume key is treated as corrupt scan state (`store.cursor`), never as the end.
- Catalog snapshot integrity is independent of declaration order. New headers
  store a digest over entries sorted by kind tag, path, stable ID, aliases,
  lifecycle tag, accepted store-key shape, accepted store-index shape, and
  accepted structural signature. Reads also recognize a matching row-order
  digest, then normalize the returned snapshot to the canonical digest.
- Corruption is fail-closed and typed: malformed keys, value frames, metadata, or backup frames decode to `StoreError::Corruption`, never partial values. Decoders enforce exact-length consumption and bounded list counts.
- On-disk format is version-gated with no automatic conversion: a mismatched `FORMAT_VERSION` is refused, a non-empty redb file with no `marrow.meta` table is corruption, and a read/write lock conflict is `store.locked`.
- Native open fails closed, never crashes: `RedbStore::open`/`open_read_only` run the redb open and its structural probe under a panic backstop (`catch_open`), so a truncated or torn body that drives redb into a layout assertion or btree `unreachable!()` becomes `store.corruption` instead of aborting the process. Redb open errors map by damage (`map_open_error`): a torn body to corruption, an unclean-shutdown repair-needed file to the typed `store.recovery_required` (a write-capable open attempts the replay and reports whether the store opened), a read/write holder conflict to `store.locked`, everything else to `store.io`.
- Diagnostics are render-only: tests assert codes and typed payloads, not prose.

## Read next

- `crates/marrow-store/src/tree.rs` — `TreeStore::memory` / `open` / `open_read_only` to construct; `write_data_node` / `write_data_value` / `read_data_value` / `delete_data_subtree` for the write/read primitives. A data path node is a hidden group-entry presence cell at the path prefix; payload reads only use the value-suffixed key.
- `crates/marrow-store/src/tree.rs` — `scan_children_until` / `next_child_after` / `for_each_page_entry`: the one paged-scan-plus-decode engine all navigation routes through.
- `crates/marrow-store/src/cell.rs` — `decode_data_cell_key` / `CellKey::data_path_prefix` / `CellKey::data_path_value` / `family`: the authoritative v0 key grammar.
- `crates/marrow-store/src/key.rs` — `encode_key_into` / `encode_escaped_bytes`: why stored byte order equals typed key order.
- `crates/marrow-store/src/redb.rs` — `RedbStore::mutate` / `commit` / `rollback`: the flat joined transaction model behind atomic rollback on the persistent engine.
