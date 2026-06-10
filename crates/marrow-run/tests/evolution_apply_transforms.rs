//! Apply over a checked transform: the body recomputes a member per record from a
//! decodable sibling at the proposed or accepted stable id, writes the computed value,
//! and stamps once. The rebuild reads every store using the resource, composes with a
//! default and a retire in one transaction, and re-previewing is idempotent over
//! unchanged reads. Completion rejects a missing transform cell.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_run::evolution::{ApplyError, Approval, apply, verify_activation_completion};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::Scalar;

use std::fs;

#[test]
fn proposal_transform_writes_target_before_catalog_acceptance() {
    let root = temp_project("apply-proposal-transform", |root| {
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
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let cents_id = proposal_catalog_id(&program, "books::Book::priceCents");
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let store_id = store_id_of(&accepted_place);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, INT),
        Some(Scalar::Int(700))
    );
    let catalog = fs::read_to_string(root.join("marrow.catalog.json")).expect("accepted catalog");
    assert!(
        !catalog.contains("books::Book::priceCents"),
        "apply must not accept the transform target before the data transaction"
    );
}

#[test]
fn completion_rejects_missing_transform_cell() {
    let root = temp_project("completion-transform-missing-cell", |root| {
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
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let cents_id = CatalogId::new(proposal_catalog_id(&program, "books::Book::priceCents"))
        .expect("priceCents id");
    apply(&witness(&program, &store), &program, &store, false, None).expect("apply");

    let store_id = store_id_of(&accepted_place);
    store
        .delete_data_subtree(
            &store_id,
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(cents_id)],
        )
        .expect("delete transformed cell");
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    let error = verify_activation_completion(&program, &store, &commit)
        .expect_err("missing transform cell fails");

    assert_eq!(error, ApplyError::Drift);
}

#[test]
fn proposal_transform_updates_every_store_using_the_resource() {
    let root = temp_project("apply-proposal-transform-multi-store", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             store ^books(id: int): Book\n\
             store ^archives(id: int): Book\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let books_place = root_place(&accepted, "books");
    let archives_place = root_place(&accepted, "archives");
    let store = TreeStore::memory();
    let books_seed = Seed {
        store: &store,
        place: &books_place,
    };
    books_seed.record(1);
    books_seed.member(1, "price", Scalar::Int(3));
    let archives_seed = Seed {
        store: &store,
        place: &archives_place,
    };
    archives_seed.record(2);
    archives_seed.member(2, "price", Scalar::Int(7));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         store ^archives(id: int): Book\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let cents_id = proposal_catalog_id(&program, "books::Book::priceCents");
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let books_store_id = store_id_of(&books_place);
    let archives_store_id = store_id_of(&archives_place);
    assert_eq!(
        read_scalar(&store, &books_store_id, 1, &cents_id, INT),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &archives_store_id, 2, &cents_id, INT),
        Some(Scalar::Int(700))
    );
}

/// A checked transform computes a new member from a sibling and apply writes the
/// computed value per record, then stamps the epoch. Each record's `priceCents`
/// becomes `price * 100`, derived from its own decodable `price`, and re-previewing
/// after the apply yields the same value (idempotent over unchanged reads).
#[test]
fn transform_computes_new_member_per_record_and_stamps() {
    let root = temp_project("apply-transform-compute", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
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
    // Two records carry distinct prices and a stale priceCents the transform recomputes.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(3));
    seed.member(1, "priceCents", Scalar::Int(0));
    seed.record(2);
    seed.member(2, "price", Scalar::Int(7));
    seed.member(2, "priceCents", Scalar::Int(0));

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, INT),
        Some(Scalar::Int(700))
    );
    assert!(
        store.read_commit_metadata().expect("read").is_some(),
        "the transform apply stamps the store"
    );

    // Idempotent: re-previewing against the now-transformed store and re-applying
    // recomputes the same value from the unchanged `price` reads.
    let resumed = witness(&program, &store);
    apply(&resumed, &program, &store, false, None).expect("re-apply succeeds");
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, INT),
        Some(Scalar::Int(700))
    );
}

/// The activatable->applyable invariant for a transform: a witness whose read members
/// all decode under their current type and whose body does not fault over the data
/// applies successfully, writing the recomputed value and stamping the store.
#[test]
fn activatable_transform_with_total_body_applies() {
    let root = temp_project("apply-transform-total", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
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
    seed.member(1, "price", Scalar::Int(5));
    seed.member(1, "priceCents", Scalar::Int(0));

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 1);

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(500))
    );
    assert!(store.read_commit_metadata().expect("read").is_some());
}

/// A transform composes with a default and a retire in one evolve block: apply
/// computes the transform target, backfills the defaulted member, drops the retired
/// member, and stamps once. The transform reads a sibling the retire does not touch.
#[test]
fn transform_composes_with_default_and_retire() {
    let root = temp_project("apply-transform-compose", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             \x20   required currency: string\n\
             \x20   subtitle: string\n\
             pub fn add(price: int, currency: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the full schema, so every member binds a stable id and current source
    // proposes no new catalog entry: the default backfills records missing `currency`,
    // the retire drops the committed `subtitle` source no longer declares, and the
    // transform recomputes the committed `priceCents`.
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    // The old record predates `currency`, so the default must backfill it.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(5));
    seed.member(1, "priceCents", Scalar::Int(0));
    seed.member_by_id(1, &subtitle_id, Scalar::Str("sub".into()));

    // New source: transform priceCents from price, default currency, retire subtitle.
    // The transform reads `price`, untouched by the other two intents.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         \x20   required currency: string\n\
         evolve\n\
         \x20   default Book.currency = \"USD\"\n\
         \x20   retire Book.subtitle\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int, currency: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let place = root_place(&program, "books");

    // The retire makes the witness non-activatable on its own; the transform and the
    // default are activatable, and the retire applies under maintenance with a scoped
    // approval. Apply composes all three in one stamped transaction.
    let w = witness(&program, &store);
    let approval = Approval {
        retires: vec![(CatalogId::new(subtitle_id.clone()).unwrap(), 1)],
    };
    let outcome = apply(&w, &program, &store, true, Some(&approval)).expect("apply");
    assert_eq!(outcome.receipt.records_transformed, 1);
    assert_eq!(outcome.receipt.records_backfilled, 1);
    assert_eq!(outcome.receipt.records_retired, 1);

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    let currency_id = member_catalog_id(&place, "currency");
    let str_ty = marrow_store::value::ScalarType::Str;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(500)),
        "the transform target is recomputed"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &currency_id, str_ty),
        Some(Scalar::Str("USD".into())),
        "the defaulted member is backfilled"
    );
    assert!(
        !store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new(subtitle_id).unwrap()
                )]
            )
            .expect("exists"),
        "the retired member is dropped"
    );
}
