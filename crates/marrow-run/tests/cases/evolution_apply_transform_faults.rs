//! Transform refusals and drift: a read whose stored bytes do not decode is refused, a
//! faulting body rolls the whole apply back byte-identical with no stamp (including
//! mid-scan after an earlier clean write), and editing the transform body or a module
//! constant after preview drifts the witness so apply fails closed before evaluating it.
use crate::evolution_apply_support;
use evolution_apply_support::*;

use marrow_run::evolution::{ApplyError, apply};
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, ScalarType, decode_value};

/// A transform whose read member's stored bytes do not decode under the current type
/// is non-activatable: apply refuses with a typed not-activatable error, staging no
/// write and leaving the store unstamped.
#[test]
fn transform_undecodable_read_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-undecodable", |root| {
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
    // `price` holds bytes that do not decode as the current `int`.
    seed.record(1);
    seed.member(1, "price", Scalar::Str("oops".into()));
    seed.member(1, "priceCents", Scalar::Int(0));

    let w = witness(&program, &store);
    assert!(!w.is_activatable(), "{w:#?}");
    let result = apply(&w, &program, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::NotActivatable)),
        "expected NotActivatable, got {result:#?}"
    );
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp"
    );

    Ok(())
}

/// A transform that recomputes an `ErrorCode` member from a string the developer's body
/// produces must enforce the same dotted-lowercase grammar the constructor and the
/// field-write path enforce. A body returning a non-conforming code is a per-record body
/// fault: apply names the target, rolls the whole transaction back byte-identical, and
/// leaves the store unstamped, so an invalid code can never be persisted to an `ErrorCode`
/// place through the transform write path.
#[test]
fn transform_into_error_code_rejects_invalid_code() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-error-code", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Log\n\
             \x20   required note: string\n\
             \x20   required code: ErrorCode\n\
             store ^logs(id: int): Log\n\
             evolve\n\
             \x20   transform Log.code\n\
             \x20       return old.note\n\
             pub fn add(note: string): Id(^logs)\n\
             \x20   return nextId(^logs)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "logs")?;
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    // `note` holds text that is not a valid error code; the transform body returns it.
    seed.record(1);
    seed.member(1, "note", Scalar::Str("this is not a valid code".into()));
    seed.member(1, "code", Scalar::Str("valid.code".into()));

    let store_id = store_id_of(&place)?;
    let code_id = member_catalog_id(&place, "code")?;
    let before = read_str(&store, &store_id, 1, &code_id);

    let w = witness(&program, &store);
    let result = apply(&w, &program, &store, false, None);

    let Err(ApplyError::TransformBodyFaulted { target, .. }) = &result else {
        panic!("expected TransformBodyFaulted, got {result:#?}");
    };
    assert_eq!(
        target.as_str(),
        code_id,
        "the fault names the transform target"
    );
    assert_eq!(
        read_str(&store, &store_id, 1, &code_id),
        before,
        "the invalid code is never written to the ErrorCode field"
    );
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp after a rejected transform body"
    );

    Ok(())
}

/// A transform that recomputes an `ErrorCode` member from a string that *is* a valid
/// dotted-lowercase code activates: the grammar gate accepts a conforming value, so the
/// recomputed code is written and the store is stamped. This proves the gate rejects only
/// non-conforming codes rather than every transform into an `ErrorCode` field.
#[test]
fn transform_into_error_code_accepts_valid_code() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-error-code-ok", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Log\n\
             \x20   required note: string\n\
             \x20   required code: ErrorCode\n\
             store ^logs(id: int): Log\n\
             evolve\n\
             \x20   transform Log.code\n\
             \x20       return old.note\n\
             pub fn add(note: string): Id(^logs)\n\
             \x20   return nextId(^logs)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "logs")?;
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "note", Scalar::Str("io.read".into()));
    seed.member(1, "code", Scalar::Str("old.code".into()));

    let store_id = store_id_of(&place)?;
    let code_id = member_catalog_id(&place, "code")?;

    let w = witness(&program, &store);
    apply(&w, &program, &store, false, None).expect("apply succeeds");

    assert_eq!(
        read_str(&store, &store_id, 1, &code_id),
        Some("io.read".to_string()),
        "the conforming recomputed code is written"
    );

    Ok(())
}

