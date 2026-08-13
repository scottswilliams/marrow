//! The generic-owner composite guard's custody battery: what one admitted batch
//! commits, what its inverse restores on every armed exit, and the enumerated owner
//! failure points the phase names.

use super::*;

use marrow_image::ImageDraft;

/// The generic-owner custody law: an admitted batch's registry effects are inverted
/// with its draft rows, so a rolled-back batch leaves the two owners in step.
///
/// Without the inverse the registry keeps the collection and instantiation rows the
/// draft rolled back, and the very next mint refuses as `CollectionIndexMismatch` —
/// a legitimate later compilation step turned into an invariant by an earlier
/// abandoned one.
#[test]
fn a_rolled_back_generic_batch_leaves_the_registry_in_step_with_its_draft() {
    let mut owner = ImageDraft::new();
    let mut records = registry(vec![template("Box", vec![("item", name("T"))])]);

    {
        let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
            .expect("a settled registry admits an ordinary batch");
        let (registry, draft) = batch.parts();
        registry
            .instantiate_list(draft, GArg::Scalar(ScalarType::Int))
            .expect("the first list instantiation mints");
        registry
            .mint_type_instance(draft, 0, &[GArg::Scalar(ScalarType::Int)], site(1))
            .expect("the first Box instantiation mints");
        assert_eq!(registry.collections.borrow().len(), 1);
        assert_eq!(registry.generics.borrow().type_insts.len(), 1);
    }

    assert!(records.collections.borrow().is_empty());
    assert!(records.collection_index.borrow().is_empty());
    assert!(records.generics.borrow().type_insts.is_empty());
    assert!(records.generics.borrow().type_index.is_empty());
    assert_eq!(owner.collection_type_count(), 0);
    assert_eq!(owner.record_type_count(), 0);

    let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
        .expect("the restored registry admits the next batch");
    let (registry, draft) = batch.parts();
    let id = registry
        .instantiate_list(draft, GArg::Scalar(ScalarType::Int))
        .expect("an in-step registry re-mints the same row");
    assert_eq!(id, coll(0));
    batch.commit();

    assert_eq!(records.collections.borrow().len(), 1);
    assert_eq!(owner.collection_type_count(), 1);
}

/// The committed arm of the same law: a committed batch keeps every registry row it
/// minted, and the draft keeps the rows they name.
#[test]
fn a_committed_generic_batch_retains_both_owners() {
    let mut owner = ImageDraft::new();
    let mut records = registry(vec![template("Box", vec![("item", name("T"))])]);

    let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
        .expect("a settled registry admits an ordinary batch");
    let (registry, draft) = batch.parts();
    registry
        .instantiate_list(draft, GArg::Scalar(ScalarType::Int))
        .expect("the list instantiation mints");
    registry
        .mint_type_instance(draft, 0, &[GArg::Scalar(ScalarType::Int)], site(1))
        .expect("the Box instantiation mints");
    batch.commit();

    assert_eq!(records.collections.borrow().len(), 1);
    assert_eq!(records.collection_index.borrow().len(), 1);
    assert_eq!(records.generics.borrow().type_insts.len(), 1);
    assert_eq!(owner.collection_type_count(), 1);
    assert_eq!(owner.record_type_count(), 1);
}

/// An unwind through an armed batch restores both owners exactly, and the registry
/// admits and serves the next batch afterwards.
#[test]
fn an_unwind_through_an_armed_generic_batch_restores_both_owners() {
    let mut owner = ImageDraft::new();
    let mut records = registry(vec![template("Box", vec![("item", name("T"))])]);

    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
            .expect("a settled registry admits an ordinary batch");
        let (registry, draft) = batch.parts();
        registry
            .instantiate_list(draft, GArg::Scalar(ScalarType::Int))
            .expect("the list instantiation mints");
        panic!("the body raises after mutating every owner");
    }));
    assert!(unwound.is_err());

    assert!(records.collections.borrow().is_empty());
    assert!(records.generics.borrow().type_insts.is_empty());
    assert_eq!(owner.collection_type_count(), 0);

    let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
        .expect("the restored registry admits the next batch");
    let (registry, draft) = batch.parts();
    assert_eq!(
        registry
            .instantiate_list(draft, GArg::Scalar(ScalarType::Int))
            .expect("an in-step registry re-mints the same row"),
        coll(0)
    );
    batch.commit();
}

