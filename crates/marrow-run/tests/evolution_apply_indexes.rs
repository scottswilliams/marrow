//! Apply over a checked index change: a new index rebuilds its entries (unique or not)
//! from the live records, reading defaulted or transformed members at the proposed
//! stable id before catalog acceptance, and fails closed on a unique collision. A
//! dropped or stale index has its derived cells cleared, the dropped id is stamped in
//! the index partition, and completion rejects a stale row with the old key arity.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_catalog::{CatalogEntryKind, CatalogMetadata};
use marrow_check::evolution::{RepairReason, Verdict};
use marrow_run::evolution::{ApplyError, apply, current_engine_profile};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_index_key, encode_identity_payload};
use marrow_store::tree::{CommitMetadata, DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

fn checked_with_missing_index_shape(
    root: &std::path::Path,
    index_path: &str,
) -> marrow_check::CheckedProgram {
    let catalog_path = root.join("marrow.catalog.json");
    let mut catalog = CatalogMetadata::from_json(
        &std::fs::read_to_string(&catalog_path).expect("read accepted catalog"),
    )
    .expect("accepted catalog parses");
    let entry = catalog
        .entries
        .iter_mut()
        .find(|entry| entry.kind == CatalogEntryKind::StoreIndex && entry.path == index_path)
        .expect("store index entry");
    entry.accepted_index_shape = None;
    let catalog = CatalogMetadata::new(catalog.epoch, catalog.entries);
    std::fs::write(&catalog_path, catalog.to_json_pretty()).expect("write accepted catalog");
    checked(root)
}

fn stamp_clean_commit(store: &TreeStore, program: &marrow_check::CheckedProgram) {
    let profile = current_engine_profile();
    store
        .write_commit_metadata(&CommitMetadata {
            commit_id: 1,
            catalog_epoch: program.catalog.accepted_epoch.expect("accepted epoch"),
            layout_epoch: profile.layout_epoch(),
            source_digest: program.source_digest(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
        })
        .expect("stamp clean commit");
}

#[test]
fn proposal_index_rebuild_writes_entries_before_catalog_acceptance() {
    let root = temp_project("apply-proposal-index-rebuild", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required isbn: string\n\
         store ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let index_id = CatalogId::new(proposal_catalog_id(&program, "books::^books::byIsbn")).unwrap();
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    for (isbn, id) in [("111", 1), ("222", 2)] {
        let scan = store
            .scan_index_tuple(&index_id, &[SavedKey::Str(isbn.into())], 2)
            .expect("scan rebuilt index");
        assert_eq!(scan.entries.len(), 1, "index entry for {isbn}");
        assert_eq!(scan.entries[0].identity, vec![SavedKey::Int(id)]);
    }
}

#[test]
fn proposal_index_rebuild_writes_identity_field_components() {
    let root = temp_project("apply-proposal-index-identity-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   authorId: Id(^authors)\n\
             store ^books(id: int): Book\n",
        );
    });
    let accepted = commit_then_check(&root);
    let authors = root_place(&accepted, "authors");
    let books = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let author_seed = Seed {
        store: &store,
        place: &authors,
    };
    author_seed.record(1);
    author_seed.member(1, "name", Scalar::Str("Ann".into()));
    author_seed.record(2);
    author_seed.member(2, "name", Scalar::Str("Bob".into()));

    let book_seed = Seed {
        store: &store,
        place: &books,
    };
    let author_member = CatalogId::new(member_catalog_id(&books, "authorId")).unwrap();
    for (book, title, author) in [(1, "A", 1), (2, "B", 2), (3, "C", 1)] {
        book_seed.record(book);
        book_seed.member(book, "title", Scalar::Str(title.into()));
        store
            .write_data_value(
                &book_seed.store_id(),
                &[SavedKey::Int(book)],
                &[DataPathSegment::Member(author_member.clone())],
                encode_identity_payload(&[SavedKey::Int(author)]),
            )
            .expect("seed identity field");
    }

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \x20   index byAuthor(authorId, id)\n",
    );
    let program = checked(&root);
    let index_id =
        CatalogId::new(proposal_catalog_id(&program, "books::^books::byAuthor")).unwrap();
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    let authors_id = store_id_of(&authors);
    let ann_key = SavedKey::Bytes(encode_identity_index_key(
        authors_id.as_str(),
        &[SavedKey::Int(1)],
    ));
    for id in [1, 3] {
        let scan = store
            .scan_index_tuple(&index_id, &[ann_key.clone(), SavedKey::Int(id)], 2)
            .expect("scan author index");
        assert_eq!(scan.entries.len(), 1, "index entry for book {id}");
        assert_eq!(scan.entries[0].identity, vec![SavedKey::Int(id)]);
    }
}

