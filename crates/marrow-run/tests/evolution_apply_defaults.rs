//! Apply over a checked default backfill: the proposal-or-accepted member is located
//! from the witness, every record lacking it is backfilled under the bound stable id,
//! and the proposal epoch is stamped. Completion re-verification fails closed on a
//! forged receipt, a missing backfilled cell, engine-profile drift, or an erased
//! commit source digest, and a backfill never overwrites preexisting target data.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_run::evolution::{
    ApplyError, apply, current_engine_profile, verify_activation_completion,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, EngineProfile, TreeStore};
use marrow_store::value::{Scalar, encode_value};

/// A required member added to source is proposal-only until the activation commits.
/// Apply must still locate that proposal member from the exact witness, backfill old
/// records under the proposed stable id, then stamp the proposal epoch. The member binds
/// only through the proposal, never the accepted snapshot, so backfilling against a member
/// the accepted catalog does not yet carry is exactly the soundness path under test.
#[test]
fn proposal_required_default_backfills_before_catalog_acceptance() {
    let root = temp_project("apply-proposal-required-default", |root| {
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
         evolve\n\
         \x20   default Book.pages = 0\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let proposal_epoch = program.catalog.proposal.as_ref().expect("proposal").epoch;
    let pages_id = proposal_catalog_id(&program, "books::Book::pages");
    assert!(
        accepted_place
            .root_members
            .iter()
            .all(|member| member.name != "pages"),
        "the accepted runtime place must not know the new member"
    );

    let w = witness(&program, &store);
    assert_eq!(
        w.proposal_catalog.as_ref().map(|c| c.epoch),
        Some(proposal_epoch)
    );
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_backfill, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.catalog_epoch, proposal_epoch);
    assert_eq!(outcome.receipt.store_commit_id_before, w.store_commit_id);
    assert_eq!(outcome.receipt.source_digest, w.source_digest);
    assert_eq!(outcome.receipt.evolution_digest, w.evolution_digest);
    assert_eq!(
        outcome.receipt.accepted_catalog_digest,
        w.accepted_catalog.digest
    );
    assert_eq!(
        outcome.receipt.proposal_catalog_digest,
        w.proposal_catalog
            .as_ref()
            .map(|catalog| catalog.digest.clone())
    );
    assert_eq!(
        outcome.receipt.changed_root_catalog_ids,
        w.changed_root_catalog_ids
    );
    assert_eq!(
        outcome.receipt.changed_index_catalog_ids,
        w.changed_index_catalog_ids
    );
    assert_eq!(outcome.receipt.records_backfilled, 2);

    let store_id = store_id_of(&accepted_place);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &pages_id, INT),
        Some(Scalar::Int(0))
    );
    assert_eq!(
        store.read_catalog_epoch().expect("epoch"),
        Some(proposal_epoch)
    );
}

#[test]
fn default_receipt_is_bounded_for_many_records() {
    let (_root, _program, _place, store, _pages_id) =
        applied_proposal_default_fixture("completion-default-bounded", 128);
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");

    assert_eq!(commit.activation_records_backfilled, 128);
    assert_eq!(commit.activation_default_records_by_id.len(), 1);
    let evidence = &commit.activation_default_records_by_id[0];
    assert_eq!(evidence.records_backfilled, 128);
    assert_eq!(evidence.target_records, 128);
    assert!(evidence.evidence_digest.starts_with("sha256:"));
    assert_eq!(
        evidence.evidence_digest.len(),
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".len()
    );
}

#[test]
fn completion_rejects_forged_default_receipt_digest() {
    let (_root, program, _place, store, _pages_id) =
        applied_proposal_default_fixture("completion-default-forged-digest", 2);
    let mut commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    commit.activation_default_records_by_id[0].evidence_digest =
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
    store
        .write_commit_metadata(&commit)
        .expect("forge default evidence");

    let error = verify_activation_completion(&program, &store, &commit)
        .expect_err("forged default receipt fails");

    assert_eq!(error, ApplyError::Drift);
}

#[test]
fn completion_rejects_missing_default_backfill_cell() {
    let (_root, program, place, store, pages_id) =
        applied_proposal_default_fixture("completion-default-missing-cell", 2);
    let store_id = store_id_of(&place);
    store
        .delete_data_subtree(
            &store_id,
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(CatalogId::new(pages_id).unwrap())],
        )
        .expect("delete defaulted cell");
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");

    let error = verify_activation_completion(&program, &store, &commit)
        .expect_err("missing default cell fails");

    assert_eq!(error, ApplyError::Drift);
}

#[test]
fn completion_rejects_engine_profile_drift() {
    let (_root, program, _place, store, _pages_id) =
        applied_proposal_default_fixture("completion-engine-profile-drift", 1);
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    store
        .write_engine_profile(&EngineProfile::new(
            current_engine_profile().layout_epoch() + 1,
        ))
        .expect("drift engine profile");

    let error = verify_activation_completion(&program, &store, &commit)
        .expect_err("engine profile drift fails");

    assert_eq!(error, ApplyError::Drift);
}

