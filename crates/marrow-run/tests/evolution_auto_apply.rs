//! Run-time auto-apply through the production preview/apply path. The auto-apply decision
//! is computed from the same witness `evolve preview`/`evolve apply` own, and discharges a
//! zero-record-mutation evolution through the real `apply`. These cases prove the decision
//! and the stamp bind to the same committed state: a zero-mutation change auto-applies and
//! advances the epoch, an obligation that mutates records fences instead, and a witness
//! whose pinned store commit id is stale fails the apply closed rather than stamping
//! against state it no longer describes.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_run::evolution::{AutoApplyOutcome, RunObligation, try_auto_apply};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

/// Add a sparse field over a populated store: discharging it writes no record, so the run
/// auto-applies it. The store advances to the proposal epoch and stamps the new shape.
#[test]
fn a_sparse_add_over_a_populated_store_auto_applies_and_advances_the_epoch() {
    let root = temp_project("autoapply-sparse-populated", |root| {
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

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let w = witness(&program, &store);
    let target_epoch = w
        .proposal_catalog
        .as_ref()
        .map(|catalog| catalog.epoch)
        .expect("a proposal epoch to advance to");

    assert_eq!(
        RunObligation::classify(&w),
        RunObligation::ZeroMutation {
            empty_retires: Vec::new()
        },
    );
    let outcome = try_auto_apply(&w, &program, &store).expect("auto-apply");
    assert_eq!(outcome, AutoApplyOutcome::Applied);
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit")
            .map(|commit| commit.catalog_epoch),
        Some(target_epoch),
        "auto-apply advanced the store to the proposal epoch",
    );
}

/// A required field added over a populated store has records to backfill, so the run must
/// not auto-apply it: it returns the backfill obligation to fence, and the store keeps no
/// stamp.
#[test]
fn a_required_add_over_a_populated_store_must_fence() {
    let root = temp_project("autoapply-required-populated", |root| {
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
    let w = witness(&program, &store);

    let outcome = try_auto_apply(&w, &program, &store).expect("classify without applying");
    assert_eq!(
        outcome,
        AutoApplyOutcome::MustFence(RunObligation::Backfill { records: 1 }),
    );
    assert_eq!(
        store.read_commit_metadata().expect("read commit"),
        None,
        "a fenced obligation stamps nothing",
    );
}

/// A destructive drop over populated data is the approval-gated case: it never
/// auto-applies regardless of how valid the change is, and the store keeps no stamp.
#[test]
fn a_populated_destructive_drop_never_auto_applies() {
    let (_root, program, _place, store, _subtitle_id) =
        destructive_retire_fixture("autoapply-destructive-drop");
    let w = witness(&program, &store);

    let outcome = try_auto_apply(&w, &program, &store).expect("classify without applying");
    assert_eq!(
        outcome,
        AutoApplyOutcome::MustFence(RunObligation::DestructiveDrop { populated: 2 }),
    );
    assert_eq!(
        store.read_commit_metadata().expect("read commit"),
        None,
        "a destructive drop stamps nothing on a bare run",
    );
}

/// The adversarial empty-drop race: an empty-target drop classifies as zero-mutation and
/// the auto-apply authorizes it with a zero-count approval. If a concurrent write
/// populates the dropped member between the probe and the stamp, the apply re-previews the
/// live store, re-classifies the drop as destructive over real data, and the witness no
/// longer matches — so the apply fails closed rather than silently dropping the now-present
/// cell under the stale zero-count approval. Losing data is never a silent side effect of
/// an auto-apply.
#[test]
fn an_empty_drop_that_becomes_populated_before_the_stamp_fails_closed() {
    let (_root, program, place, store, subtitle_id) =
        destructive_retire_fixture("autoapply-empty-drop-race");
    // The fixture seeds two subtitle cells; clear them so the retire targets an empty
    // member and classifies as zero-mutation, exactly the auto-apply case.
    let store_id = store_id_of(&place);
    for id in [1, 2] {
        store
            .delete_data_subtree(
                &store_id,
                &[marrow_store::key::SavedKey::Int(id)],
                &[marrow_store::tree::DataPathSegment::Member(
                    marrow_store::cell::CatalogId::new(subtitle_id.clone()).expect("subtitle id"),
                )],
            )
            .expect("clear seeded subtitle cells");
    }
    let w = witness(&program, &store);
    assert_eq!(
        RunObligation::classify(&w),
        RunObligation::ZeroMutation {
            empty_retires: vec![
                marrow_store::cell::CatalogId::new(subtitle_id.clone()).expect("subtitle id")
            ]
        },
        "with no subtitle cells the retire is a zero-mutation drop",
    );
    // A concurrent writer repopulates the dropped member after the witness was taken.
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.member_by_id(1, &subtitle_id, Scalar::Str("resurrected".into()));

    let result = try_auto_apply(&w, &program, &store);
    assert!(
        result.is_err(),
        "an empty-drop witness must not stamp once its target is repopulated, got {result:?}",
    );
    assert_eq!(
        store
            .read_data_value(
                &store_id,
                &[marrow_store::key::SavedKey::Int(1)],
                &[marrow_store::tree::DataPathSegment::Member(
                    marrow_store::cell::CatalogId::new(subtitle_id).expect("subtitle id"),
                )],
            )
            .expect("read subtitle"),
        Some(
            marrow_store::value::encode_value(&Scalar::Str("resurrected".into())).expect("encode")
        ),
        "the concurrently-written cell survives the failed auto-apply",
    );
}

/// The TOCTOU invariant, encoded deterministically: the auto-apply decision is bound to
/// the store commit id the witness pinned. A witness whose pinned commit id no longer
/// matches the store — exactly what a concurrent writer that committed after the probe
/// produces — fails the apply closed inside the write transaction rather than stamping a
/// stale zero-mutation decision. The change here is intrinsically additive, so its
/// classification stays `ZeroMutation`; only the stale pin distinguishes the safe stamp
/// from the unsafe one, proving the probe and the stamp serialize on the same committed
/// state.
#[test]
fn a_stale_commit_pin_fails_the_auto_apply_closed() {
    let root = temp_project("autoapply-toctou-pin", |root| {
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

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let mut w = witness(&program, &store);
    // The witness is additive, so it classifies as zero-mutation and would auto-apply.
    assert_eq!(
        RunObligation::classify(&w),
        RunObligation::ZeroMutation {
            empty_retires: Vec::new()
        },
    );
    // Model a writer that committed between the probe and the stamp: the store no longer
    // sits at the commit id the witness pinned.
    w.store_commit_id = Some(99);
    let result = try_auto_apply(&w, &program, &store);
    assert!(
        result.is_err(),
        "a stale commit pin must fail the auto-apply closed, got {result:?}",
    );
    assert_eq!(
        store.read_commit_metadata().expect("read commit"),
        None,
        "the failed auto-apply stamps nothing",
    );
}
