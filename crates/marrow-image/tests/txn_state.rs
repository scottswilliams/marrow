//! The transaction surface's state battery: savepoint identity and epoch laws, the
//! armed rollback's byte-exact total inverse, and the per-kind nonblocking N+1
//! ledger deltas observed through the public fence — commit retains the crossing,
//! rollback restores the exact pre-transaction verdict and bytes.

use marrow_image::bounds::{
    MAX_COLLECTIONS, MAX_CONSTS, MAX_ENUMS, MAX_ROOTS, MAX_STRING_BYTES, MAX_STRINGS, MAX_TYPES,
};
use marrow_image::{
    CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, DraftStateError, DraftTxn,
    EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr,
    LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

/// The armed transaction a fresh savepoint admits over `owner`.
fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner
        .begin_transaction(owner.savepoint())
        .expect("a fresh savepoint admits")
}

/// A committed one-function draft that encodes, for rollback byte-identity checks.
fn exporting_owner() -> ImageDraft {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let name = draft.intern_string("main");
    let source = draft.intern_string("src/main.mw");
    draft.intern_int(0);
    let main = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code: vec![Instr::Return],
            spans: Vec::new(),
        })
        .expect("no site operand needs validating");
    draft.add_export(ExportId::of_local("m", "main"), main);
    draft.commit();
    owner
}

// ---- The savepoint battery.

/// A savepoint minted by one draft is a foreign token to another — even when the two
/// drafts are byte-identical, their allocation identities differ.
#[test]
fn a_foreign_savepoint_is_refused_before_any_mutation() {
    let first = ImageDraft::new();
    let mut second = ImageDraft::new();
    let foreign = first.savepoint();
    assert_eq!(
        second.begin_transaction(foreign).err(),
        Some(DraftStateError::ForeignDraft),
    );
    // The refusal rotated nothing: the second draft's own savepoint still admits.
    admitted(&mut second).commit();
}

/// Admitting one of two sibling savepoints rotates the one-shot epoch and stales the
/// other, and the staled sibling stays stale after the admitted transaction commits.
#[test]
fn admitting_one_sibling_savepoint_stales_the_other() {
    let mut owner = ImageDraft::new();
    let first = owner.savepoint();
    let second = owner.savepoint();
    let txn = owner
        .begin_transaction(first)
        .expect("the first sibling admits");
    txn.commit();
    assert_eq!(
        owner.begin_transaction(second).err(),
        Some(DraftStateError::StaleEpoch),
    );
}

/// A savepoint from before a rolled-back transaction is stale even though every
/// logical draft byte again equals the state it observed: the consumed epoch is
/// monotone authentication state outside the logical inverse.
#[test]
fn a_savepoint_stays_stale_after_a_rollback_restores_its_bytes() {
    let mut owner = exporting_owner();
    let before = owner.savepoint();
    {
        let mut txn = admitted(&mut owner);
        txn.intern_string("discarded");
        // The armed guard drops here: the logical state is restored byte for byte.
    }
    assert_eq!(
        owner.begin_transaction(before).err(),
        Some(DraftStateError::StaleEpoch),
    );
}

/// A savepoint outliving its dropped draft keeps its token allocations alive and
/// still fails allocation-identity validation against a fresh draft, so ordinary
/// allocator reuse cannot forge an admission (the ABA case).
#[test]
fn a_savepoint_outliving_its_draft_cannot_admit_a_successor() {
    let orphan = {
        let doomed = ImageDraft::new();
        doomed.savepoint()
    };
    // Forced allocator-reuse pressure: many byte-identical drafts are allocated and
    // dropped, inviting the freed draft's addresses back into circulation. The orphan
    // strongly retains its own token allocations, so no successor can be handed them.
    for _ in 0..1024 {
        drop(ImageDraft::new());
    }
    let mut successor = ImageDraft::new();
    assert_eq!(
        successor.begin_transaction(orphan).err(),
        Some(DraftStateError::ForeignDraft),
    );
}

// ---- The armed inverse is byte-exact across every owner.