#[test]
fn unique_index_over_identity_field_collision_fails_closed() {
    let root = temp_project("apply-proposal-index-identity-field-unique", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   authorId: Id(^authors)\n\
             store ^books(id: int): Book\n",
        );
    });
    let accepted = commit_then_check(&root);
    let authors = root_place(&accepted, "authors");
    let books = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let author_seed = Seed {
        store: &store,
        place: &authors,
    };
    author_seed.record(1);
    author_seed.member(1, "name", Scalar::Str("Ann".into()));

    let book_seed = Seed {
        store: &store,
        place: &books,
    };
    let author_member = CatalogId::new(member_catalog_id(&books, "authorId")).unwrap();
    for (book, title) in [(1, "A"), (2, "B")] {
        book_seed.record(book);
        book_seed.member(book, "title", Scalar::Str(title.into()));
        store
            .write_data_value(
                &book_seed.store_id(),
                &[SavedKey::Int(book)],
                &[DataPathSegment::Member(author_member.clone())],
                encode_identity_payload(&[SavedKey::Int(1)]),
            )
            .expect("seed identity field");
    }

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \x20   index oneBookByAuthor(authorId) unique\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);

    assert!(!w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.index_collisions, 1, "{w:#?}");
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NotActivatable);
}

#[test]
fn unique_index_over_malformed_identity_field_fails_closed() {
    let root = temp_project("apply-proposal-index-identity-field-malformed", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   authorId: Id(^authors)\n\
             store ^books(id: int): Book\n",
        );
    });
    let accepted = commit_then_check(&root);
    let books = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let book_seed = Seed {
        store: &store,
        place: &books,
    };
    let author_member = CatalogId::new(member_catalog_id(&books, "authorId")).unwrap();
    book_seed.record(1);
    book_seed.member(1, "title", Scalar::Str("A".into()));
    store
        .write_data_value(
            &book_seed.store_id(),
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(author_member)],
            encode_identity_payload(&[SavedKey::Str("not-an-author-int".into())]),
        )
        .expect("seed malformed identity field");

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \x20   index oneBookByAuthor(authorId) unique\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);

    assert!(!w.is_activatable(), "{w:#?}");
    assert!(
        w.verdicts.iter().any(|verdict| matches!(
            verdict.verdict,
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue
            }
        )),
        "{w:#?}"
    );
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NotActivatable);
}

#[test]
fn proposal_index_rebuild_reads_defaulted_member_before_catalog_acceptance() {
    let record_count = 1_024usize;
    let root = temp_project("apply-proposal-index-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    for id in 1..=record_count {
        let record_id = id as i64;
        seed.record(record_id);
        seed.member(record_id, "title", Scalar::Str(format!("Book {id}")));
    }

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         \x20   index byPages(pages, id)\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let index_id = CatalogId::new(proposal_catalog_id(&program, "books::^books::byPages")).unwrap();
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_backfilled, record_count);
    assert_eq!(outcome.receipt.default_records_by_id.len(), 1);
    assert_eq!(
        outcome.receipt.default_records_by_id[0].records_backfilled,
        record_count as u64
    );
    assert_eq!(
        outcome.receipt.default_records_by_id[0].target_records,
        record_count as u64
    );
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    for id in [1, 512, 1_024] {
        let scan = store
            .scan_index_tuple(&index_id, &[SavedKey::Int(0), SavedKey::Int(id)], 2)
            .expect("scan rebuilt default index");
        assert_eq!(scan.entries.len(), 1, "index entry for {id}");
        assert_eq!(scan.entries[0].identity, vec![SavedKey::Int(id)]);
    }
}

