//! Evolution apply publishes the activated catalog snapshot inside the apply
//! transaction. A committed activation advances the catalog snapshot, commit stamp,
//! source digest, and data together; a transaction that fails before commit rolls
//! all of them back; and an apply whose store catalog drifted from the witness fails
//! closed before staging.

mod evolution_apply_support;

use std::path::Path;

use evolution_apply_support::*;

use marrow_check::CheckedProgram;
use marrow_run::evolution::{ApplyError, apply, commit_catalog_baseline};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

/// Publish `source` as the store's baseline accepted catalog the way a state-establishing
/// run does — through `commit_catalog_baseline` over the uncommitted program — then return
/// the program re-checked against the published store snapshot, as production rebinds it.
fn publish_baseline(root: &Path, source: &str, store: &TreeStore) -> CheckedProgram {
    write(root, "src/books.mw", source);
    let pending = checked(root);
    let wrote = commit_catalog_baseline(store, &pending).expect("commit baseline");
    assert!(wrote, "the baseline proposal is published");
    recheck_against_snapshot(root, store)
}

/// Re-check the project binding the store's published accepted-catalog snapshot.
fn recheck_against_snapshot(root: &Path, store: &TreeStore) -> CheckedProgram {
    let accepted = store
        .read_catalog_snapshot()
        .expect("read snapshot")
        .expect("snapshot published");
    let (report, program) =
        marrow_check::check_project_with_catalog(root, &config(), Some(&accepted))
            .expect("re-check against snapshot");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

const PROPOSAL_DEFAULT: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required pages: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   default Book.pages = 0\n\
     pub fn add(title: string): Id(^books)\n\
     \x20   return nextId(^books)\n";

const BASELINE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn add(title: string): Id(^books)\n\
     \x20   return nextId(^books)\n";

// The evolved shape adds a new required `priceCents` member (advancing the catalog) whose
// backfilling transform overflows the 64-bit range, so the apply would publish the
// activated catalog but faults mid-transaction first.
const TRANSFORM_OVERFLOW: &str = "module books\n\
     resource Book\n\
     \x20   required price: int\n\
     \x20   required priceCents: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   transform Book.priceCents\n\
     \x20       return old.price * 1000000000000\n\
     pub fn add(price: int): Id(^books)\n\
     \x20   return nextId(^books)\n";

const TRANSFORM_BASELINE: &str = "module books\n\
     resource Book\n\
     \x20   required price: int\n\
     store ^books(id: int): Book\n\
     pub fn add(price: int): Id(^books)\n\
     \x20   return nextId(^books)\n";

/// A committed activation advances the store's catalog snapshot, commit stamp, source
/// digest, and backfilled data in one transaction. Reading them back proves they moved
/// together: the published snapshot is the activated proposal, not the baseline.
#[test]
fn committed_activation_advances_snapshot_epoch_digest_and_data_together() {
    let root = temp_project("apply-publishes-snapshot", |_| {});
    let store = TreeStore::memory();
    // Establish the baseline accepted catalog in the store, exactly as a state-establishing
    // run does, so the apply advances from a real published snapshot rather than from none.
    let accepted = publish_baseline(&root, BASELINE, &store);
    let accepted_place = root_place(&accepted, "books");
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let baseline_snapshot = store
        .read_catalog_snapshot()
        .expect("read baseline snapshot")
        .expect("baseline snapshot published");

    write(&root, "src/books.mw", PROPOSAL_DEFAULT);
    let program = recheck_against_snapshot(&root, &store);
    let proposal = program
        .catalog
        .proposal
        .clone()
        .expect("the default proposal");
    let w = witness(&program, &store);
    assert!(
        w.proposal_catalog.is_some(),
        "the default advances the catalog"
    );

    let outcome = apply(&w, &program, &store, false, None).expect("apply");

    // The published snapshot is now the activated proposal, advanced from the baseline.
    let published = store
        .read_catalog_snapshot()
        .expect("read snapshot")
        .expect("snapshot published");
    assert_eq!(published, proposal, "the activated proposal was published");
    assert_ne!(
        published.digest, baseline_snapshot.digest,
        "the snapshot advanced past the baseline"
    );
    assert_eq!(
        store.catalog_snapshot_digest().expect("digest"),
        Some(proposal.digest.clone()),
        "the snapshot digest is the activated proposal digest"
    );
    // The commit stamp, source digest, and data advanced in the same transaction.
    let commit = store
        .read_commit_metadata()
        .expect("commit")
        .expect("activation commit");
    assert_eq!(commit.source_digest, w.source_digest);
    assert_eq!(commit.catalog_epoch, proposal.epoch);
    let store_id = store_id_of(&accepted_place);
    let pages_id = proposal_catalog_id(&program, "books::Book::pages");
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0)),
        "the backfill committed with the snapshot"
    );
    assert_eq!(outcome.receipt.catalog_epoch, proposal.epoch);
}

