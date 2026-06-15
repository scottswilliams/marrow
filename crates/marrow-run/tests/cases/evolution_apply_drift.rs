//! Apply drift, rollback, and idempotence: the witness is the only input crossing the
//! check->run boundary, so tampering its source digest, backfill count, or pinned store
//! commit id aborts before any write. An optional add stamps without a data step, a
//! failed apply rolls back and a resumed apply succeeds, and a no-op re-apply does not
//! churn the commit id.
use crate::evolution_apply_support;
use evolution_apply_support::*;

use marrow_run::evolution::{ApplyError, apply, current_engine_profile};
use marrow_store::StoreError;
use marrow_store::tree::{CommitMetadata, TreeStore};
use marrow_store::value::Scalar;

/// An optional sparse add is a no-op: apply stamps the proposal epoch with no data
/// step. The store is stamped but carries no new member cell.
#[test]
fn optional_add_stamps_epoch_without_data_step() {
    let root = temp_project("apply-optional-add", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let witness = witness(&program, &store);
    let proposal_epoch = witness.proposal_catalog.as_ref().map(|c| c.epoch);
    let proposal_digest = witness
        .proposal_catalog
        .as_ref()
        .map(|catalog| catalog.digest.clone());
    let outcome = apply(&witness, &program, &store, false, None).expect("apply");
    assert_eq!(outcome.receipt.records_backfilled, 0);
    assert_eq!(outcome.receipt.default_records_by_id.len(), 0);
    assert_eq!(outcome.receipt.records_transformed, 0);
    assert_eq!(outcome.receipt.indexes_rebuilt, 0);
    assert_eq!(outcome.receipt.records_retired, 0);
    assert_eq!(outcome.receipt.proposal_catalog_digest, proposal_digest);

    let store_id = store_id_of(&place);
    let subtitle_id = member_catalog_id(&place, "subtitle");
    assert_eq!(
        read_scalar(
            &store,
            &store_id,
            1,
            &subtitle_id,
            marrow_store::value::ScalarType::Str
        ),
        None,
        "an optional add writes no data"
    );
    // The epoch was still stamped so old binaries are fenced.
    if let Some(epoch) = proposal_epoch {
        assert_eq!(
            store
                .read_commit_metadata()
                .expect("read commit")
                .map(|commit| commit.catalog_epoch),
            Some(epoch)
        );
    }
}

/// Source-digest drift: the witness no longer matches what the source now discharges.
/// Apply aborts with a typed drift error before staging a write.
#[test]
fn source_digest_drift_aborts() {
    let root = temp_project("apply-source-drift", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let mut witness = witness(&program, &store);
    witness.source_digest =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(store.read_commit_metadata().expect("read"), None);
}

/// Count drift: the witness backfill count no longer matches the live store, so apply
/// aborts before staging a write. Witness equality catches the count change because a
/// re-preview produces a different count.
#[test]
fn count_drift_aborts() {
    let root = temp_project("apply-count-drift", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let mut witness = witness(&program, &store);
    // Simulate a concurrent writer adding a record after the witness was taken: the
    // live re-preview now counts one more record to backfill.
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));
    // Tamper the witness count to a stale value so the only mismatch is the count.
    witness.counts.records_to_backfill = 999;
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(store.read_commit_metadata().expect("read"), None);
}

/// Store-commit drift: a concurrent writer advanced the store commit id after the
/// witness pinned it, so apply aborts. The witness pins `store_commit_id`; tampering
/// it to a stale value models the store moving under the apply.
#[test]
fn store_commit_drift_aborts() {
    let root = temp_project("apply-commit-drift", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let mut witness = witness(&program, &store);
    // The witness was taken against a store with no commit stamp (None). Pin it to a
    // value the store does not hold, modelling a writer that committed after preview.
    witness.store_commit_id = Some(42);
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::StoreCommitDrift { .. })),
        "expected StoreCommitDrift, got {result:#?}"
    );
    assert_eq!(store.read_commit_metadata().expect("read"), None);
}

#[test]
fn commit_id_overflow_aborts_without_staging_apply_writes() {
    let root = temp_project("apply-commit-overflow", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    let mut predecessor = commit_metadata(u64::MAX, program.source_digest());
    predecessor.catalog_epoch = program.catalog.accepted_epoch.expect("accepted epoch");
    store
        .write_commit_metadata(&predecessor)
        .expect("stamp predecessor at the commit-id limit");

    let witness = witness(&program, &store);
    assert_eq!(witness.store_commit_id, Some(u64::MAX));
    let result = apply(&witness, &program, &store, false, None);
    assert!(
        matches!(
            result,
            Err(ApplyError::Store(StoreError::LimitExceeded {
                limit: "commit id"
            }))
        ),
        "expected checked commit-id overflow, got {result:#?}"
    );

    let store_id = store_id_of(&place);
    let pages_id = member_catalog_id(&place, "pages");
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        None,
        "the failed apply must not stage the default backfill"
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit")
            .expect("predecessor stamp remains")
            .commit_id,
        u64::MAX
    );
}

