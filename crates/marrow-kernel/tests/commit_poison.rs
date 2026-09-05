//! Indeterminate commit recovery is affine, exact, and never retried.
//!
//! A durable commit resolves to exactly one of confirmed, aborted, or indeterminate.
//! A confirmed commit reports `Committed`; a clean abort reports `Aborted` and leaves
//! the store usable. An indeterminate commit latches the poison flag and returns one
//! opaque non-cloneable recovery fact owning the exact before and proposed-after
//! witness states. Consuming that fact classifies known-new, known-old, or unknown.
//!
//! These drive the production kernel commit path (`TxnSession::commit`, the exact call
//! `marrow-vm` issues for `TxnCommit`) through a fault-injecting engine double whose
//! transaction reports a chosen [`CommitOutcome`] while independently either persisting
//! or discarding the staged bytes — so the classify path is exercised both ways.

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::durable::{
    CommitResult, Durable, DurableStore, EntryValue, InvocationGrant, KernelFault, NativeStore,
    SessionError,
};
use marrow_kernel::equality::ValueDomain;

#[path = "common/fault_engine.rs"]
mod fault_engine;
use fault_engine::{
    FaultEngine, Mode, ModeHandle, WriteFaultHandle, project, schema, sites, unscoped_store, write,
};

fn entry(v: i64) -> EntryValue {
    EntryValue {
        groups: Vec::new(),
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(v)))],
    }
}