/// A pure transform body can still raise a genuine runtime fault over a record (here an
/// integer overflow). Apply reports a typed `TransformBodyFaulted` naming the target,
/// rolls the whole transaction back, and leaves the store byte-identical with no stamp:
/// a body fault is the developer's logic faulting on real data, not store corruption.
#[test]
fn transform_body_fault_aborts_byte_identical() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-fault", |root| {
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
             \x20       return old.price * 1000000000000\n\
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
    // `price` is large enough that `price * 1e12` overflows the 64-bit range.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(9_000_000_000));
    seed.member(1, "priceCents", Scalar::Int(0));

    let store_id = store_id_of(&place)?;
    let cents_id = member_catalog_id(&place, "priceCents")?;
    let before = read_scalar(&store, &store_id, 1, &cents_id, INT);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let result = apply(&w, &program, &store, false, None);

    let Err(ApplyError::TransformBodyFaulted { target, .. }) = &result else {
        panic!("expected TransformBodyFaulted, got {result:#?}");
    };
    assert_eq!(
        target.as_str(),
        cents_id,
        "the fault names the transform target"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        before,
        "the target cell is unchanged after a body fault"
    );
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp after a body fault"
    );

    Ok(())
}

/// A transform that recomputes the first record cleanly but faults on the second must
/// discard the first record's already-recomputed write: a transform that fails midway
/// rolls the whole apply back, leaving every target cell byte-identical and the store
/// unstamped. Record 1's `price` is small enough to transform, while record 2's overflows,
/// so apply stages record 1's write, then faults on record 2 before any commit.
#[test]
fn transform_body_fault_midscan_discards_earlier_staged_write()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-fault-midscan", |root| {
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
             \x20       return old.price * 1000000000000\n\
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
    // Record 1 transforms cleanly to a sentinel value; record 2 overflows the 64-bit range.
    seed.record(1);
    seed.member(1, "price", Scalar::Int(1));
    seed.member(1, "priceCents", Scalar::Int(0));
    seed.record(2);
    seed.member(2, "price", Scalar::Int(9_000_000_000));
    seed.member(2, "priceCents", Scalar::Int(0));

    let store_id = store_id_of(&place)?;
    let cents_id = member_catalog_id(&place, "priceCents")?;
    let before_one = read_scalar(&store, &store_id, 1, &cents_id, INT);
    let before_two = read_scalar(&store, &store_id, 2, &cents_id, INT);

    let w = witness(&program, &store);
    assert!(w.is_activatable(), "{w:#?}");
    let result = apply(&w, &program, &store, false, None);

    let Err(ApplyError::TransformBodyFaulted { target, .. }) = &result else {
        panic!("expected TransformBodyFaulted, got {result:#?}");
    };
    assert_eq!(
        target.as_str(),
        cents_id,
        "the fault names the transform target"
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

    Ok(())
}

/// A transform body can read module constants. Changing one after preview must drift
/// the witness before apply evaluates the new constant and writes unauthorized data.
#[test]
fn transform_constant_drift_aborts_before_apply() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-const-drift", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             const Scale = 100\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * Scale\n\
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
    seed.record(1);
    seed.member(1, "price", Scalar::Int(5));
    seed.member(1, "priceCents", Scalar::Int(0));
    let witness = witness(&program, &store);

    write(
        &root,
        "src/books.mw",
        "module books\n\
         const Scale = 200\n\
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * Scale\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let changed_program = checked(&root).expect("checked fixture");
    let result = apply(&witness, &changed_program, &store, false, None);

    let store_id = store_id_of(&place)?;
    let cents_id = member_catalog_id(&place, "priceCents")?;
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(0)),
        "the target is unchanged when const drift is rejected"
    );

    Ok(())
}

/// Editing a transform body after building the witness must abort apply with Drift,
/// even though the durable shape is unchanged. The store-stamp digest binds shape only,
/// so it does not move when the body changes; the witness records the evolution digest,
/// which does, so re-running preview produces an unequal witness and apply fails closed
/// before evaluating the new body. Without the evolution digest the verdict carries only
/// the read catalog ids, which a body edit leaves identical, so the change would slip
/// past every other witness field.
#[test]
fn transform_body_drift_aborts_before_apply() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-transform-body-drift", |root| {
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
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 200\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let changed_program = checked(&root).expect("checked fixture");
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

    let store_id = store_id_of(&place)?;
    let cents_id = member_catalog_id(&place, "priceCents")?;
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected Drift, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &cents_id, INT),
        Some(Scalar::Int(0)),
        "the target is unchanged when transform-body drift is rejected"
    );

    Ok(())
}

/// Read a stored `Str` member value as a `String`, or `None` when the cell is absent.
fn read_str(
    store: &TreeStore,
    store_id: &marrow_store::cell::CatalogId,
    id: i64,
    member_id: &str,
) -> Option<String> {
    let member = marrow_store::cell::CatalogId::new(member_id).expect("member id");
    let bytes = store
        .read_data_value(
            store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member)],
        )
        .expect("read member");
    bytes.map(
        |bytes| match decode_value(&bytes, ScalarType::Str).expect("decode value") {
            Scalar::Str(text) => text,
            other => panic!("expected a string value, got {other:?}"),
        },
    )
}
