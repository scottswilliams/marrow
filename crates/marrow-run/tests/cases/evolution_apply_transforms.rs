//! Apply over a checked transform: the body recomputes a member per record from a
//! decodable sibling at the proposed or accepted stable id, writes the computed value,
//! and stamps once. The rebuild reads every store using the resource, composes with a
//! default and a retire in one transaction, and re-previewing is idempotent over
//! unchanged reads.
use crate::evolution_apply_support;
use evolution_apply_support::*;

use marrow_run::evolution::{Approval, apply};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, ScalarType};

#[test]
fn proposal_transform_writes_target_before_catalog_acceptance()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-proposal-transform", |root| {
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
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books")?;
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
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root).expect("checked fixture");
    let cents_id = proposal_catalog_id(&program, "books::Book::priceCents")?;
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let store_id = store_id_of(&accepted_place)?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, ScalarType::Int),
        Some(Scalar::Int(700))
    );

    Ok(())
}

#[test]
fn proposal_transform_writes_an_identity_target_leaf() -> Result<(), Box<dyn std::error::Error>> {
    // A transform whose target member is `Id(^owners)` recomputes an identity per record
    // by copying the accepted identity field of `old`. The encode path is the same
    // `value_to_leaf` the append and direct positional writes use, so the recomputed
    // leaf stores the canonical identity payload, byte-identical to the source.
    let baseline = "module app\n\
         resource Owner\n\
         \x20   name: string\n\
         store ^owners(id: int): Owner\n\
         resource Item\n\
         \x20   required ownerRef: Id(^owners)\n\
         store ^items(id: int): Item\n\
         pub fn add(): Id(^items)\n\
         \x20   return nextId(^items)\n";
    let root = temp_project("apply-proposal-identity-transform", |root| {
        write(root, "src/app.mw", baseline);
    });
    let accepted = commit_then_check(&root).expect("committed fixture");
    let items_place = root_place(&accepted, "items")?;
    let store_id = store_id_of(&items_place)?;
    let owner_ref_accepted = CatalogId::new(
        member_catalog_id(&items_place, "ownerRef").expect("accepted ownerRef catalog id"),
    )
    .expect("ownerRef catalog id");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &items_place,
    };
    // Seed the accepted identity field directly with its canonical payload.
    let seed_owner_ref = |id: i64, key: i64| {
        seed.record(id);
        store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(owner_ref_accepted.clone())],
                encode_identity_payload(&[SavedKey::Int(key)]),
            )
            .expect("seed ownerRef identity leaf");
    };
    seed_owner_ref(1, 7);
    seed_owner_ref(2, 9);

    write(
        &root,
        "src/app.mw",
        "module app\n\
         resource Owner\n\
         \x20   name: string\n\
         store ^owners(id: int): Owner\n\
         resource Item\n\
         \x20   required ownerRef: Id(^owners)\n\
         \x20   ownerMirror: Id(^owners)\n\
         store ^items(id: int): Item\n\
         evolve\n\
         \x20   transform Item.ownerMirror\n\
         \x20       return old.ownerRef\n\
         pub fn add(): Id(^items)\n\
         \x20   return nextId(^items)\n",
    );
    let program = checked(&root).expect("checked fixture");
    let mirror_id = CatalogId::new(proposal_catalog_id(&program, "app::Item::ownerMirror")?)
        .expect("ownerMirror catalog id");
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let read = |id: i64| {
        store
            .read_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(mirror_id.clone())],
            )
            .expect("read ownerMirror")
    };
    assert_eq!(read(1), Some(encode_identity_payload(&[SavedKey::Int(7)])));
    assert_eq!(read(2), Some(encode_identity_payload(&[SavedKey::Int(9)])));
    Ok(())
}

#[test]
fn proposal_transform_updates_every_store_using_the_resource()
-> Result<(), Box<dyn std::error::Error>> {
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
    let accepted = commit_then_check(&root).expect("committed fixture");
    let books_place = root_place(&accepted, "books")?;
    let archives_place = root_place(&accepted, "archives")?;
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
    let program = checked(&root).expect("checked fixture");
    let cents_id = proposal_catalog_id(&program, "books::Book::priceCents")?;
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let books_store_id = store_id_of(&books_place)?;
    let archives_store_id = store_id_of(&archives_place)?;
    assert_eq!(
        read_scalar(&store, &books_store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &archives_store_id, 2, &cents_id, ScalarType::Int),
        Some(Scalar::Int(700))
    );

    Ok(())
}