/// The three-arm template-proof preservation law over nonempty legacy function owners.
///
/// A proof runs directly on the live registry and draft, and until `IMGFUNC01` takes
/// authority the composite guard also carries preservation-only coverage of the legacy
/// `fn_insts`, `fn_index`, and `fn_queue` owners, the recorded build fault, and the
/// throwaway image bytes the proof emits. All three ways out of a proof — the ordinary
/// scope exit, an early return carrying a lowering error, and an unwind — restore every
/// one of those owners to exactly the state the proof was admitted over.
///
/// Prepopulating the function owners is the point: an inverse that restored only the
/// type and collection owners would pass an empty-`fn_*` fixture and lose a reserved
/// image function index here.
#[test]
fn a_template_proof_preserves_prepopulated_function_owners_on_exit_error_and_unwind() {
    /// How the proof body leaves its scope.
    #[derive(Clone, Copy, Debug)]
    enum ProofExit {
        Ordinary,
        Error,
        Unwind,
    }

    for exit in [ProofExit::Ordinary, ProofExit::Error, ProofExit::Unwind] {
        let mut records = registry(vec![
            template("Leaf", vec![("value", name("T"))]),
            enum_template("Choice", apply("Leaf", vec![name("T")])),
        ]);
        let mut owner = ImageDraft::new();
        let scalar = GArg::Scalar(ScalarType::Int);
        {
            let mut seed = admitted(&mut owner);
            records
                .mint_type_instance(&mut seed, 0, &[scalar], site(2))
                .expect("the settled seed row mints");
            records
                .instantiate_list(&mut seed, scalar)
                .expect("the settled seed collection mints");
            seed.commit();
        }
        // The legacy function owners the proof must not disturb: a reservation that
        // occupies a real image function index, its lockstep key, and its queue entry.
        records.set_fn_base(37);
        assert_eq!(
            records
                .reserve_fn_instance(7, vec![scalar], site(5))
                .expect("the stable function row reserves"),
            37,
        );
        let before = stable_snapshot(&records);
        assert_eq!(
            before.functions.len(),
            1,
            "the fixture seeded a function row"
        );
        assert_eq!(before.queue.len(), 1, "the fixture seeded a queue entry");
        assert!(!before.fn_index.is_empty(), "the fixture seeded its key");
        let draft_before = draft_snapshot(&owner);

        /// Everything a proof pass appends: isolated instantiations against the
        /// abstract domain plus a throwaway image function.
        fn prove(scope: &mut GenericOwnerTxn<'_, '_>, scalar: GArg) {
            let (registry, txn) = scope.parts();
            let text = GArg::Scalar(ScalarType::Text);
            registry
                .mint_type_instance(txn, 0, &[text], site(28))
                .expect("the proof mints its own isolated row");
            registry
                .instantiate_list(txn, text)
                .expect("the proof mints its own isolated collection");
            registry
                .reserve_fn_instance(9, vec![scalar], site(29))
                .expect("the proof reserves its own throwaway function row");
            let name = txn
                .intern_string("throwaway")
                .expect("a within-domain mint");
            txn.add_record_type(marrow_image::RecordTypeDef {
                name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        }

        match exit {
            ProofExit::Ordinary => {
                let mut scope = GenericOwnerTxn::enter_proof(&mut records, &mut owner)
                    .expect("a settled registry admits the proof");
                prove(&mut scope, scalar);
                drop(scope);
            }
            ProofExit::Error => {
                // The early-return arm: the proof body carries a lowering error out
                // through `?`, so the scope drops on the error path rather than at the
                // end of a block.
                fn proving(
                    records: &mut TypeRegistry,
                    owner: &mut ImageDraft,
                    scalar: GArg,
                ) -> Result<(), GenericInvariant> {
                    let mut scope = GenericOwnerTxn::enter_proof(records, owner)?;
                    prove(&mut scope, scalar);
                    Err(GenericInvariant::TypeTemplateMissing(99))
                }
                assert_eq!(
                    proving(&mut records, &mut owner, scalar),
                    Err(GenericInvariant::TypeTemplateMissing(99)),
                );
            }
            ProofExit::Unwind => {
                let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut scope = GenericOwnerTxn::enter_proof(&mut records, &mut owner)
                        .expect("a settled registry admits the proof");
                    prove(&mut scope, scalar);
                    panic!("the proof body raises after appending to every owner");
                }));
                assert!(unwound.is_err(), "the panic reached the catch");
            }
        }

        assert_eq!(
            stable_snapshot(&records),
            before,
            "{exit:?}: the proof left every registry owner exactly as it found it",
        );
        assert_eq!(
            draft_snapshot(&owner),
            draft_before,
            "{exit:?}: the throwaway image bytes were restored exactly",
        );
    }
}

