//! Private row codec for the engine catalog family.
//!
//! The accepted catalog persists as one header row plus one row per entry, all
//! under [`CellKey::catalog_family`]. A read verifies the stored digest against
//! the decoded entries before returning a normalized snapshot: any tampered entry
//! row, even one that decodes into a structurally valid form, fails closed as
//! corruption.

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};

use crate::backend::{Backend, StoreError};
use crate::cell::CellKey;
use crate::codec::BoundedReader;

const CATALOG_SCAN_PAGE: usize = 1024;
const HEADER_ROW: u8 = 0x00;
const ENTRY_ROW: u8 = 0x10;
const ROW_VALUE_VERSION_V0: u8 = 0;
const MIN_LENGTH_PREFIX_BYTES: usize = 4;

struct Header {
    epoch: u64,
    digest: String,
}

struct EntryRow {
    ordinal: u64,
    entry: CatalogEntry,
}

enum CatalogRow {
    Header(Header),
    Entry(Box<EntryRow>),
}

pub(crate) fn read_catalog_snapshot(
    backend: &(impl Backend + ?Sized),
) -> Result<Option<CatalogMetadata>, StoreError> {
    let mut header = None;
    let mut entries = Vec::new();
    for row in scan_catalog_rows(backend)? {
        match row {
            CatalogRow::Header(next) => header = Some(next),
            CatalogRow::Entry(entry) => entries.push(*entry),
        }
    }
    let Some(header) = header else {
        if entries.is_empty() {
            return Ok(None);
        }
        return Err(corrupt_catalog(
            "catalog entries exist without a header row",
        ));
    };
    entries.sort_by_key(|entry| entry.ordinal);
    for (expected, entry) in entries.iter().enumerate() {
        if entry.ordinal != expected as u64 {
            return Err(corrupt_catalog("catalog entry ordinals are not contiguous"));
        }
    }
    let entries = entries.into_iter().map(|row| row.entry).collect();
    // Rebuilding through the catalog normalizer recomputes the canonical digest from the
    // decoded entries, so any header mismatch proves the snapshot is stale or tampered even
    // when every row still decodes into a structurally valid entry.
    CatalogMetadata::from_stored_parts(header.epoch, header.digest, entries)
        .map(Some)
        .map_err(|_| corrupt_catalog("catalog digest does not match stored entries"))
}

pub(crate) fn read_catalog_snapshot_digest(
    backend: &(impl Backend + ?Sized),
) -> Result<Option<String>, StoreError> {
    Ok(read_catalog_snapshot(backend)?.map(|snapshot| snapshot.digest))
}

pub(crate) fn replace_catalog_snapshot(
    backend: &mut (impl Backend + ?Sized),
    snapshot: &CatalogMetadata,
) -> Result<(), StoreError> {
    snapshot
        .validate()
        .map_err(|error| corrupt_catalog(error.message))?;
    // Replacing the family deletes every prior row and rewrites the header and
    // entries, so it must be one transaction: a failure partway through would
    // otherwise leave the catalog with no header or a partial entry set. Nested
    // inside an apply transaction this defers durability to the outer commit, so
    // catalog rows still publish atomically with data, indexes, and metadata.
    backend.begin()?;
    match write_catalog_rows(backend, snapshot) {
        Ok(()) => backend.commit(),
        Err(error) => {
            let _ = backend.rollback();
            Err(error)
        }
    }
}

fn write_catalog_rows(
    backend: &mut (impl Backend + ?Sized),
    snapshot: &CatalogMetadata,
) -> Result<(), StoreError> {
    backend.delete(&catalog_family())?;
    backend.write(&catalog_header_key(), encode_header(snapshot)?)?;
    for (ordinal, entry) in snapshot.entries.iter().enumerate() {
        backend.write(
            &catalog_entry_key(&entry.stable_id),
            encode_entry(ordinal as u64, entry)?,
        )?;
    }
    Ok(())
}

