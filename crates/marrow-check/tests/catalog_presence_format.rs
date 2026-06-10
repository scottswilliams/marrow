mod support;

use std::fs;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};

use support::catalog::{catalog, catalog_path, derived_id, entry as literal_entry, write_catalog};
use support::{config, temp_project};

/// A catalog entry whose stable id is minted deterministically from `label`, so a
/// fixture refers to a member by a readable name and still gets a `cat_`-shaped id the
/// catalog parser accepts. Tests that need a specific literal id call
/// [`literal_entry`] (the shared builder) directly.
fn entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    label: &str,
    aliases: &[&str],
) -> CatalogEntry {
    literal_entry(kind, canonical_path, &derived_id(label), aliases)
}

/// The accepted catalog is the durable ABI: a torn write would brick the project, so
/// `write_accepted_catalog` must be all-or-nothing. Overwriting a prior, smaller catalog
/// with a larger one must leave exactly one artifact in the directory — the target file —
/// with no temp staging file beside it and no partial older content exposed.
#[test]
fn write_accepted_catalog_leaves_only_the_target_file() {
    let root = temp_project("catalog-atomic-no-temp", |_| {});

    let prior = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    write_catalog(&root, &prior);

    let next = larger_catalog();
    marrow_check::write_accepted_catalog(&root, &config(), &next).expect("write accepted catalog");

    let entries: Vec<String> = fs::read_dir(&*root)
        .expect("read project root")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into()
        })
        .collect();
    assert_eq!(
        entries,
        vec![String::from("marrow.catalog.json")],
        "a successful write leaves only the target file, with no temp staging artifact"
    );
}

/// The bytes `write_accepted_catalog` lands on disk are the complete catalog, not a
/// truncated prefix: reading the target back parses to the same metadata that was written.
#[test]
fn write_accepted_catalog_lands_the_complete_catalog() {
    let root = temp_project("catalog-atomic-complete", |_| {});

    let written = larger_catalog();
    marrow_check::write_accepted_catalog(&root, &config(), &written)
        .expect("write accepted catalog");

    let bytes = fs::read_to_string(catalog_path(&root)).expect("read catalog");
    let read_back = CatalogMetadata::from_json(&bytes).expect("complete, parseable catalog");
    assert_eq!(read_back, written);
}

fn larger_catalog() -> CatalogMetadata {
    catalog(vec![
        entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
        entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book.title",
            "member-title",
            &[],
        ),
    ])
}

#[test]
fn accepted_catalog_rejects_alias_and_stable_id_collisions() {
    for metadata in [
        catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(
                CatalogEntryKind::Resource,
                "library::Book",
                "res-library",
                &["books::Book"],
            ),
        ]),
        catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "res-book", &[]),
        ]),
    ] {
        let json = metadata.to_json_pretty();
        let error = CatalogMetadata::from_json(&json).expect_err("collision is rejected");

        assert_eq!(error.code, marrow_catalog::CATALOG_INVALID);
    }
}

#[test]
fn accepted_catalog_round_trips_stable_ids_aliases_lifecycle_epoch_and_digest() {
    let metadata = catalog(vec![
        entry(
            CatalogEntryKind::Resource,
            "books::Book",
            "res-book",
            &["library::Book"],
        ),
        CatalogEntry {
            kind: CatalogEntryKind::EnumMember,
            path: "books::Status::archived".to_string(),
            stable_id: derived_id("enum-member-archived"),
            aliases: vec!["books::Status::inactive".to_string()],
            lifecycle: CatalogLifecycle::Deprecated,
            accepted_key_shape: None,
            accepted_struct: None,
        },
    ]);

    let json = metadata.to_json_pretty();
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.epoch, 7);
    assert!(
        parsed.digest.starts_with("sha256:"),
        "catalog digest must be collision-resistant: {}",
        parsed.digest
    );
    assert_eq!("sha256:".len() + 64, parsed.digest.len());
    assert_eq!(parsed.digest, metadata.digest);
    assert_eq!(parsed.entries, metadata.entries);
}

#[test]
fn accepted_catalog_round_trips_reserved_lifecycle() {
    let metadata = catalog(vec![CatalogEntry {
        lifecycle: CatalogLifecycle::Reserved,
        ..entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::oldTitle",
            "member-old-title",
            &[],
        )
    }]);

    let json = metadata.to_json_pretty();
    assert!(json.contains("\"lifecycle\": \"reserved\""), "{json}");
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.entries[0].lifecycle, CatalogLifecycle::Reserved);
}

#[test]
fn non_sha_catalog_digest_is_rejected() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata.to_json_pretty().replacen(
        &format!("\"digest\": \"{}\"", metadata.digest),
        "\"digest\": \"weak:0000000000000000\"",
        1,
    );

    let error = CatalogMetadata::from_json(&json).expect_err("non-SHA digest rejected");

    assert_eq!(error.code, marrow_catalog::CATALOG_INVALID);
}

/// A retired identity is dropped from the catalog, never carried as a `removed` lifecycle
/// marker: `removed` is not a representable [`CatalogLifecycle`], so a catalog whose
/// lifecycle text reads `removed` fails closed with `CATALOG_INVALID`. The contrast against
/// the byte-identical `active` catalog, which parses clean, pins the rejection to the
/// lifecycle value rather than the digest or another field.
#[test]
fn removed_catalog_lifecycle_is_rejected() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata.to_json_pretty();
    CatalogMetadata::from_json(&json).expect("active lifecycle parses clean");

    let field = "\"lifecycle\": \"active\"";
    assert!(
        json.contains(field),
        "on-disk lifecycle spelling drifted: {json}"
    );
    let removed = json.replacen(field, "\"lifecycle\": \"removed\"", 1);

    let error = CatalogMetadata::from_json(&removed).expect_err("removed lifecycle rejected");
    assert_eq!(error.code, marrow_catalog::CATALOG_INVALID);
}

#[test]
fn parallel_catalog_additions_merge_without_regenerating_ids() {
    let branch_a_id = "cat_11111111111111111111111111111111";
    let branch_b_id = "cat_22222222222222222222222222222222";
    let metadata = CatalogMetadata::new(
        9,
        vec![
            literal_entry(
                CatalogEntryKind::Resource,
                "branch_a::Book",
                branch_a_id,
                &[],
            ),
            literal_entry(
                CatalogEntryKind::Resource,
                "branch_b::Magazine",
                branch_b_id,
                &[],
            ),
        ],
    );

    let parsed = CatalogMetadata::from_json(&metadata.to_json_pretty()).expect("catalog parses");

    assert_eq!(parsed.entries[0].stable_id, branch_a_id);
    assert_eq!(parsed.entries[1].stable_id, branch_b_id);
    assert!(parsed.digest.starts_with("sha256:"));
}