#[test]
fn unique_index_over_defaulted_member_collision_fails_closed() {
    let root = temp_project("apply-proposal-index-default-unique", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         \x20   index byPages(pages) unique\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);
    assert!(!w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.index_collisions, 1, "{w:#?}");
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NotActivatable);
}

#[test]
fn proposal_index_rebuild_reads_transform_target_before_catalog_acceptance() {
    let root = temp_project("apply-proposal-index-transform", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             store ^books(id: int): Book\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "price", Scalar::Int(3));
    seed.record(2);
    seed.member(2, "price", Scalar::Int(7));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         \x20   index byCents(priceCents, id)\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let index_id = CatalogId::new(proposal_catalog_id(&program, "books::^books::byCents")).unwrap();
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    for (cents, id) in [(300, 1), (700, 2)] {
        let scan = store
            .scan_index_tuple(&index_id, &[SavedKey::Int(cents), SavedKey::Int(id)], 2)
            .expect("scan rebuilt transform index");
        assert_eq!(scan.entries.len(), 1, "index entry for {cents}");
        assert_eq!(scan.entries[0].identity, vec![SavedKey::Int(id)]);
    }
}

#[test]
fn unique_index_over_transform_target_fails_closed() {
    let root = temp_project("apply-proposal-index-transform-unique", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             store ^books(id: int): Book\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "price", Scalar::Int(3));
    seed.record(2);
    seed.member(2, "price", Scalar::Int(7));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         \x20   index byCents(priceCents) unique\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);
    assert!(!w.is_activatable(), "{w:#?}");
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NotActivatable);
}

/// A unique index accepted before index-shape signatures rebuilds its entries once and stamps the
/// advanced catalog that freezes the current signature.
#[test]
fn legacy_unique_index_shape_rebuild_writes_entries_and_stamps() {
    let root = temp_project("apply-index-rebuild", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    commit_then_check(&root);
    let program = checked_with_missing_index_shape(&root, "books::^books::byIsbn");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // Records exist with distinct member values but no index cells were written.
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));

    let witness = witness(&program, &store);
    let index_id = CatalogId::new(index_catalog_id(&place, "byIsbn")).unwrap();
    store
        .write_index_entry(
            &index_id,
            &[SavedKey::Str("stale".into())],
            &[SavedKey::Int(99)],
            Vec::new(),
        )
        .expect("seed stale index entry");

    let outcome = apply(&witness, &program, &store, false, None).expect("apply");
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    let one = store
        .scan_index_tuple(&index_id, &[SavedKey::Str("111".into())], 2)
        .expect("scan");
    assert_eq!(one.entries.len(), 1, "the rebuilt index holds 111");
    assert_eq!(one.entries[0].identity, vec![SavedKey::Int(1)]);
    let two = store
        .scan_index_tuple(&index_id, &[SavedKey::Str("222".into())], 2)
        .expect("scan");
    assert_eq!(two.entries.len(), 1, "the rebuilt index holds 222");
    let stale = store
        .scan_index_tuple(&index_id, &[SavedKey::Str("stale".into())], 2)
        .expect("scan stale");
    assert!(
        stale.entries.is_empty(),
        "a rebuild must delete stale index entries"
    );
}

/// A non-unique index accepted before index-shape signatures rebuilds its entries. The discharge
/// must issue a derived rebuild for the affected index, so apply writes the index entries rather
/// than stamping success over a silently empty index.
#[test]
fn legacy_non_unique_index_shape_rebuild_writes_entries() {
    let root = temp_project("apply-nonunique-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required genre: string\n\
             store ^books(id: int): Book\n\
             \x20   index byGenre(genre, id)\n\
             pub fn add(genre: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    commit_then_check(&root);
    let program = checked_with_missing_index_shape(&root, "books::^books::byGenre");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "genre", Scalar::Str("scifi".into()));
    seed.record(2);
    seed.member(2, "genre", Scalar::Str("scifi".into()));

    let w = witness(&program, &store);
    let outcome = apply(&w, &program, &store, false, None).expect("apply");
    assert_eq!(
        outcome.receipt.indexes_rebuilt, 1,
        "the non-unique index rebuilds"
    );

    // A non-unique index ends with the identity keys, so each record publishes exactly
    // one entry under its full `(genre, id)` tuple.
    let index_id = CatalogId::new(index_catalog_id(&place, "byGenre")).unwrap();
    for id in [1, 2] {
        let scan = store
            .scan_index_tuple(
                &index_id,
                &[SavedKey::Str("scifi".into()), SavedKey::Int(id)],
                8,
            )
            .expect("scan");
        let identities: Vec<_> = scan
            .entries
            .iter()
            .map(|entry| entry.identity.clone())
            .collect();
        assert_eq!(
            identities,
            vec![vec![SavedKey::Int(id)]],
            "a new non-unique index must hold record {id}, not be silently empty"
        );
    }
}