#[test]
fn transform_if_const_over_sparse_old_member_applies() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-if-const-old", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books")?;
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("one".into()));
    seed.member(1, "subtitle", Scalar::Str("present".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("two".into()));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   required summary: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.summary\n\
         \x20       if const subtitle = old.subtitle\n\
         \x20           return subtitle\n\
         \x20       return old.title\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root).expect("checked fixture");
    let summary_id = proposal_catalog_id(&program, "books::Book::summary")?;
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let store_id = store_id_of(&accepted_place)?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &summary_id, ScalarType::Str),
        Some(Scalar::Str("present".into()))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &summary_id, ScalarType::Str),
        Some(Scalar::Str("two".into()))
    );

    Ok(())
}

#[test]
fn transform_exists_over_sparse_old_member_applies() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-exists-old", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books")?;
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("one".into()));
    seed.member(1, "subtitle", Scalar::Str("present".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("two".into()));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   required summary: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.summary\n\
         \x20       if exists(old.subtitle)\n\
         \x20           return old.subtitle\n\
         \x20       return old.title\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root).expect("checked fixture");
    let summary_id = proposal_catalog_id(&program, "books::Book::summary")?;
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 2);

    let store_id = store_id_of(&accepted_place)?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &summary_id, ScalarType::Str),
        Some(Scalar::Str("present".into()))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &summary_id, ScalarType::Str),
        Some(Scalar::Str("two".into()))
    );

    Ok(())
}

/// A checked transform computes a new member from a sibling and apply writes the
/// computed value per record, then stamps the epoch. This is the activatable->applyable
/// invariant: a witness whose read members all decode under their current type and whose
/// body does not fault over the data applies successfully. Each record's `priceCents`
/// becomes `price * 100`, derived from its own decodable `price`, and re-previewing
/// after the apply yields the same value (idempotent over unchanged reads).
#[test]
fn transform_computes_new_member_per_record_and_stamps() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-compute", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
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

    let store_id = store_id_of(&place)?;
    let cents_id = member_catalog_id(&place, "priceCents")?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, ScalarType::Int),
        Some(Scalar::Int(700))
    );
    assert!(
        store.read_commit_metadata().expect("read").is_some(),
        "the transform apply stamps the store"
    );

    // The apply advanced the catalog and recorded the transform on its target. Re-checking
    // against the now-advanced store catalog, the transform reads as consumed: re-applying it
    // discharges nothing rather than rewriting the same values forever.
    let advanced = store
        .read_catalog_snapshot()
        .expect("read advanced catalog snapshot");
    let (report, resumed_program) =
        marrow_check::check_project_with_catalog(&root, &config(), advanced.as_ref())
            .expect("re-check against advanced catalog");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let resumed = witness(&resumed_program, &store);
    assert_eq!(
        resumed.counts.records_to_transform, 0,
        "a consumed transform leaves no record work: {resumed:#?}"
    );
    let outcome =
        apply(&resumed, &resumed_program, &store, false, None).expect("re-apply succeeds");
    assert_eq!(
        outcome.receipt.records_transformed, 0,
        "re-applying a consumed transform is a no-op"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(300))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, ScalarType::Int),
        Some(Scalar::Int(700))
    );

    Ok(())
}

/// A transform onto a member first MINTED in the same proposal settles in a single
/// apply. The new required field is added and transformed together: one apply writes
/// the computed cell and advances the epoch, and re-checking against the advanced
/// catalog must read the transform as consumed (no record work, a no-op re-apply at a
/// stable epoch). A new member's structural addition and its transform discharge mark
/// must land in one activation, not split across two epochs.
#[test]
fn transform_onto_newly_minted_member_settles_in_one_apply()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-new-member-one-apply", |root| {
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
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books")?;
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
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root).expect("checked fixture");
    let cents_id = proposal_catalog_id(&program, "books::Book::priceCents")?;
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 1);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_transformed, 1);
    let epoch_after_first = outcome.receipt.catalog_epoch;

    let store_id = store_id_of(&accepted_place)?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(300))
    );

    // Re-check against the advanced catalog with the block kept. The transform must read
    // as consumed: no record work, a no-op re-apply, and a stable epoch.
    let advanced = store
        .read_catalog_snapshot()
        .expect("read advanced catalog snapshot");
    let (report, resumed_program) =
        marrow_check::check_project_with_catalog(&root, &config(), advanced.as_ref())
            .expect("re-check against advanced catalog");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let resumed = witness(&resumed_program, &store);
    assert_eq!(
        resumed.counts.records_to_transform, 0,
        "a consumed transform onto a new member leaves no record work: {resumed:#?}"
    );
    let outcome =
        apply(&resumed, &resumed_program, &store, false, None).expect("re-apply succeeds");
    assert_eq!(
        outcome.receipt.records_transformed, 0,
        "re-applying a consumed new-member transform is a no-op"
    );
    assert_eq!(
        outcome.receipt.catalog_epoch, epoch_after_first,
        "a settled transform does not advance the epoch on re-apply"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(300))
    );

    Ok(())
}