#[test]
fn scoped_native_reopen_leaves_a_missing_engine_path_absent() {
    let dir = std::env::temp_dir().join(format!(
        "marrow-kernel-existing-reopen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let path = dir.join("store.redb");
    assert!(
        NativeStore::acquire_existing(&dir)
            .expect("acquire the owner over a store directory with no engine")
            .bind_and_open_existing([0x61; 16], project(&schema(), sites()), || Ok::<
                _,
                std::convert::Infallible,
            >(()))
            .is_err(),
        "a scoped lifecycle reopen must refuse a missing engine",
    );
    assert!(
        !path.exists(),
        "a scoped lifecycle reopen must never create the missing engine path",
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn commit_one(store: &mut DurableStore<FaultEngine>, key: &str, v: i64) -> CommitResult {
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let site = txn.site(0);
    txn.create_entry(&site, &[KeyScalar::Str(key.into())], entry(v))
        .expect("create");
    txn.commit()
}

/// An indeterminate commit returns one affine fact, latches poison, and the latch is
/// consulted at session open (E02 residue, F02a): every later session open — read or
/// write — refuses with [`SessionError::Poisoned`], so a poisoned handle can neither
/// replay a commit nor observe its own indeterminate state. Recovery consumes the fact and
/// classifies exact before/after state.
#[test]
fn an_indeterminate_commit_poisons_and_every_later_session_open_refuses() {
    let mode = ModeHandle::new(Mode::IndeterminateDrop);
    let mut store = unscoped_store(FaultEngine::new(mode.clone()));

    let recovery = match commit_one(&mut store, "a", 1) {
        CommitResult::Indeterminate(recovery) => recovery,
        other => panic!("expected an indeterminate result, got {other:?}"),
    };
    // A "retry" would be a fresh, well-formed transaction. Even with the double flipped to
    // confirm, the latch consulted at open refuses the transaction session outright — no
    // replay, only a reopen — and a read session is refused for the same reason: the
    // store's state is indeterminate until reclassified.
    mode.set(Mode::Confirm);
    assert!(
        matches!(
            store.txn_session(InvocationGrant::full_store(), write()),
            Err(SessionError::Poisoned)
        ),
        "a poisoned handle refuses a later transaction open rather than replaying",
    );
    assert!(
        matches!(
            store.read_session(InvocationGrant::full_store(), write()),
            Err(SessionError::Poisoned)
        ),
        "a poisoned handle refuses a later read open until a reopen reclassifies",
    );
    drop(recovery);
    assert!(matches!(
        store.read_session(InvocationGrant::full_store(), write()),
        Err(SessionError::Poisoned),
    ));
}

/// A transaction's commit boundary is one-shot even though the VM-facing trait takes
/// `&mut self`: once the engine has returned an indeterminate verdict, the same session
/// cannot manufacture a later known-old result. The first call retains sole ownership of
/// the affine recovery fact and the store remains poisoned; the repeated call reports only
/// that this session has already crossed its terminal boundary.
#[test]
fn repeating_an_indeterminate_commit_never_reclassifies_it_as_aborted() {
    let mode = ModeHandle::new(Mode::IndeterminateDrop);
    let mut store = unscoped_store(FaultEngine::new(mode));
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let site = txn.site(0);
    txn.create_entry(&site, &[KeyScalar::Str("a".into())], entry(1))
        .expect("create");

    let recovery = match txn.commit() {
        CommitResult::Indeterminate(recovery) => recovery,
        other => panic!("expected an indeterminate result, got {other:?}"),
    };
    assert!(matches!(txn.commit(), CommitResult::SessionFinished));
    drop(txn);

    drop(recovery);
    assert!(matches!(
        store.read_session(InvocationGrant::full_store(), write()),
        Err(SessionError::Poisoned),
    ));
}

/// A custom engine entering through the ordinary public constructor has no persistent
/// lifecycle provenance. Its affine fact remains truthful — the originating handle is
/// poisoned and the fact is consumed — but neither the persisted nor dropped half may claim
/// `KnownNew` or `KnownOld` against a later public handle.
#[test]
fn unscoped_indeterminate_facts_expose_no_persistent_classifier() {
    for mode in [Mode::IndeterminatePersist, Mode::IndeterminateDrop] {
        let backing = FaultEngine::new(ModeHandle::new(mode));
        let mut store = unscoped_store(backing.clone());
        let recovery = match commit_one(&mut store, "a", 1) {
            CommitResult::Indeterminate(recovery) => recovery,
            other => panic!("expected an indeterminate result, got {other:?}"),
        };
        drop(store);

        let mut reopened = unscoped_store(backing);
        drop(recovery);
        reopened
            .read_session(InvocationGrant::full_store(), write())
            .expect("a fresh generic handle is unscoped and has no classifier");
    }
}

#[test]
fn a_clean_abort_faults_without_poisoning() {
    let mode = ModeHandle::new(Mode::Abort);
    let mut store = unscoped_store(FaultEngine::new(mode.clone()));

    let aborted = commit_one(&mut store, "a", 1);
    assert!(matches!(aborted, CommitResult::Aborted));

    // Not poisoned: a later commit succeeds where a poisoned store would have faulted.
    mode.set(Mode::Confirm);
    let next = commit_one(&mut store, "b", 2);
    assert!(
        matches!(next, CommitResult::Committed),
        "a clean abort leaves the store usable"
    );

    // And the aborted write never landed, while the later one did.
    let mut read = store
        .read_session(InvocationGrant::full_store(), write())
        .expect("read session");
    let site = read.site(1);
    assert_eq!(
        read.read_field(&site, &[KeyScalar::Str("a".into())]),
        Ok(None),
        "the aborted write is not present"
    );
    assert_eq!(
        read.read_field(&site, &[KeyScalar::Str("b".into())]),
        Ok(Some(ValueDomain::Scalar(RuntimeScalar::Int(2)))),
        "the post-abort commit is present"
    );
}

/// Read the `value` field of entry `key` on a settled store, scoping the read session so
/// its borrow ends before the caller drives the store again.
fn read_value(store: &mut DurableStore<FaultEngine>, key: &str) -> Option<ValueDomain> {
    let mut read = store
        .read_session(InvocationGrant::full_store(), write())
        .expect("read session");
    let site = read.site(1);
    read.read_field(&site, &[KeyScalar::Str(key.into())])
        .expect("field read")
}

/// The witness put shares the staged data's transaction: if it fails before the engine
/// commit, the result is known-old `Aborted` and the transaction rolls back. Fail the third
/// write — after the created entry's marker and value leaf — then prove the handle stays
/// usable and the staged entry did not land.
#[test]
fn a_witness_put_failure_is_known_old_and_leaves_the_handle_usable() {
    let mode = ModeHandle::new(Mode::Confirm);
    let write_fault = WriteFaultHandle::inert();
    let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));

    let seeded = commit_one(&mut store, "a", 1);
    assert!(matches!(seeded, CommitResult::Committed));
    // Prior state is present on the healthy handle, before the poisoning fault.
    assert_eq!(
        read_value(&mut store, "a"),
        Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))
    );

    // In the next commit: write 1 = marker put, write 2 = value-leaf put, write 3 = the
    // witness put. Fail the witness put.
    write_fault.set(Some(3));
    let faulted = commit_one(&mut store, "b", 2);
    assert!(matches!(faulted, CommitResult::Aborted));

    // The witness put failed before the engine commit, so dropping the transaction proves
    // known-old and the handle remains usable.
    write_fault.set(None);
    assert!(matches!(
        commit_one(&mut store, "c", 3),
        CommitResult::Committed
    ));
    assert_eq!(read_value(&mut store, "b"), None);
}

