//! The activation fence: a store this binary applies is stamped at the binary's own
//! engine profile, so the same binary passes the fence while one an epoch behind is
//! locked out. Apply fences before staging any write when the store evolved past the
//! binary, refuses when no catalog was ever accepted, and fences on engine-profile,
//! layout-epoch, or same-epoch schema drift.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_run::evolution::{ApplyError, FenceError, apply, current_engine_profile, fence};
use marrow_store::tree::{EngineProfile, TreeStore};
use marrow_store::value::Scalar;

/// A store this binary applies is stamped at the binary's own engine profile, so the
/// same binary that applied it passes the open fence, while a binary pinned one epoch
/// behind is locked out. This is the activation lockout the fence enforces without a
/// generation server: stamp and fence agree by construction.
#[test]
fn applied_store_passes_same_binary_fence_and_locks_out_older() {
    let root = temp_project("fence-lockout", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
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

    let w = witness(&program, &store);
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");
    let stamped_epoch = outcome.receipt.catalog_epoch;
    let accepted = program.catalog.accepted_epoch.expect("accepted epoch");
    assert_eq!(stamped_epoch, accepted);

    // The same binary reopening the store it just stamped is not fenced.
    fence(
        Some(accepted),
        &program.source_digest(),
        &current_engine_profile(),
        &store,
    )
    .expect("same binary proceeds");

    // A binary one accepted epoch behind is fenced: the store was evolved past it.
    let older = fence(
        Some(accepted - 1),
        &program.source_digest(),
        &current_engine_profile(),
        &store,
    )
    .expect_err("older binary fenced");
    assert_eq!(
        older,
        FenceError::StoreEvolved {
            stored: stamped_epoch,
            accepted: accepted - 1,
        }
    );
}

/// A stale binary must not apply over a store a newer binary already evolved past its
/// accepted epoch. Apply fences before staging any write, so the store is left
/// unchanged: no data, no stamp advance.
#[test]
fn apply_is_fenced_when_store_evolved_past_the_binary() {
    let root = temp_project("fence-apply-stale", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let accepted = program.catalog.accepted_epoch.expect("accepted epoch");
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    // Stamp the store as a newer binary would: a catalog epoch past this program's
    // accepted epoch, with this binary's engine profile so only the epoch fences.
    store
        .write_catalog_epoch(accepted + 1)
        .expect("stamp newer epoch");
    store
        .write_engine_profile(&current_engine_profile())
        .expect("stamp profile");

    let w = witness(&program, &store);
    let error = apply(&w, &program, &store, false, None).expect_err("stale apply fenced");
    assert_eq!(
        error,
        ApplyError::Fenced(FenceError::StoreEvolved {
            stored: accepted + 1,
            accepted,
        })
    );
    // The store was not advanced and no data was written.
    assert_eq!(
        store.read_catalog_epoch().expect("epoch"),
        Some(accepted + 1)
    );
    let store_id = store_id_of(&place);
    let pages_id = member_catalog_id(&place, "pages");
    assert_eq!(read_scalar(&store, &store_id, 1, &pages_id, INT), None);
}

/// An engine-profile drift fences a run-capable open even when the catalog epoch
/// matches: the physical layout the store recorded is not the one this binary writes.
#[test]
fn engine_profile_drift_fences_a_matching_epoch_store() {
    let store = TreeStore::memory();
    store.write_catalog_epoch(2).expect("epoch");
    store
        .write_engine_profile(&EngineProfile::new(
            current_engine_profile().layout_epoch() + 1,
        ))
        .expect("drifted profile");
    let error = fence(
        Some(2),
        "sha256:0000000000000000000000000000000000000000000000000000000000000002",
        &current_engine_profile(),
        &store,
    )
    .expect_err("drift fenced");
    assert_eq!(error, FenceError::EngineProfileDrift);
}

/// A store that predates digest stamping carries only a layout epoch. A drifted
/// layout epoch must fence even without a profile digest to compare.
#[test]
fn layout_epoch_drift_without_profile_digest_is_fenced() {
    let store = TreeStore::memory();
    store.write_catalog_epoch(2).expect("epoch");
    store
        .write_layout_epoch(current_engine_profile().layout_epoch() + 1)
        .expect("layout epoch stamp");

    let error = fence(
        Some(2),
        "sha256:0000000000000000000000000000000000000000000000000000000000000002",
        &current_engine_profile(),
        &store,
    )
    .expect_err("fenced");
    assert_eq!(error, FenceError::EngineProfileDrift);
}

/// Apply over a program that accepted no catalog has no baseline epoch to advance from.
/// It must refuse with a typed error and leave the store untouched rather than stamp a
/// phantom proposal epoch and churn the commit id on every retry.
#[test]
fn apply_without_accepted_catalog_refuses_and_leaves_store_unchanged() {
    let root = temp_project("apply-no-accepted", |root| {
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
    // The program proposes a catalog but none has been committed. Without a committed
    // catalog the bound store catalog ids are unresolved, so there is nothing to seed;
    // apply must refuse before reaching any data work.
    let program = checked(&root);
    assert!(program.catalog.accepted_epoch.is_none());
    let store = TreeStore::memory();

    let w = witness(&program, &store);
    let error = apply(&w, &program, &store, false, None).expect_err("apply refuses");
    assert_eq!(error, ApplyError::NoAcceptedCatalog);
    assert_eq!(
        store.read_catalog_epoch().expect("epoch"),
        None,
        "no phantom epoch was stamped"
    );
    assert_eq!(
        store.read_commit_metadata().expect("commit"),
        None,
        "no commit id churned"
    );
}

/// A store stamped under schema A is fenced when opened by a binary compiled against a
/// structurally different schema B at the same catalog epoch. The catalog epoch alone
/// cannot tell the two schemas apart; the schema-bearing source digest does. Without the
/// digest fence, schema B would proceed past activation against bytes shaped for A.
#[test]
fn schema_drift_at_the_same_epoch_is_fenced_before_execution() {
    let root_a = temp_project("fence-schema-a", |root| {
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
    let program_a = commit_then_check(&root_a);
    let place_a = root_place(&program_a, "books");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place_a,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    apply(
        &witness(&program_a, &store),
        &program_a,
        &store,
        false,
        None,
    )
    .expect("apply schema A");
    let accepted = program_a.catalog.accepted_epoch.expect("accepted epoch");

    // Schema B accepts at the same first epoch but binds `title` to a different type, so
    // its source digest differs from the one the store recorded under schema A.
    let root_b = temp_project("fence-schema-b", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: int\n\
             pub fn add(title: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program_b = commit_then_check(&root_b);
    assert_eq!(
        program_b.catalog.accepted_epoch.expect("accepted epoch"),
        accepted,
        "both schemas accept at the same epoch"
    );
    assert_ne!(
        program_a.source_digest(),
        program_b.source_digest(),
        "the two schemas carry distinct source digests"
    );

    let error = fence(
        Some(accepted),
        &program_b.source_digest(),
        &current_engine_profile(),
        &store,
    )
    .expect_err("schema B fenced against schema A's store");
    assert_eq!(error, FenceError::SchemaDrift);
    assert_eq!(error.code(), "run.schema_drift");
}