fn scan_catalog_rows(backend: &(impl Backend + ?Sized)) -> Result<Vec<CatalogRow>, StoreError> {
    let prefix = catalog_family();
    let mut rows = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let page = match cursor.as_ref() {
            Some(cursor) => backend.scan_after(&prefix, cursor, CATALOG_SCAN_PAGE)?,
            None => backend.scan(&prefix, CATALOG_SCAN_PAGE)?,
        };
        cursor = page.entries.last().map(|(key, _)| key.clone());
        for (key, value) in page.entries {
            rows.push(decode_row(&prefix, &key, &value)?);
        }
        if !page.truncated {
            break;
        }
        if cursor.is_none() {
            return Err(StoreError::InvalidCursor {
                message: "catalog scan page was truncated without a cursor".into(),
            });
        }
    }
    Ok(rows)
}

fn decode_row(prefix: &[u8], key: &[u8], value: &[u8]) -> Result<CatalogRow, StoreError> {
    let tail = key
        .strip_prefix(prefix)
        .ok_or_else(|| corrupt_catalog("catalog row key has the wrong family"))?;
    match tail.split_first() {
        Some((&HEADER_ROW, [])) => Ok(CatalogRow::Header(decode_header(value)?)),
        Some((&ENTRY_ROW, stable_id)) => {
            let stable_id = decode_stable_id(stable_id)?;
            Ok(CatalogRow::Entry(Box::new(decode_entry(stable_id, value)?)))
        }
        _ => Err(corrupt_catalog("catalog row key is malformed")),
    }
}

fn encode_header(snapshot: &CatalogMetadata) -> Result<Vec<u8>, StoreError> {
    let mut out = vec![ROW_VALUE_VERSION_V0];
    out.extend_from_slice(&snapshot.epoch.to_be_bytes());
    put_text(&snapshot.digest, &mut out)?;
    Ok(out)
}

fn decode_header(bytes: &[u8]) -> Result<Header, StoreError> {
    let mut cursor = BoundedReader::new(bytes, corrupt_catalog_truncated);
    take_version(&mut cursor)?;
    let epoch = cursor.take_u64()?;
    let digest = take_text(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(corrupt_catalog("catalog header has trailing bytes"));
    }
    Ok(Header { epoch, digest })
}

fn encode_entry(ordinal: u64, entry: &CatalogEntry) -> Result<Vec<u8>, StoreError> {
    let mut out = vec![ROW_VALUE_VERSION_V0];
    out.extend_from_slice(&ordinal.to_be_bytes());
    out.push(entry.kind.tag());
    put_text(&entry.path, &mut out)?;
    put_texts(entry.aliases.iter().map(String::as_str), &mut out)?;
    out.push(entry.lifecycle.tag());
    put_optional_text(entry.accepted_key_shape.as_deref(), &mut out)?;
    put_optional_text(entry.accepted_struct.as_deref(), &mut out)?;
    put_optional_text(entry.accepted_index_shape.as_deref(), &mut out)?;
    Ok(out)
}

fn decode_entry(stable_id: &str, bytes: &[u8]) -> Result<EntryRow, StoreError> {
    let mut cursor = BoundedReader::new(bytes, corrupt_catalog_truncated);
    take_version(&mut cursor)?;
    let ordinal = cursor.take_u64()?;
    let kind = decode_kind(cursor.take_u8()?)?;
    let path = take_text(&mut cursor)?;
    let aliases = take_texts(&mut cursor)?;
    let lifecycle = decode_lifecycle(cursor.take_u8()?)?;
    let accepted_key_shape = take_optional_text(&mut cursor)?;
    let accepted_struct = take_optional_text(&mut cursor)?;
    let accepted_index_shape = take_optional_text(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(corrupt_catalog("catalog entry has trailing bytes"));
    }
    let entry = CatalogEntry {
        kind,
        path,
        stable_id: stable_id.to_string(),
        aliases,
        lifecycle,
        accepted_key_shape,
        accepted_index_shape,
        accepted_struct,
    };
    Ok(EntryRow { ordinal, entry })
}

fn catalog_family() -> Vec<u8> {
    CellKey::catalog_family().into_bytes()
}

fn catalog_header_key() -> Vec<u8> {
    let mut key = catalog_family();
    key.push(HEADER_ROW);
    key
}

fn catalog_entry_key(stable_id: &str) -> Vec<u8> {
    let mut key = catalog_family();
    key.push(ENTRY_ROW);
    key.extend_from_slice(stable_id.as_bytes());
    key
}

