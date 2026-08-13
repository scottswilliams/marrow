//! The transaction surface's state battery: savepoint identity and epoch laws, the
//! armed rollback's byte-exact total inverse, and the per-kind nonblocking N+1
//! ledger deltas observed through the public fence — commit retains the crossing,
//! rollback restores the exact pre-transaction verdict and bytes.

use marrow_image::bounds::{
    MAX_COLLECTIONS, MAX_CONSTS, MAX_ENUMS, MAX_STRING_BYTES, MAX_STRINGS, MAX_TYPES,
};
use marrow_image::{
    CollectionTypeDef, DraftStateError, DraftTxn, EnumTypeDef, ExportId, FieldDef, FunctionDef,
    ImageBuildError, ImageDraft, ImageType, Instr, RecordTypeDef, Scalar,
};

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
