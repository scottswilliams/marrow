//! Transform refusals and drift: a read whose stored bytes do not decode is refused, a
//! faulting body rolls the whole apply back byte-identical with no stamp (including
//! mid-scan after an earlier clean write), and editing the transform body or a module
//! constant after preview drifts the witness so apply fails closed before evaluating it.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_run::evolution::{ApplyError, apply};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use std::fs;

/// A transform whose read member's stored bytes do not decode under the current type
/// is non-activatable: apply refuses with a typed not-activatable error, staging no
/// write and leaving the store unstamped.
#[test]
fn transform_undecodable_read_is_refused() {
    let root = temp_project("apply-transform-undecodable", |root| {
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
    // `price` holds bytes that do not decode as the current `int`.
    seed.record(1);
    seed.member(1, "price", Scalar::Str("oops".into()));
    seed.member(1, "priceCents", Scalar::Int(0));

    let w = witness(&program, &store);
    assert!(!w.is_activatable(), "{w:#?}");
    let result = apply(&w, &program, &store, true, None);
    assert!(
        matches!(result, Err(ApplyError::NotActivatable)),
        "expected NotActivatable, got {result:#?}"
    );
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp"
    );
    fs::remove_dir_all(&root).ok();
}

/// A pure transform body can still raise a genuine runtime fault over a record (here an
/// integer overflow). Apply reports a typed `TransformBodyFaulted` naming the target,
/// rolls the whole transaction back, and leaves the store byte-identical with no stamp:
/// a body fault is the developer's logic faulting on real data, not store corruption.
#[test]
fn transform_body_fault_aborts_byte_identical() {
    let root = temp_project("apply-transform-fault", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 1000000000000\n\
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
    // `price` is large enough that `price * 1e12` overflows the 64-bit range.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(9_000_000_000));
    seed.member(1, "priceCents", Scalar::Int(0));

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    let before = read_scalar(&store, &store_id, 1, &cents_id, INT);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let result = apply(&w, &program, &store, false, None);
    let cents_path = cents_id.clone();
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(result, Err(ApplyError::TransformBodyFaulted { .. })),
        "expected TransformBodyFaulted, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_path, INT),
        before,
        "the target cell is unchanged after a body fault"
    );
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp after a body fault"
    );
}

/// A transform that recomputes the first record cleanly but faults on the second must
/// discard the first record's already-recomputed write: a transform that fails midway
/// rolls the whole apply back, leaving every target cell byte-identical and the store
/// unstamped. Record 1's `price` is small enough to transform, while record 2's overflows,
/// so apply stages record 1's write, then faults on record 2 before any commit.
#[test]
fn transform_body_fault_midscan_discards_earlier_staged_write() {
    let root = temp_project("apply-transform-fault-midscan", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 1000000000000\n\
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
    // Record 1 transforms cleanly to a sentinel value; record 2 overflows the 64-bit range.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(1));
    seed.member(1, "priceCents", Scalar::Int(0));
    seed.record(2);
    seed.member(2, "price", Scalar::Int(9_000_000_000));
    seed.member(2, "priceCents", Scalar::Int(0));

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    let before_one = read_scalar(&store, &store_id, 1, &cents_id, INT);
    let before_two = read_scalar(&store, &store_id, 2, &cents_id, INT);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let result = apply(&w, &program, &store, false, None);
    fs::remove_dir_all(&root).ok();

    assert!(
        matches!(result, Err(ApplyError::TransformBodyFaulted { .. })),
        "expected TransformBodyFaulted, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        before_one,
        "the cleanly-transformed record's staged write must not survive a later fault"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &cents_id, INT),
        before_two,
        "the faulting record's target is unchanged"
    );
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "a mid-scan transform fault leaves the store unstamped"
    );
}

/// A transform body can read module constants. Changing one after preview must drift
/// the witness before apply evaluates the new constant and writes unauthorized data.
#[test]
fn transform_constant_drift_aborts_before_apply() {
    let root = temp_project("apply-transform-const-drift", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             const Scale = 100\n\
             resource Book at ^books(id: int)\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * Scale\n\
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
    let witness = witness(&program, &store);

    write(
        &root,
        "src/books.mw",
        "module books\n\
         const Scale = 200\n\
         resource Book at ^books(id: int)\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * Scale\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let changed_program = checked(&root);
    let result = apply(&witness, &changed_program, &store, false, None);

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(0)),
        "the target is unchanged when const drift is rejected"
    );
    fs::remove_dir_all(&root).ok();
}

/// Editing a transform body after building the witness must abort apply with Drift,
/// even though the durable shape is unchanged. The store-stamp digest binds shape only,
/// so it does not move when the body changes; the witness records the evolution digest,
/// which does, so re-running preview produces an unequal witness and apply fails closed
/// before evaluating the new body. Without the evolution digest the verdict carries only
/// the read catalog ids, which a body edit leaves identical, so the change would slip
/// past every other witness field.
#[test]
fn transform_body_drift_aborts_before_apply() {
    let root = temp_project("apply-transform-body-drift", |root| {
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
    let witness = witness(&program, &store);

    // The shape is untouched; only the transform body changes the multiplier. The
    // shape-only stamp digest is identical, so without the evolution digest this edit
    // would not drift the witness.
    assert_eq!(
        witness.source_digest,
        program.source_digest(),
        "the shape digest is unchanged by the body edit"
    );
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 200\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let changed_program = checked(&root);
    assert_eq!(
        changed_program.source_digest(),
        witness.source_digest,
        "the body edit leaves the shape digest unchanged"
    );
    assert_ne!(
        changed_program.evolution_digest(),
        witness.evolution_digest,
        "the body edit drifts the evolution digest the witness records"
    );
    let result = apply(&witness, &changed_program, &store, false, None);

    let store_id = store_id_of(&place);
    let cents_id = member_catalog_id(&place, "priceCents");
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(0)),
        "the target is unchanged when transform-body drift is rejected"
    );
    fs::remove_dir_all(&root).ok();
}