/// A shape-neutral in-place transform of an already-accepted member, over a store
/// already stamped at the accepted epoch under the accepted source digest, must still
/// rewrite the data. The transform proposes no new catalog entry, so its target epoch
/// equals the accepted epoch the store already carries, and its source digest equals the
/// stamped one; only the transform witness distinguishes the activation from a settled
/// store. Apply must discharge the transform — preview promises `records_to_transform`,
/// so apply must report the same count and overwrite the target cell — not treat the
/// matching stamp as a finished activation and silently no-op the checked migration.
#[test]
fn in_place_transform_over_stamped_store_rewrites_data() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-in-place-transform-stamped", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required code: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.code\n\
             \x20       return std::text::length(old.title)\n\
             pub fn add(title: string, code: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the full schema with the evolve block, so `code` is an accepted member and
    // the accepted source digest is the transform-variant digest. The transform recomputes
    // an existing leaf in place: no new catalog entry, so the witness carries no proposal.
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("encyclopedia".into()));
    seed.member(1, "code", Scalar::Int(5));

    // Stamp the store at the accepted epoch under the accepted source digest, the steady
    // state a prior `marrow run` leaves behind. The transform's target epoch and source
    // digest now match the stamp exactly, so a target-stamp short-circuit would skip it.
    stamp_clean_commit(&store, &program);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_transform, 1);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(
        outcome.receipt.records_transformed, 1,
        "apply discharges the transform the preview promised"
    );

    let store_id = store_id_of(&place)?;
    let code_id = member_catalog_id(&place, "code")?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &code_id, ScalarType::Int),
        Some(Scalar::Int(12)),
        "the transform overwrites the existing leaf with the recomputed value"
    );

    Ok(())
}

/// A transform composes with a default and a retire in one evolve block: apply
/// computes the transform target, backfills the defaulted member, drops the retired
/// member, and stamps once. The transform reads a sibling the retire does not touch.
#[test]
fn transform_composes_with_default_and_retire() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-compose", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             \x20   required currency: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             pub fn add(price: int, currency: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the full schema, so every member binds a stable id and current source
    // proposes no new catalog entry: the default backfills records missing `currency`,
    // the retire drops the committed `subtitle` source no longer declares, and the
    // transform recomputes the committed `priceCents`.
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;

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
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         \x20   required currency: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   default Book.currency = \"USD\"\n\
         \x20   retire Book.subtitle\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn add(price: int, currency: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;

    // The retire makes the witness non-activatable on its own; the transform and the
    // default are activatable, and the retire applies under maintenance with a scoped
    // approval. Apply composes all three in one stamped transaction.
    let w = witness(&program, &store);
    let approval = Approval {
        retires: vec![(CatalogId::new(subtitle_id.clone())?, 1)],
    };
    let outcome = apply(&w, &program, &store, true, Some(&approval)).expect("apply");
    assert_eq!(outcome.receipt.records_transformed, 1);
    assert_eq!(outcome.receipt.records_backfilled, 1);
    assert_eq!(outcome.receipt.records_retired, 1);

    let store_id = store_id_of(&place)?;
    let cents_id = member_catalog_id(&place, "priceCents")?;
    let currency_id = member_catalog_id(&place, "currency")?;
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, ScalarType::Int),
        Some(Scalar::Int(500)),
        "the transform target is recomputed"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &currency_id, ScalarType::Str),
        Some(Scalar::Str("USD".into())),
        "the defaulted member is backfilled"
    );
    assert!(
        !store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(CatalogId::new(subtitle_id)?)]
            )
            .expect("exists"),
        "the retired member is dropped"
    );

    Ok(())
}