fn decode_stable_id(bytes: &[u8]) -> Result<&str, StoreError> {
    std::str::from_utf8(bytes).map_err(|_| corrupt_catalog("catalog stable ID key is not UTF-8"))
}

fn put_text(value: &str, out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(value.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "catalog text length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_texts<'a>(
    values: impl IntoIterator<Item = &'a str>,
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    let values = values.into_iter().collect::<Vec<_>>();
    let len = u32::try_from(values.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "catalog text count",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for value in values {
        put_text(value, out)?;
    }
    Ok(())
}

fn put_optional_text(value: Option<&str>, out: &mut Vec<u8>) -> Result<(), StoreError> {
    match value {
        Some(value) => {
            out.push(1);
            put_text(value, out)
        }
        None => {
            out.push(0);
            Ok(())
        }
    }
}

fn decode_kind(tag: u8) -> Result<CatalogEntryKind, StoreError> {
    CatalogEntryKind::from_tag(tag)
        .ok_or_else(|| corrupt_catalog("catalog entry kind tag is unknown"))
}

fn decode_lifecycle(tag: u8) -> Result<CatalogLifecycle, StoreError> {
    CatalogLifecycle::from_tag(tag)
        .ok_or_else(|| corrupt_catalog("catalog lifecycle tag is unknown"))
}

type CatalogReader<'a> = BoundedReader<'a, StoreError>;

fn take_version(cursor: &mut CatalogReader<'_>) -> Result<(), StoreError> {
    let version = cursor.take_u8()?;
    if version == ROW_VALUE_VERSION_V0 {
        Ok(())
    } else {
        Err(corrupt_catalog("catalog row version is unknown"))
    }
}

fn take_text(cursor: &mut CatalogReader<'_>) -> Result<String, StoreError> {
    let bytes = cursor.take_prefixed_bytes()?;
    String::from_utf8(bytes.to_vec()).map_err(|_| corrupt_catalog("catalog text is not UTF-8"))
}

fn take_texts(cursor: &mut CatalogReader<'_>) -> Result<Vec<String>, StoreError> {
    let len =
        cursor.take_bounded_count_with(MIN_LENGTH_PREFIX_BYTES, corrupt_catalog_text_count)?;
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(take_text(cursor)?);
    }
    Ok(values)
}

fn take_optional_text(cursor: &mut CatalogReader<'_>) -> Result<Option<String>, StoreError> {
    match cursor.take_u8()? {
        0 => Ok(None),
        1 => Ok(Some(take_text(cursor)?)),
        _ => Err(corrupt_catalog("catalog optional text tag is unknown")),
    }
}

fn corrupt_catalog_truncated(_: &[u8]) -> StoreError {
    corrupt_catalog("catalog row is truncated")
}

fn corrupt_catalog_text_count(_: &[u8]) -> StoreError {
    corrupt_catalog("catalog text count is malformed")
}

fn corrupt_catalog(message: impl Into<String>) -> StoreError {
    StoreError::Corruption {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};

    use super::{
        ENTRY_ROW, HEADER_ROW, ROW_VALUE_VERSION_V0, encode_entry, encode_header, put_text,
        read_catalog_snapshot, read_catalog_snapshot_digest, replace_catalog_snapshot,
    };
    use crate::backend::Backend;
    use crate::mem::MemStore;

    fn stable_id(suffix: u8) -> String {
        format!("cat_{:032x}", suffix)
    }

    /// A snapshot exercising aliases, a store key shape, a store-index shape, and a member
    /// structural signature, so every optional field has a populated round-trip.
    fn sample_snapshot() -> CatalogMetadata {
        CatalogMetadata::new(
            7,
            vec![
                CatalogEntry {
                    kind: CatalogEntryKind::Store,
                    path: "books".to_string(),
                    stable_id: stable_id(1),
                    aliases: vec!["library".to_string(), "tomes".to_string()],
                    lifecycle: CatalogLifecycle::Active,
                    accepted_key_shape: Some("int,string".to_string()),
                    accepted_index_shape: None,
                    accepted_struct: None,
                },
                CatalogEntry {
                    kind: CatalogEntryKind::StoreIndex,
                    path: "books.byTitle".to_string(),
                    stable_id: stable_id(3),
                    aliases: Vec::new(),
                    lifecycle: CatalogLifecycle::Active,
                    accepted_key_shape: None,
                    accepted_index_shape: Some(
                        "unique=false;keys=[member:cat_00000000000000000000000000000002:string]"
                            .to_string(),
                    ),
                    accepted_struct: None,
                },
                CatalogEntry {
                    kind: CatalogEntryKind::ResourceMember,
                    path: "books.title".to_string(),
                    stable_id: stable_id(2),
                    aliases: Vec::new(),
                    lifecycle: CatalogLifecycle::Reserved,
                    accepted_key_shape: None,
                    accepted_index_shape: None,
                    accepted_struct: Some("leaf:string".to_string()),
                },
            ],
        )
    }

    #[test]
    fn a_snapshot_round_trips_through_the_codec() {
        let snapshot = sample_snapshot();
        let mut backend = MemStore::default();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write snapshot");

        let read = read_catalog_snapshot(&backend)
            .expect("read snapshot")
            .expect("snapshot present");
        assert_eq!(read, snapshot);
        assert_eq!(
            read_catalog_snapshot_digest(&backend).expect("read digest"),
            Some(snapshot.digest.clone())
        );
    }

    #[test]
    fn an_empty_catalog_family_reads_as_none() {
        let backend = MemStore::default();
        assert_eq!(read_catalog_snapshot(&backend).expect("read"), None);
        assert_eq!(
            read_catalog_snapshot_digest(&backend).expect("read digest"),
            None
        );
    }

    #[test]
    fn replacing_a_snapshot_clears_the_prior_entries() {
        let mut backend = MemStore::default();
        replace_catalog_snapshot(&mut backend, &sample_snapshot()).expect("write first");

        let smaller = CatalogMetadata::new(
            8,
            vec![CatalogEntry {
                kind: CatalogEntryKind::Resource,
                path: "authors".to_string(),
                stable_id: stable_id(3),
                aliases: Vec::new(),
                lifecycle: CatalogLifecycle::Active,
                accepted_key_shape: None,
                accepted_index_shape: None,
                accepted_struct: None,
            }],
        );
        replace_catalog_snapshot(&mut backend, &smaller).expect("write second");

        let read = read_catalog_snapshot(&backend)
            .expect("read")
            .expect("present");
        assert_eq!(read, smaller, "the replacement leaves no stale entry rows");
    }

    fn header_key() -> Vec<u8> {
        let mut key = super::catalog_family();
        key.push(HEADER_ROW);
        key
    }

    fn entry_key(stable_id: &str) -> Vec<u8> {
        super::catalog_entry_key(stable_id)
    }

    #[test]
    fn an_entry_row_without_a_header_is_corruption() {
        let mut backend = MemStore::default();
        let id = stable_id(1);
        let entry = CatalogEntry {
            kind: CatalogEntryKind::Store,
            path: "books".to_string(),
            stable_id: id.clone(),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: None,
        };
        backend
            .write(&entry_key(&id), encode_entry(0, &entry).expect("encode"))
            .expect("seed orphan entry");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn a_truncated_entry_row_is_corruption() {
        let mut backend = MemStore::default();
        let snapshot = sample_snapshot();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write");

        let id = &snapshot.entries[0].stable_id;
        let mut bytes = encode_entry(0, &snapshot.entries[0]).expect("encode");
        bytes.truncate(bytes.len() - 2);
        backend.write(&entry_key(id), bytes).expect("corrupt row");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn a_non_utf8_path_in_an_entry_row_is_corruption() {
        let mut backend = MemStore::default();
        let snapshot = sample_snapshot();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write");

        // Hand-build an entry row whose path length frame promises bytes that are not
        // valid UTF-8, so the text decode rejects the row.
        let id = &snapshot.entries[0].stable_id;
        let mut bytes = vec![ROW_VALUE_VERSION_V0];
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.push(CatalogEntryKind::Store.tag());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.push(0xff);
        backend.write(&entry_key(id), bytes).expect("corrupt row");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn a_short_entry_row_without_index_shape_is_corruption() {
        let mut backend = MemStore::default();
        let id = stable_id(1);
        let snapshot = CatalogMetadata::new(
            7,
            vec![CatalogEntry {
                kind: CatalogEntryKind::StoreIndex,
                path: "books.byTitle".to_string(),
                stable_id: id.clone(),
                aliases: Vec::new(),
                lifecycle: CatalogLifecycle::Active,
                accepted_key_shape: None,
                accepted_index_shape: None,
                accepted_struct: None,
            }],
        );
        backend
            .write(&header_key(), encode_header(&snapshot).expect("header"))
            .expect("seed header");

        let mut bytes = vec![ROW_VALUE_VERSION_V0];
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.push(CatalogEntryKind::StoreIndex.tag());
        put_text("books.byTitle", &mut bytes).expect("path");
        bytes.extend_from_slice(&0u32.to_be_bytes()); // no aliases
        bytes.push(CatalogLifecycle::Active.tag());
        bytes.push(0); // no key shape
        bytes.push(0); // no structural signature
        backend
            .write(&entry_key(&id), bytes)
            .expect("seed short entry");

        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn an_unknown_kind_tag_is_corruption() {
        let mut backend = MemStore::default();
        let snapshot = sample_snapshot();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write");

        let id = &snapshot.entries[0].stable_id;
        let mut bytes = vec![ROW_VALUE_VERSION_V0];
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.push(99); // no kind tag is 99
        put_text("books", &mut bytes).expect("path");
        bytes.extend_from_slice(&0u32.to_be_bytes()); // no aliases
        bytes.push(CatalogLifecycle::Active.tag());
        bytes.push(0); // no key shape
        bytes.push(0); // no struct
        backend.write(&entry_key(id), bytes).expect("corrupt row");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn a_non_contiguous_ordinal_is_corruption() {
        let mut backend = MemStore::default();
        let snapshot = sample_snapshot();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write");

        // Re-encode the first entry with an ordinal of 5, so the ordinals are {5, 1}:
        // sorted they are 1 then 5, which is not contiguous from zero.
        let id = &snapshot.entries[0].stable_id;
        let bytes = encode_entry(5, &snapshot.entries[0]).expect("encode");
        backend.write(&entry_key(id), bytes).expect("corrupt row");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn tampering_one_entry_path_against_the_stored_header_is_corruption() {
        let mut backend = MemStore::default();
        let snapshot = sample_snapshot();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write");

        // Rewrite the first entry to a different but structurally valid path while
        // leaving the header digest untouched. The header digest was computed over the
        // original entries, so the recompute-compare on read must reject the change.
        let id = &snapshot.entries[0].stable_id;
        let tampered = CatalogEntry {
            path: "magazines".to_string(),
            ..snapshot.entries[0].clone()
        };
        backend
            .write(&entry_key(id), encode_entry(0, &tampered).expect("encode"))
            .expect("tamper row");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    #[test]
    fn an_unknown_header_version_is_corruption() {
        let mut backend = MemStore::default();
        let snapshot = sample_snapshot();
        replace_catalog_snapshot(&mut backend, &snapshot).expect("write");

        let mut bytes = vec![0xff];
        bytes.extend_from_slice(&snapshot.epoch.to_be_bytes());
        put_text(&snapshot.digest, &mut bytes).expect("digest");
        backend.write(&header_key(), bytes).expect("corrupt header");
        assert_corruption(read_catalog_snapshot(&backend));
    }

    fn assert_corruption<T: std::fmt::Debug>(result: Result<T, crate::StoreError>) {
        let error = result.expect_err("expected corruption");
        assert_eq!(error.code(), "store.corruption", "{error:?}");
    }

    // The entry-row tag is referenced indirectly through `catalog_entry_key`; this
    // anchors the constant so a tag renumber is caught here too.
    #[test]
    fn entry_key_carries_the_entry_row_tag() {
        let key = entry_key(&stable_id(1));
        let tail = key
            .strip_prefix(super::catalog_family().as_slice())
            .unwrap();
        assert_eq!(tail.first(), Some(&ENTRY_ROW));
    }
}