/// The enumerated generic-owner failure points, one per owner the phase names: a batch
/// that reaches the owner is aborted at that point and every owner — not only the one
/// touched — comes back to the state admission captured.
///
/// Each case asserts the owner really did change inside the batch before the abort, so
/// no case can pass by never reaching its owner. The transient fill owners and the
/// recorded build fault are planted directly: production leaves them dirty only when a
/// fill fails partway, and planting that state is how the inverse is proved total over
/// it rather than only over the settled shapes a clean batch produces.
#[test]
fn each_enumerated_generic_owner_failure_point_restores_every_owner() {
    struct Failure {
        owner: &'static str,
        /// Reach the owner inside the armed batch. Returns nothing: the snapshot
        /// comparison below is the whole assertion.
        touch: fn(&mut GenericOwnerTxn<'_, '_>),
    }
    let failures = [
        Failure {
            owner: "draft rows, instantiation vector, and lookup index",
            touch: |batch| {
                let (registry, draft) = batch.parts();
                registry
                    .mint_type_instance(draft, 0, &[GArg::Scalar(ScalarType::Text)], site(11))
                    .expect("the batch mints a fresh instantiation");
            },
        },
        Failure {
            owner: "collection vector and collection index",
            touch: |batch| {
                let (registry, draft) = batch.parts();
                registry
                    .instantiate_list(draft, GArg::Scalar(ScalarType::Text))
                    .expect("the batch mints a fresh collection");
            },
        },
        Failure {
            owner: "function rows, function index, and reservation queue",
            touch: |batch| {
                let (registry, _) = batch.parts();
                registry
                    .reserve_fn_instance(9, vec![GArg::Scalar(ScalarType::Text)], site(12))
                    .expect("the batch reserves a fresh function row");
            },
        },
        Failure {
            owner: "transient fill state and existing-row dependency edges",
            touch: |batch| {
                let (registry, draft) = batch.parts();
                registry
                    .mint_type_instance(draft, 0, &[GArg::Scalar(ScalarType::Text)], site(13))
                    .expect("the batch mints the row the dirty edges name");
                let mut generics = registry.generics.borrow_mut();
                let dirty = generics.type_insts.len() - 1;
                let key = TypeInstKey::from(generics.type_insts[dirty].id);
                generics.fill_batch_start = Some(dirty);
                generics.fill_rows.insert(key, dirty);
                generics.fill_stack.push(dirty);
                generics
                    .fill_failures
                    .push((dirty, ResolveRefusal::Unsupported));
                generics.type_insts[dirty].dependents.push(dirty);
            },
        },
        Failure {
            owner: "the recorded build fault",
            touch: |batch| {
                let (registry, _) = batch.parts();
                registry.generics.borrow_mut().build_invariant =
                    Some(GenericInvariant::TypeTemplateMissing(99));
            },
        },
        Failure {
            owner: "the argument domain",
            touch: |batch| {
                let (registry, _) = batch.parts();
                registry.generics.borrow_mut().argument_domain = ArgumentDomain::TemplateProof;
            },
        },
    ];

    for failure in failures {
        let mut records = registry(vec![template("Leaf", vec![("value", name("T"))])]);
        let mut owner = ImageDraft::new();
        let scalar = GArg::Scalar(ScalarType::Int);
        {
            let mut seed = admitted(&mut owner);
            records
                .mint_type_instance(&mut seed, 0, &[scalar], site(2))
                .expect("the settled seed row mints");
            records
                .instantiate_list(&mut seed, scalar)
                .expect("the settled seed collection mints");
            seed.commit();
        }
        records.set_fn_base(37);
        records
            .reserve_fn_instance(7, vec![scalar], site(5))
            .expect("the settled seed function row reserves");
        let before = stable_snapshot(&records);
        let draft_before = draft_snapshot(&owner);

        {
            let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
                .expect("a settled registry admits an ordinary batch");
            (failure.touch)(&mut batch);
            assert_ne!(
                stable_snapshot(batch.registry()),
                before,
                "{}: the case reached its owner before the abort",
                failure.owner,
            );
            // The armed guard drops here: the batch is abandoned, not committed.
        }

        assert_eq!(
            stable_snapshot(&records),
            before,
            "{}: every registry owner returned to its admitted state",
            failure.owner,
        );
        assert_eq!(
            draft_snapshot(&owner),
            draft_before,
            "{}: the draft returned to its admitted bytes",
            failure.owner,
        );
    }
}