/// A failure before the outer commit rolls the whole apply back: the catalog snapshot,
/// commit metadata, and data all stay at their pre-apply state. A transform whose body
/// overflows faults inside the transaction, so the activated snapshot is never published
/// over the baseline.
#[test]
fn apply_rollback_leaves_snapshot_epoch_and_data_at_pre_apply_state() {
    let root = temp_project("apply-rollback-no-publish", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, TRANSFORM_BASELINE, &store);
    let accepted_place = root_place(&accepted, "books");
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    // `price * 1e12` overflows the 64-bit range, so the backfilling transform faults.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(9_000_000_000));

    let snapshot_before = store.read_catalog_snapshot().expect("snapshot");
    let commit_before = store.read_commit_metadata().expect("commit");
    let store_id = store_id_of(&accepted_place);

    write(&root, "src/books.mw", TRANSFORM_OVERFLOW);
    let program = recheck_against_snapshot(&root, &store);
    let cents_id = proposal_catalog_id(&program, "books::Book::priceCents");
    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    assert!(
        w.proposal_catalog.is_some(),
        "the new member advances the catalog"
    );

    let result = apply(&w, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::TransformBodyFaulted { .. })),
        "expected TransformBodyFaulted, got {result:#?}"
    );

    // Catalog rows, metadata, and data are all exactly as before: nothing advanced.
    assert_eq!(
        store.read_catalog_snapshot().expect("snapshot"),
        snapshot_before,
        "the activated snapshot was never published over the baseline"
    );
    assert_eq!(store.read_commit_metadata().expect("commit"), commit_before);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        None,
        "the faulting transform wrote no target cell"
    );
}

/// Apply re-reads the store's published catalog snapshot and fails closed when it drifted
/// from the one the witness was built against. The witness discharged its obligations over
/// the baseline catalog; republishing a different catalog out from under it makes apply
/// refuse before staging rather than write a shape the store no longer accepts.
#[test]
fn apply_fails_closed_when_the_store_catalog_drifts_from_the_witness() {
    let root = temp_project("apply-catalog-fence", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, BASELINE, &store);
    let accepted_place = root_place(&accepted, "books");
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    write(&root, "src/books.mw", PROPOSAL_DEFAULT);
    let program = recheck_against_snapshot(&root, &store);
    let w = witness(&program, &store);

    // Republish a different catalog under the witness: same entries at a drifted epoch, so
    // the published snapshot digest no longer matches the accepted catalog the witness
    // fingerprints.
    let drifted = marrow_catalog::CatalogMetadata::new(
        999,
        store
            .read_catalog_snapshot()
            .expect("snapshot")
            .expect("baseline snapshot")
            .entries,
    );
    store.begin().expect("begin");
    store
        .replace_catalog_snapshot(&drifted)
        .expect("republish drifted snapshot");
    store.commit().expect("commit drift");

    let error = apply(&w, &program, &store, false, None).expect_err("apply fails closed");
    assert!(
        matches!(error, ApplyError::CatalogDrift { .. }),
        "expected CatalogDrift, got {error:#?}"
    );
    // Nothing was staged: the drifted snapshot is left as the test set it, the epoch did
    // not advance to the proposal, and no proposal commit was written.
    assert_eq!(
        store.catalog_snapshot_digest().expect("digest"),
        Some(drifted.digest)
    );
}