#[test]
fn completion_rejects_erased_commit_source_digest() {
    let (_root, program, _place, store, _pages_id) =
        applied_proposal_default_fixture("completion-source-digest-erased", 1);
    let mut commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    commit.source_digest.clear();
    store
        .write_commit_metadata(&commit)
        .expect("erase source digest");

    let error = verify_activation_completion(&program, &store, &commit)
        .expect_err("erased source digest fails");

    assert_eq!(error, ApplyError::Drift);
}

#[test]
fn completion_uses_applied_step_evidence_not_per_commit_changed_ids() {
    let (_root, program, _place, store, _pages_id) =
        applied_proposal_default_fixture("completion-changed-ids-cleared", 1);
    let mut commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    assert!(!commit.changed_root_catalog_ids.is_empty());
    commit.changed_root_catalog_ids.clear();
    commit.changed_index_catalog_ids.clear();
    store
        .write_commit_metadata(&commit)
        .expect("clear per-commit changed ids");

    verify_activation_completion(&program, &store, &commit)
        .expect("carried applied-step evidence does not include per-commit changed ids");
}

#[test]
fn proposal_required_default_rejects_preexisting_target_data() {
    let root = temp_project("apply-proposal-required-default-existing-target", |root| {
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
         evolve\n\
         \x20   default Book.pages = 0\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let pages_id = proposal_catalog_id(&program, "books::Book::pages");
    let store_id = store_id_of(&accepted_place);
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(
                CatalogId::new(pages_id.clone()).expect("pages id"),
            )],
            encode_value(&Scalar::Int(7)).expect("encode rogue value"),
        )
        .expect("seed rogue proposal target");

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_backfill, 1);
    let result = apply(&w, &program, &store, false, None);
    assert!(matches!(result, Err(ApplyError::Store(_))), "{result:#?}");

    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(7))
    );
    assert_eq!(read_scalar(&store, &store_id, 2, &pages_id, INT), None);
    assert_eq!(store.read_catalog_epoch().expect("epoch"), None);
    assert!(store.read_commit_metadata().expect("read commit").is_none());
}

#[test]
fn proposal_default_backfills_every_store_using_the_resource() {
    let root = temp_project("apply-proposal-default-multi-store", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             store ^archives(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
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
    books_seed.member(1, "title", Scalar::Str("Dune".into()));
    let archives_seed = Seed {
        store: &store,
        place: &archives_place,
    };
    archives_seed.record(2);
    archives_seed.member(2, "title", Scalar::Str("Kindred".into()));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         store ^archives(id: int): Book\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let pages_id = proposal_catalog_id(&program, "books::Book::pages");
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert_eq!(w.counts.records_to_backfill, 2);

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_backfilled, 2);

    let books_store_id = store_id_of(&books_place);
    let archives_store_id = store_id_of(&archives_place);
    assert_eq!(
        read_scalar(&store, &books_store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0))
    );
    assert_eq!(
        read_scalar(&store, &archives_store_id, 2, &pages_id, INT),
        Some(Scalar::Int(0))
    );
}

/// A required-with-default change backfills exactly the records lacking the member
/// and stamps the proposal epoch. The applied store carries the encoded default at
/// each old record and a commit stamp at the proposal epoch.
#[test]
fn required_with_default_backfills_exactly_k_and_stamps_epoch() {
    let root = temp_project("apply-required-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
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
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));
    // One record already carries pages, so the backfill must touch only the two that
    // lack it; preview counts records_to_backfill = 2.
    seed.record(3);
    seed.member(3, "title", Scalar::Str("Neuromancer".into()));
    seed.member(3, "pages", Scalar::Int(271));

    let w = witness(&program, &store);
    // The full schema (including required `pages` and the evolve default) was already
    // accepted, so source proposes no catalog change: apply stamps the accepted epoch
    // while the data catches up.
    assert!(w.proposal_catalog.is_none());
    let target_epoch = w.accepted_catalog.epoch;

    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    assert_eq!(outcome.receipt.records_backfilled, 2);
    assert_eq!(outcome.receipt.catalog_epoch, target_epoch);

    let store_id = store_id_of(&place);
    let pages_id = member_catalog_id(&place, "pages");
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0))
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &pages_id, INT),
        Some(Scalar::Int(0))
    );
    // The record that already had a value is untouched.
    assert_eq!(
        read_scalar(&store, &store_id, 3, &pages_id, INT),
        Some(Scalar::Int(271))
    );

    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("a stamp");
    assert_eq!(commit.catalog_epoch, target_epoch);
    assert_eq!(
        store.read_catalog_epoch().expect("epoch"),
        Some(target_epoch)
    );

    // Idempotent re-apply: the same source against the now-applied store re-previews
    // to a no-op for pages (every record carries it) and re-applying succeeds.
    let resumed = witness(&program, &store);
    let second = apply(&resumed, &program, &store, false, None).expect("re-apply succeeds");
    assert_eq!(second.receipt.records_backfilled, 0);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0))
    );
}