/// The three registry owners an abandoned batch previously kept: the reserved image
/// function base, the lazily built metadata row directory, and the reservation queue's
/// front entry.
///
/// Each is asserted as an artifact — the exact base, the directory's presence, the exact
/// queue contents — and each case first shows the owner changing inside the batch, so no
/// arm can pass by never reaching its owner.
#[test]
fn an_abandoned_batch_restores_the_function_base_the_metadata_cache_and_the_queue_front() {
    let scalar = GArg::Scalar(ScalarType::Int);

    // The reserved function base is ordinary registry state, so a batch that moves it and
    // is abandoned leaves the base admission captured.
    {
        let mut records = registry(vec![template("Leaf", vec![("value", name("T"))])]);
        let mut owner = ImageDraft::new();
        records.set_fn_base(37);
        {
            let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
                .expect("a settled registry admits an ordinary batch");
            let (registry, _) = batch.parts();
            registry.set_fn_base(99);
            assert_eq!(
                registry.generics.borrow().fn_base,
                99,
                "the case reached the function base before the abort",
            );
        }
        assert_eq!(records.generics.borrow().fn_base, 37);
    }

    // The metadata row directory is a cache the registry may not hold at all. Rewinding
    // an extant directory is not the inverse of building the first one, so a batch that
    // opens the first session must leave the registry holding none.
    {
        let mut records = registry(vec![template("Leaf", vec![("value", name("T"))])]);
        let mut owner = ImageDraft::new();
        assert!(
            records.row_directory.borrow().is_none(),
            "a fresh registry holds no directory",
        );
        {
            let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
                .expect("a settled registry admits an ordinary batch");
            let (registry, _) = batch.parts();
            registry
                .with_metadata_session(|_| Ok::<(), GenericInvariant>(()))
                .expect("the batch opens a metadata session");
            assert!(
                registry.row_directory.borrow().is_some(),
                "the case built the directory before the abort",
            );
        }
        assert!(
            records.row_directory.borrow().is_none(),
            "an abandoned batch leaves the registry the directory it had: none",
        );
    }

    // The reservation queue: a batch's appends are undone and the entry the drain driver
    // is working on is still at the front, because the driver reads it rather than
    // removing it.
    {
        let mut records = registry(vec![template("Leaf", vec![("value", name("T"))])]);
        let mut owner = ImageDraft::new();
        records
            .reserve_fn_instance(7, vec![scalar], site(5))
            .expect("the settled seed function row reserves");
        let front = records.peek_fn_pending().expect("the seed entry is queued");
        assert_eq!(front, (7, vec![scalar], 0));
        let queued_before: Vec<_> = records
            .generics
            .borrow()
            .fn_queue
            .iter()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
            .collect();
        assert_eq!(queued_before.len(), 1);
        {
            let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
                .expect("a settled registry admits an ordinary batch");
            let (registry, _) = batch.parts();
            registry
                .reserve_fn_instance(9, vec![GArg::Scalar(ScalarType::Text)], site(12))
                .expect("the batch reserves a further instance");
            assert_eq!(
                registry.generics.borrow().fn_queue.len(),
                2,
                "the case appended to the queue before the abort",
            );
        }
        let queued_after: Vec<_> = records
            .generics
            .borrow()
            .fn_queue
            .iter()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
            .collect();
        assert_eq!(
            queued_after, queued_before,
            "the abandoned batch's appends are gone and the front entry is intact",
        );
        assert_eq!(records.peek_fn_pending(), Some(front));
    }
}

/// The two owners the inverse deliberately does not restore, pinned as a decision rather
/// than left as an omission.
///
/// `limit` and `collection_payloads` are diagnostic payload. The phase places diagnostics
/// exclusively in the predecessor substrate's custody — the draft guard never owns,
/// copies, journals, or exposes them — and this inverse mirrors that boundary rather than
/// opening a second custody over the same rows. What decides whether a batch's
/// diagnostics become visible is the settlement capability a committed or rolled-back
/// guard produces, not a registry rollback.
#[test]
fn an_abandoned_batch_leaves_the_diagnostic_owners_to_their_own_custody() {
    assert_eq!(
        super::owner_txn::UNRESTORED_DIAGNOSTIC_OWNERS,
        ["limit", "collection_payloads"],
    );

    let mut records = registry(vec![template("Leaf", vec![("value", name("T"))])]);
    let mut owner = ImageDraft::new();
    assert!(matches!(records.generics.borrow().limit, LimitState::Open));
    {
        let mut batch = GenericOwnerTxn::begin(&mut records, &mut owner)
            .expect("a settled registry admits an ordinary batch");
        let (registry, _) = batch.parts();
        registry.record_limit(site(3), "the pinned subject");
        assert!(matches!(
            registry.generics.borrow().limit,
            LimitState::Pending(_)
        ));
    }
    assert!(
        matches!(records.generics.borrow().limit, LimitState::Pending(_)),
        "the recorded limit stays with the diagnostic substrate across an abandoned batch",
    );
}
