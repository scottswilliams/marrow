//! Apply over a checked index change: a new index rebuilds its entries (unique or not)
//! from the live records, reading defaulted or transformed members at the proposed
//! stable id before catalog acceptance, and fails closed on a unique collision. A
//! dropped or stale index has its derived cells cleared, the dropped id is stamped in
//! the index partition, and completion rejects a stale row with the old key arity.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_check::check_project;
use marrow_run::evolution::{ApplyError, apply, verify_activation_completion};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

use std::fs;

#[test]
fn proposal_index_rebuild_writes_entries_before_catalog_acceptance() {
    let root = temp_project("apply-proposal-index-rebuild", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required isbn: string\n\
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
    let catalog = fs::read_to_string(root.join("marrow.catalog.json")).expect("accepted catalog");
    assert!(
        !catalog.contains("books::^books::byIsbn"),
        "apply must not accept the index before the rebuild transaction"
    );
}

#[test]
fn completion_rejects_stale_index_row_with_old_key_arity() {
    let root = temp_project("completion-index-stale-row", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required isbn: string\n\
         \x20   index byIsbn(isbn, id) unique\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let index_id =
        CatalogId::new(proposal_catalog_id(&program, "books::^books::byIsbn")).expect("index id");
    apply(&witness(&program, &store), &program, &store, false, None).expect("apply");

    store
        .write_index_entry(
            &index_id,
            &[SavedKey::Str("stale".into())],
            &[SavedKey::Int(99)],
            b"stale".to_vec(),
        )
        .expect("write stale old-arity row");
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    let error =
        verify_activation_completion(&program, &store, &commit).expect_err("stale index row fails");

    assert_eq!(error, ApplyError::Drift);
}

#[test]
fn proposal_index_rebuild_reads_defaulted_member_before_catalog_acceptance() {
    let root = temp_project("apply-proposal-index-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
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
    assert_eq!(outcome.receipt.records_backfilled, 2);
    assert_eq!(outcome.receipt.indexes_rebuilt, 1);

    for id in [1, 2] {
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
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
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
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
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
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
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

/// A new unique index over clean data rebuilds its entries and stamps the epoch.
#[test]
fn new_index_rebuild_writes_entries_and_stamps() {
    let root = temp_project("apply-index-rebuild", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
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

/// A new NON-UNIQUE index over existing records rebuilds its entries. The discharge
/// must issue a derived rebuild regardless of uniqueness, so apply writes the index
/// entries rather than stamping success over a silently empty index. A non-unique index
/// ends with the identity keys, so each record publishes one entry under its full key
/// tuple `(genre, id)`.
#[test]
fn new_non_unique_index_rebuild_writes_entries() {
    let root = temp_project("apply-nonunique-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required genre: string\n\
             \x20   index byGenre(genre, id)\n\
             pub fn add(genre: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
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

/// Dropping a source index deletes its derived cells on apply. The schema with the
/// index is committed and base records seed live index cells; current source drops the
/// index, so discharge classifies `IndexDropped` and apply deletes the whole index
/// subtree, leaving no cells under the dropped id. The base records and their members
/// are untouched.
#[test]
fn dropped_index_apply_deletes_index_cells() {
    let root = temp_project("apply-index-drop", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required isbn: string\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check dropping index");

    let w = witness(&program, &store);
    // Dropping an index leaves the accepted entry lingering rather than proposing a new
    // catalog, so apply stamps the accepted epoch while the derived cells are cleared.
    assert!(w.proposal_catalog.is_none());
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");

    assert_eq!(outcome.receipt.catalog_epoch, w.accepted_catalog.epoch);
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
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
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
         resource Book at ^books(id: int)\n\
         \x20   required isbn: string\n\
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