#[test]
fn same_name_index_key_change_rebuilds_under_existing_catalog_id() {
    let root = temp_project("apply-index-same-name-key-change", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byLookup(title, id)\n\
             pub fn add(title: string, shelf: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = CatalogId::new(index_catalog_id(&accepted_place, "byLookup")).unwrap();

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    for (id, title) in [(1, "Dune"), (2, "Hyperion")] {
        seed.record(id);
        seed.member(id, "title", Scalar::Str(title.into()));
        seed.member(id, "shelf", Scalar::Str("fiction".into()));
        store
            .write_index_entry(
                &index_id,
                &[SavedKey::Str(title.into()), SavedKey::Int(id)],
                &[SavedKey::Int(id)],
                Vec::new(),
            )
            .expect("seed old index entry");
    }
    stamp_clean_commit(&store, &accepted);

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required shelf: string\n\
         store ^books(id: int): Book\n\
         \x20   index byLookup(shelf, id)\n\
         pub fn add(title: string, shelf: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");

    let outcome = apply(&w, &program, &store, false, None).expect("apply");
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    for id in [1, 2] {
        let scan = store
            .scan_index_tuple(
                &index_id,
                &[SavedKey::Str("fiction".into()), SavedKey::Int(id)],
                8,
            )
            .expect("scan new key");
        assert_eq!(
            scan.entries
                .iter()
                .map(|entry| entry.identity.clone())
                .collect::<Vec<_>>(),
            vec![vec![SavedKey::Int(id)]],
            "same-name index must be rebuilt for record {id}"
        );
    }
    let stale = store
        .scan_index_tuple(
            &index_id,
            &[SavedKey::Str("Dune".into()), SavedKey::Int(1)],
            8,
        )
        .expect("scan old key");
    assert!(
        stale.entries.is_empty(),
        "old key-shape entries must be removed by the rebuild"
    );
}

#[test]
fn same_name_unique_index_key_change_fails_closed_on_collisions() {
    let root = temp_project("apply-index-same-name-unique-key-change", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byLookup(title) unique\n\
             pub fn add(title: string, shelf: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = CatalogId::new(index_catalog_id(&accepted_place, "byLookup")).unwrap();

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    for (id, title) in [(1, "Dune"), (2, "Hyperion")] {
        seed.record(id);
        seed.member(id, "title", Scalar::Str(title.into()));
        seed.member(id, "shelf", Scalar::Str("fiction".into()));
        store
            .write_index_entry(
                &index_id,
                &[SavedKey::Str(title.into())],
                &[SavedKey::Int(id)],
                Vec::new(),
            )
            .expect("seed old unique index entry");
    }
    stamp_clean_commit(&store, &accepted);

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required shelf: string\n\
         store ^books(id: int): Book\n\
         \x20   index byLookup(shelf) unique\n\
         pub fn add(title: string, shelf: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);

    assert!(!w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.index_collisions, 1, "{w:#?}");
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NotActivatable);
}

#[test]
fn same_name_index_uniqueness_change_fails_closed_on_collisions() {
    let root = temp_project("apply-index-same-name-uniqueness-change", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byLookup(shelf, id)\n\
             pub fn add(title: string, shelf: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = CatalogId::new(index_catalog_id(&accepted_place, "byLookup")).unwrap();

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    for (id, title) in [(1, "Dune"), (2, "Hyperion")] {
        seed.record(id);
        seed.member(id, "title", Scalar::Str(title.into()));
        seed.member(id, "shelf", Scalar::Str("fiction".into()));
        store
            .write_index_entry(
                &index_id,
                &[SavedKey::Str("fiction".into()), SavedKey::Int(id)],
                &[SavedKey::Int(id)],
                Vec::new(),
            )
            .expect("seed old non-unique index entry");
    }
    stamp_clean_commit(&store, &accepted);

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required shelf: string\n\
         store ^books(id: int): Book\n\
         \x20   index byLookup(shelf) unique\n\
         pub fn add(title: string, shelf: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);

    assert!(!w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.index_collisions, 1, "{w:#?}");
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NotActivatable);
}

/// Dropping a source index discharges: apply deletes its derived cells, advances the
/// catalog epoch, and drops the index entry from the published catalog. The schema with
/// the index is committed and base records seed live index cells; current source drops the
/// index, so discharge classifies `IndexDropped` and apply clears the whole index subtree.
/// The base records and their members are untouched, and re-previewing finds nothing left.
#[test]
fn dropped_index_apply_deletes_index_cells() {
    let root = temp_project("apply-index-drop", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the schema that declares the index, so the index binds a stable id.
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = CatalogId::new(index_catalog_id(&accepted_place, "byIsbn")).unwrap();
    let store_id = store_id_of(&accepted_place);

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));
    // Live index cells the dropped index would otherwise leak.
    for (key, id) in [("111", 1), ("222", 2)] {
        store
            .write_index_entry(
                &index_id,
                &[SavedKey::Str(key.into())],
                &[SavedKey::Int(id)],
                Vec::new(),
            )
            .expect("seed index entry");
    }
    assert!(
        index_has_children(&store, &index_id),
        "the index starts with cells"
    );

    // Drop the index from source while keeping the member it covered.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required isbn: string\n\
         store ^books(id: int): Book\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);

    let w = witness(&program, &store);
    // Dropping an index discharges as its own evolution: it builds a proposal that advances
    // the epoch and removes the index entry, so the catalog no longer carries a derived index
    // with no cells behind it.
    let proposal_epoch = w
        .proposal_catalog
        .as_ref()
        .expect("a dropped index builds a proposal")
        .epoch;
    assert_eq!(proposal_epoch, w.accepted_catalog.epoch + 1);
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");

    assert_eq!(outcome.receipt.catalog_epoch, proposal_epoch);
    assert!(
        !index_has_children(&store, &index_id),
        "the dropped index must have no remaining cells"
    );
    for (id, isbn) in [(1, "111"), (2, "222")] {
        let bytes = store
            .read_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(
                    CatalogId::new(member_catalog_id(&accepted_place, "isbn")).unwrap(),
                )],
            )
            .expect("read isbn")
            .expect("isbn present");
        assert_eq!(bytes, encode_value(&Scalar::Str(isbn.into())).unwrap());
    }

    // The published catalog dropped the index entry and advanced its epoch, so re-checking
    // and re-previewing against the committed snapshot finds no further index drop.
    let accepted = store
        .read_catalog_snapshot()
        .expect("read snapshot")
        .expect("a snapshot");
    assert_eq!(accepted.epoch, proposal_epoch);
    assert!(
        !accepted
            .entries
            .iter()
            .any(|entry| entry.stable_id == index_id.as_str()),
        "the dropped index entry is gone from the committed catalog: {:#?}",
        accepted.entries
    );
}

/// Dropping a unique index from source stamps its catalog id in the commit metadata's
/// index partition, never among the data roots. The discharge already knows the id is a
/// store index by its catalog entry kind, so apply must not re-derive the index set
/// from current source (which no longer declares the index).
#[test]
fn dropped_index_id_stamped_as_index_not_root() {
    let root = temp_project("apply-drop-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the schema that declares the index, so the index binds a stable id.
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = index_catalog_id(&accepted_place, "byIsbn");

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));

    // Drop the index from source while keeping the member; the accepted catalog still
    // names it, so discharge classifies an index drop. Apply stays activatable and stamps.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required isbn: string\n\
         store ^books(id: int): Book\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    apply(&w, &program, &store, false, None).expect("apply succeeds");

    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("a stamp");
    assert!(
        commit
            .changed_index_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id must be stamped as an index: {commit:#?}"
    );
    assert!(
        !commit
            .changed_root_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id must not be stamped as a data root: {commit:#?}"
    );
}