/// Reconcile writes an absent marker for a markerless entry whose required fields are all
/// staged. If that marker put fails, the result is known-old `Aborted` and rolls back. Stage a required field
/// through `set_required` (which stages a leaf but no marker) and fail reconcile's marker
/// put — the second write of the commit, after the leaf — then confirm a later session can
/// commit and the partially staged entry is absent.
#[test]
fn a_reconcile_marker_put_failure_is_known_old_and_leaves_the_handle_usable() {
    let mode = ModeHandle::new(Mode::Confirm);
    let write_fault = WriteFaultHandle::inert();
    let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));

    let seeded = commit_one(&mut store, "a", 1);
    assert!(matches!(seeded, CommitResult::Committed));

    // Write 1 = the value-leaf put; write 2 = reconcile's marker put for the markerless
    // entry. Fail the marker put.
    write_fault.set(Some(2));
    let faulted = {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn session");
        let value = txn.site(1);
        txn.set_required(
            &value,
            &[KeyScalar::Str("b".into())],
            ValueDomain::Scalar(RuntimeScalar::Int(2)),
        )
        .expect("stage required field");
        txn.commit()
    };
    assert!(matches!(faulted, CommitResult::Aborted));

    // The reconcile write failed before commit, so dropping the transaction proves known-old
    // and a later session may proceed.
    write_fault.set(None);
    assert!(matches!(
        commit_one(&mut store, "c", 3),
        CommitResult::Committed
    ));
    assert_eq!(read_value(&mut store, "b"), None);
}

/// An `apply` put or remove that fails mid-plan (`store.rs` ~646-660) faults the durable
/// op with `KernelFault::Engine` and does not commit, so the still-live transaction aborts
/// on drop and the prior committed state is intact. Exercises both the `Put` arm (a
/// partly-applied create) and the `Remove` arm (a partly-applied erase); in each the
/// second write of the plan fails, leaving one cell already staged in the working copy
/// that the abort must discard.
#[test]
fn an_apply_write_fault_faults_and_the_store_stays_abortable() {
    // Put arm: create writes marker (write 1) then value leaf (write 2); fail the leaf.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let seeded = commit_one(&mut store, "a", 1);
        assert!(matches!(seeded, CommitResult::Committed));

        write_fault.set(Some(2));
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn session");
            let site = txn.site(0);
            let err = txn
                .create_entry(&site, &[KeyScalar::Str("b".into())], entry(2))
                .expect_err("the apply put fault surfaces");
            assert!(
                matches!(err, KernelFault::Engine(_)),
                "an engine write fault surfaces as KernelFault::Engine"
            );
            // Drop without commit: the transaction aborts, discarding the staged marker.
        }
        // The store took no permanent write: no poison (a later commit succeeds), the
        // prior entry is intact, and the partly-created entry never landed.
        write_fault.set(None);
        let next = commit_one(&mut store, "c", 3);
        assert!(
            matches!(next, CommitResult::Committed),
            "an aborted apply leaves the store usable"
        );
        assert_eq!(
            read_value(&mut store, "a"),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))
        );
        assert_eq!(read_value(&mut store, "b"), None);
        assert_eq!(
            read_value(&mut store, "c"),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(3)))
        );
    }

    // Remove arm: erase removes marker (write 1) then value leaf (write 2); fail the leaf
    // removal, so the marker is already removed in the working copy the abort discards.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let seeded = commit_one(&mut store, "a", 1);
        assert!(matches!(seeded, CommitResult::Committed));

        write_fault.set(Some(2));
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn session");
            let site = txn.site(0);
            let err = txn
                .erase_entry(&site, &[KeyScalar::Str("a".into())])
                .expect_err("the apply remove fault surfaces");
            assert!(
                matches!(err, KernelFault::Engine(_)),
                "an engine remove fault surfaces as KernelFault::Engine"
            );
            // Drop without commit: the abort restores the partly-removed entry.
        }
        assert_eq!(
            read_value(&mut store, "a"),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(1))),
            "the aborted erase left the prior entry intact"
        );
    }
}