/// A rolled-back transaction leaves the draft encoding to the exact bytes it held at
/// admission, across interned strings and constants, reserved-and-filled types,
/// enums, collections, functions, exports, and test entries.
#[test]
fn a_rolled_back_transaction_restores_the_exact_bytes() {
    let mut owner = exporting_owner();
    let before = owner.encode().expect("the base draft encodes").bytes;
    {
        let mut txn = admitted(&mut owner);
        let name = txn.intern_string("Extra");
        let field = txn.intern_string("f");
        txn.intern_text("extra-text");
        let record = txn.add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        });
        txn.set_record_fields(
            record,
            vec![FieldDef {
                name: field,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        )
        .expect("the reserved row fills once");
        txn.add_enum_type(EnumTypeDef {
            name,
            variants: Vec::new(),
        });
        txn.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        });
        let int = txn.value_scalar(Scalar::Int);
        txn.value_struct(vec![int, int]);
    }
    let after = owner.encode().expect("the restored draft encodes").bytes;
    assert_eq!(before, after, "the armed inverse is byte-exact");
    // The interning indexes were restored with the pool: the same text re-mints the
    // same ordinal, so a stale index entry cannot alias a discarded row.
    let mut txn = admitted(&mut owner);
    let re_minted = txn.intern_text("extra-text");
    let twin = txn.intern_text("extra-text");
    assert_eq!(re_minted, twin);
}

/// A fill of a pre-transaction reserved row is journaled and reverted: after the
/// rollback the row holds its prior definition and spends its one fill again.
#[test]
fn a_rolled_back_fill_of_a_pre_transaction_row_is_reverted() {
    let mut owner = ImageDraft::new();
    let mut txn = admitted(&mut owner);
    let name = txn.intern_string("R");
    let field = txn.intern_string("f");
    let record = txn.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    txn.commit();
    let before = owner.encode().expect("the reserved draft encodes").bytes;
    {
        let mut txn = admitted(&mut owner);
        txn.set_record_fields(
            record,
            vec![FieldDef {
                name: field,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        )
        .expect("the reserved row fills once");
    }
    let after = owner.encode().expect("the restored draft encodes").bytes;
    assert_eq!(before, after, "the displaced definition moved back");
    let mut txn = admitted(&mut owner);
    txn.set_record_fields(record, Vec::new())
        .expect("the reverted row spends its one fill again");
    txn.commit();
}

// ---- Per-kind nonblocking N+1 ledger deltas, observed through the public fence.

/// Each active kind's N/N+1 law: the N+1 mutation is admitted (never refused at the
/// surface), the fence refuses the provisional image with exactly that kind's
/// verdict (the ledger's canonical minimum shadow-compared against the walk), commit
/// retains the crossing, and rollback restores the exact pre-transaction verdict.
#[test]
fn each_active_kind_admits_its_crossing_and_the_fence_refuses_it() {
    struct Kind {
        cross: fn(&mut DraftTxn<'_>),
        verdict: ImageBuildError,
    }
    let kinds = [
        Kind {
            cross: |txn| {
                for index in 0..=MAX_STRINGS {
                    txn.intern_string(&format!("s{index:05}"));
                }
            },
            verdict: ImageBuildError::TooManyStrings,
        },
        Kind {
            cross: |txn| {
                txn.intern_string(&"x".repeat(MAX_STRING_BYTES + 1));
            },
            verdict: ImageBuildError::StringTooLong,
        },
        Kind {
            cross: |txn| {
                for value in 0..=(MAX_CONSTS as i64) {
                    txn.intern_int(value);
                }
            },
            verdict: ImageBuildError::TooManyConsts,
        },
        Kind {
            cross: |txn| {
                let name = txn.intern_string("T");
                for _ in 0..=MAX_TYPES {
                    txn.add_record_type(RecordTypeDef {
                        name,
                        fields: Vec::new(),
                    });
                }
            },
            verdict: ImageBuildError::TooManyTypes,
        },
        Kind {
            cross: |txn| {
                let name = txn.intern_string("E");
                for _ in 0..=MAX_ENUMS {
                    txn.add_enum_type(EnumTypeDef {
                        name,
                        variants: Vec::new(),
                    });
                }
            },
            verdict: ImageBuildError::TooManyEnums,
        },
        Kind {
            cross: |txn| {
                for _ in 0..=MAX_COLLECTIONS {
                    txn.add_collection_type(CollectionTypeDef::List {
                        elem: ImageType::scalar(Scalar::Int),
                    });
                }
            },
            verdict: ImageBuildError::TooManyCollections,
        },
    ];
    for kind in kinds {
        // Rollback: the crossing and its ledger delta are restored exactly.
        let mut owner = exporting_owner();
        let clean = owner.encode().expect("the base draft encodes").bytes;
        {
            let mut txn = admitted(&mut owner);
            (kind.cross)(&mut txn);
            assert_eq!(
                txn.encode().map(|_| ()),
                Err(kind.verdict.clone()),
                "the provisional N+1 state is refused only at the fence",
            );
        }
        assert_eq!(
            owner.encode().expect("the restored draft encodes").bytes,
            clean,
            "rollback restored the rows and the ledger byte for byte",
        );
        // Commit: the crossing is retained and the fence still refuses.
        let mut txn = admitted(&mut owner);
        (kind.cross)(&mut txn);
        txn.commit();
        assert_eq!(owner.encode().map(|_| ()), Err(kind.verdict));
    }
}

/// The Roots kind's N/N+1 law: the N+1 root occurrence is admitted, the fence refuses
/// with exactly `TooManyRoots` (never ledger drift), rollback restores the exact
/// pre-transaction verdict and bytes, and commit retains the crossing.
#[test]
fn the_roots_crossing_is_admitted_and_the_fence_refuses_it() {
    let product = LedgerIdBytes::from_bytes([0x0d; 16]);
    let mut owner = ImageDraft::new();
    let mut txn = admitted(&mut owner);
    let name = txn.intern_string("R");
    let record = txn.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    txn.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    let value = txn.value_scalar(Scalar::Int);
    txn.declare_product(
        &admitted_plan(),
        product,
        record,
        vec![DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: LedgerIdBytes::from_bytes([0x50; 16]),
                required: true,
                value,
            },
        }],
    )
    .expect("a well-formed declaration");
    let mut roots = 0u32;
    let mut admit_one = |txn: &mut DraftTxn<'_>| {
        let name = txn.intern_string(&format!("r{roots:05}"));
        let mut placement = [0x60u8; 16];
        placement[0] = (roots & 0xff) as u8;
        placement[1] = ((roots >> 8) & 0xff) as u8;
        roots += 1;
        txn.add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name,
                keys: Vec::new(),
                placement: LedgerIdBytes::from_bytes(placement),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    };
    for _ in 0..MAX_ROOTS {
        admit_one(&mut txn);
    }
    txn.commit();
    let clean = owner.encode().expect("exactly MAX_ROOTS fits").bytes;

    // Rollback: the crossing and its ledger delta are restored exactly.
    {
        let mut txn = admitted(&mut owner);
        admit_one(&mut txn);
        assert_eq!(
            txn.encode().map(|_| ()),
            Err(ImageBuildError::TooManyRoots),
            "the provisional N+1 root is refused only at the fence",
        );
    }
    assert_eq!(
        owner.encode().expect("the restored draft encodes").bytes,
        clean,
        "rollback restored the rows and the ledger byte for byte",
    );

    // Commit: the crossing is retained and the fence still refuses.
    let mut txn = admitted(&mut owner);
    admit_one(&mut txn);
    txn.commit();
    assert_eq!(
        owner.encode().map(|_| ()),
        Err(ImageBuildError::TooManyRoots)
    );
}

