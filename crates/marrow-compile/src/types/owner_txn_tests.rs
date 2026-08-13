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