/// A failed apply leaves no stamp and a resumed apply re-previews and succeeds
/// (idempotent). A read-only store handle fails the apply, so nothing lands; re-opening
/// the same file read-write and re-applying lands the change, proving the apply wiring
/// commits nothing on failure and that resume is a no-op for data a record already
/// carries. The byte-identical mid-plan rollback after a fault that strikes between two
/// staged writes is proven by the store's transaction-bracket test, which owns that
/// invariant; here the read-only handle aborts before the first write.
#[test]
fn failed_apply_rolls_back_and_resumes_idempotently() {
    let root = temp_project("apply-rollback", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");

    let store_path = root.join("data.marrow");
    {
        let store = TreeStore::open(&store_path).expect("open store");
        let seed = Seed {
            store: &store,
            place: &place,
        };
        seed.record(1);
        seed.member(1, "title", Scalar::Str("Dune".into()));
        seed.record(2);
        seed.member(2, "title", Scalar::Str("Hyperion".into()));
    }

    let store_id = store_id_of(&place);
    let pages_id = member_catalog_id(&place, "pages");

    // A read-only handle fails the apply commit; nothing must land.
    {
        let ro = TreeStore::open_read_only(&store_path).expect("open read only");
        let witness = witness(&program, &ro);
        let result = apply(&witness, &program, &ro, false, None);
        assert!(result.is_err(), "read-only apply must fail");
        assert_eq!(ro.read_commit_metadata().expect("read"), None, "no stamp");
        assert_eq!(
            read_scalar(&ro, &store_id, 1, &pages_id, INT),
            None,
            "no partial backfill"
        );
    }

    // Resume against a writable handle: the same source re-previews to the same
    // witness shape and apply now succeeds, backfilling both records.
    {
        let rw = TreeStore::open(&store_path).expect("reopen store");
        let witness = witness(&program, &rw);
        let outcome = apply(&witness, &program, &rw, false, None).expect("resumed apply");
        assert_eq!(outcome.receipt.records_backfilled, 2);
        assert_eq!(
            read_scalar(&rw, &store_id, 1, &pages_id, INT),
            Some(Scalar::Int(0))
        );
        assert_eq!(
            read_scalar(&rw, &store_id, 2, &pages_id, INT),
            Some(Scalar::Int(0))
        );
        assert!(rw.read_commit_metadata().expect("read").is_some());
    }
}

/// A no-op evolution — the store already sits at the program's accepted epoch with no
/// data work to do — must not restamp metadata or advance the commit id on a repeat
/// apply. Re-applying is genuinely idempotent: the commit id is unchanged.
#[test]
fn no_op_apply_does_not_churn_the_commit_id() {
    let root = temp_project("apply-noop", |root| {
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
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    // First apply backfills and stamps.
    let first = apply(&witness(&program, &store), &program, &store, false, None).expect("apply");
    let stamped_commit = store
        .read_commit_metadata()
        .expect("commit")
        .expect("a stamp")
        .commit_id;
    assert_eq!(first.receipt.commit_id, stamped_commit);

    // Second apply over the now-applied store has nothing to do and the epoch already
    // matches: it reports the existing commit id and writes no new stamp.
    let second =
        apply(&witness(&program, &store), &program, &store, false, None).expect("re-apply");
    assert_eq!(second.receipt.records_backfilled, 0);
    assert_eq!(second.receipt.commit_id, stamped_commit);
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("commit")
            .expect("a stamp")
            .commit_id,
        stamped_commit,
        "a no-op re-apply does not churn the commit id"
    );

    // A third apply is still a stable no-op.
    let third =
        apply(&witness(&program, &store), &program, &store, false, None).expect("third apply");
    assert_eq!(third.receipt.commit_id, stamped_commit);
}

fn commit_metadata(commit_id: u64, source_digest: String) -> CommitMetadata {
    let profile = current_engine_profile();
    CommitMetadata {
        commit_id,
        catalog_epoch: 0,
        layout_epoch: profile.layout_epoch(),
        source_digest,
        engine_profile_digest: profile.digest_bytes(),
        changed_root_catalog_ids: Vec::new(),
        changed_index_catalog_ids: Vec::new(),
    }
}