/// The `intern_text` compound law at `MAX_CONSTS`: the new text commits its string, its
/// N+1 constant, and the Consts candidate as one delta; a later Strings crossing adds
/// that earlier-ranked candidate without erasing Consts — the fence's shadow-compare
/// would report drift, not a verdict, if either slot were lost — and rollback restores
/// both tables, both indexes, and the exact prior ledger.
#[test]
fn an_intern_text_at_the_const_cap_commits_its_whole_compound() {
    let mut owner = exporting_owner();
    let clean = owner.encode().expect("the base draft encodes").bytes;
    {
        let mut txn = admitted(&mut owner);
        for value in 0..(MAX_CONSTS as i64) {
            txn.intern_int(value);
        }
        txn.intern_text("the crossing text");
        assert_eq!(
            txn.encode().map(|_| ()),
            Err(ImageBuildError::TooManyConsts),
            "the compound committed the N+1 constant and the Consts candidate",
        );
        // The compound's string half committed: re-interning is a hit, not a growth.
        let first = txn.intern_string("the crossing text");
        let again = txn.intern_string("the crossing text");
        assert_eq!(first, again, "the compound's string row is retained");
        // A later Strings crossing outranks Consts without erasing it.
        for index in 0..=MAX_STRINGS {
            txn.intern_string(&format!("s{index:05}"));
        }
        assert_eq!(
            txn.encode().map(|_| ()),
            Err(ImageBuildError::TooManyStrings),
            "the earlier-ranked Strings candidate is added and Consts is retained",
        );
    }
    assert_eq!(
        owner.encode().expect("the restored draft encodes").bytes,
        clean,
        "rollback restored both tables, both indexes, and the exact prior ledger",
    );
}
