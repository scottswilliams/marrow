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
    LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar, VariantDef,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

/// A committed one-function draft that encodes, for rollback byte-identity checks.
fn exporting_owner() -> ImageDraft {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let name = draft.intern_string("main").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    draft.intern_int(0).expect("a within-domain mint");
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
        txn.intern_string("discarded")
            .expect("a within-domain mint");
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
        let name = txn.intern_string("Extra").expect("a within-domain mint");
        let field = txn.intern_string("f").expect("a within-domain mint");
        txn.intern_text("extra-text").expect("a within-domain mint");
        let record = txn.reserve_record_type(name).expect("a within-domain mint");
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
        })
        .expect("a within-domain mint");
        txn.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .expect("a within-domain mint");
        let int = txn.value_scalar(Scalar::Int);
        txn.value_struct(vec![int, int])
            .expect("a within-bounds shape appends");
    }
    let after = owner.encode().expect("the restored draft encodes").bytes;
    assert_eq!(before, after, "the armed inverse is byte-exact");
    // The interning indexes were restored with the pool: the same text re-mints the
    // same ordinal, so a stale index entry cannot alias a discarded row.
    let mut txn = admitted(&mut owner);
    let re_minted = txn.intern_text("extra-text").expect("a within-domain mint");
    let twin = txn.intern_text("extra-text").expect("a within-domain mint");
    assert_eq!(re_minted, twin);
}

/// A fill of a pre-transaction reserved row is journaled and reverted: after the
/// rollback the row is `Vacant` again — the fence-distinct reservation state — and
/// spends its one fill again.
#[test]
fn a_rolled_back_fill_of_a_pre_transaction_row_is_reverted() {
    let mut owner = ImageDraft::new();
    let mut txn = admitted(&mut owner);
    let name = txn.intern_string("R").expect("a within-domain mint");
    let field = txn.intern_string("f").expect("a within-domain mint");
    let record = txn.reserve_record_type(name).expect("a within-domain mint");
    txn.commit();
    assert_eq!(
        owner.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("vacant record type")),
        "a reservation left vacant is the fence's coherence invariant",
    );
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
        assert!(txn.encode().is_ok(), "the filled draft encodes");
    }
    assert_eq!(
        owner.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("vacant record type")),
        "the rollback reverted the fill, so the row is vacant again",
    );
    let mut txn = admitted(&mut owner);
    txn.set_record_fields(record, Vec::new())
        .expect("the reverted row spends its one fill again");
    txn.commit();
    assert!(
        owner.encode().is_ok(),
        "the explicit empty fill is a valid filled-empty definition, distinct from vacant",
    );
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
                    txn.intern_string(&format!("s{index:05}"))
                        .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyStrings,
        },
        Kind {
            cross: |txn| {
                txn.intern_string(&"x".repeat(MAX_STRING_BYTES + 1))
                    .expect("a within-domain mint");
            },
            verdict: ImageBuildError::StringTooLong,
        },
        Kind {
            cross: |txn| {
                for value in 0..=(MAX_CONSTS as i64) {
                    txn.intern_int(value).expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyConsts,
        },
        Kind {
            cross: |txn| {
                let name = txn.intern_string("T").expect("a within-domain mint");
                for _ in 0..=MAX_TYPES {
                    txn.add_record_type(RecordTypeDef {
                        name,
                        fields: Vec::new(),
                    })
                    .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyTypes,
        },
        Kind {
            cross: |txn| {
                let name = txn.intern_string("E").expect("a within-domain mint");
                for _ in 0..=MAX_ENUMS {
                    txn.add_enum_type(EnumTypeDef {
                        name,
                        variants: Vec::new(),
                    })
                    .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyEnums,
        },
        Kind {
            cross: |txn| {
                for _ in 0..=MAX_COLLECTIONS {
                    txn.add_collection_type(CollectionTypeDef::List {
                        elem: ImageType::scalar(Scalar::Int),
                    })
                    .expect("a within-domain mint");
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
    let name = txn.intern_string("R").expect("a within-domain mint");
    let record = txn
        .add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
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
        let name = txn
            .intern_string(&format!("r{roots:05}"))
            .expect("a within-domain mint");
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
            txn.intern_int(value).expect("a within-domain mint");
        }
        txn.intern_text("the crossing text")
            .expect("a within-domain mint");
        assert_eq!(
            txn.encode().map(|_| ()),
            Err(ImageBuildError::TooManyConsts),
            "the compound committed the N+1 constant and the Consts candidate",
        );
        // The compound's string half committed: re-interning is a hit, not a growth.
        let first = txn
            .intern_string("the crossing text")
            .expect("a within-domain mint");
        let again = txn
            .intern_string("the crossing text")
            .expect("a within-domain mint");
        assert_eq!(first, again, "the compound's string row is retained");
        // A later Strings crossing outranks Consts without erasing it.
        for index in 0..=MAX_STRINGS {
            txn.intern_string(&format!("s{index:05}"))
                .expect("a within-domain mint");
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

/// A complete definition never spends a fill: a record or enum added with its
/// definition is not a reservation, so a later "fill" is the typed double-fill
/// refusal, never a replacement. Only a reserved row admits exactly one fill, and a
/// reserved row left vacant is the fence's coherence invariant while an explicit
/// empty fill is a valid filled-empty definition.
#[test]
fn a_complete_definition_is_not_replaceable_and_vacancy_is_fence_distinct() {
    let mut owner = ImageDraft::new();
    let mut txn = admitted(&mut owner);
    let name = txn.intern_string("R").expect("a within-domain mint");
    let complete = txn
        .add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        })
        .expect("a within-domain mint");
    assert_eq!(
        txn.set_record_fields(complete, Vec::new()),
        Err(DraftStateError::IncoherentToken),
        "a complete definition is not a reservation and admits no fill",
    );
    let enum_name = txn.intern_string("E").expect("a within-domain mint");
    let complete_enum = txn
        .add_enum_type(EnumTypeDef {
            name: enum_name,
            variants: vec![VariantDef {
                name: enum_name,
                category: false,
                payload: Vec::new(),
            }],
        })
        .expect("a within-domain mint");
    assert_eq!(
        txn.set_enum_variants(complete_enum, Vec::new()),
        Err(DraftStateError::IncoherentToken),
        "a complete enum definition is not a reservation and admits no fill",
    );
}

/// Each counted kind's exact-N arm, the law the N+1 arm above cannot state on its own:
/// exactly `MAX_<kind>` rows are accepted at the fence, and the N-th row really is in
/// the artifact — one more row of the same kind flips the fence to that kind's verdict,
/// so the boundary sits exactly at N and no accepted row was quietly dropped.
///
/// Where the kind has a keyed lookup, the N-th row also stays available through it: a
/// re-request at the maximum is served from the existing row rather than minting, which
/// the still-clean fence proves (a mint would have crossed).
///
/// The seeded counts below are self-checking. If the fixture ever seeds a different
/// number of rows the exact-N encode or the N+1 refusal fails, so neither figure can
/// drift silently.
#[test]
fn each_counted_kind_admits_exactly_its_maximum_and_stays_available() {
    struct Kind {
        /// Rows of this kind the exporting fixture already holds.
        seeded: usize,
        maximum: usize,
        mint: fn(&mut DraftTxn<'_>, usize),
        /// Re-request row `index` through the kind's keyed lookup, where it has one.
        lookup: Option<fn(&mut DraftTxn<'_>, usize)>,
        verdict: ImageBuildError,
    }
    let kinds = [
        Kind {
            // "main" and "src/main.mw".
            seeded: 2,
            maximum: MAX_STRINGS,
            mint: |txn, index| {
                txn.intern_string(&format!("s{index:05}"))
                    .expect("a within-domain mint");
            },
            lookup: Some(|txn, index| {
                txn.intern_string(&format!("s{index:05}"))
                    .expect("a within-domain mint");
            }),
            verdict: ImageBuildError::TooManyStrings,
        },
        Kind {
            // The interned `0`.
            seeded: 1,
            maximum: MAX_CONSTS,
            mint: |txn, index| {
                txn.intern_int(index as i64 + 1)
                    .expect("a within-domain mint");
            },
            lookup: Some(|txn, index| {
                txn.intern_int(index as i64 + 1)
                    .expect("a within-domain mint");
            }),
            verdict: ImageBuildError::TooManyConsts,
        },
        Kind {
            seeded: 0,
            maximum: MAX_TYPES,
            mint: |txn, _| {
                let name = txn.intern_string("T").expect("a within-domain mint");
                txn.add_record_type(RecordTypeDef {
                    name,
                    fields: Vec::new(),
                })
                .expect("a within-domain mint");
            },
            lookup: None,
            verdict: ImageBuildError::TooManyTypes,
        },
        Kind {
            seeded: 0,
            maximum: MAX_ENUMS,
            mint: |txn, _| {
                let name = txn.intern_string("E").expect("a within-domain mint");
                txn.add_enum_type(EnumTypeDef {
                    name,
                    variants: Vec::new(),
                })
                .expect("a within-domain mint");
            },
            lookup: None,
            verdict: ImageBuildError::TooManyEnums,
        },
        Kind {
            seeded: 0,
            maximum: MAX_COLLECTIONS,
            mint: |txn, _| {
                txn.add_collection_type(CollectionTypeDef::List {
                    elem: ImageType::scalar(Scalar::Int),
                })
                .expect("a within-domain mint");
            },
            lookup: None,
            verdict: ImageBuildError::TooManyCollections,
        },
    ];

    for kind in kinds {
        let mut owner = exporting_owner();
        let mut txn = admitted(&mut owner);
        for index in 0..kind.maximum - kind.seeded {
            (kind.mint)(&mut txn, index);
        }
        assert_eq!(
            txn.encode().map(|_| ()),
            Ok(()),
            "exactly the maximum is accepted at the fence",
        );
        if let Some(lookup) = kind.lookup {
            (lookup)(&mut txn, 0);
            assert_eq!(
                txn.encode().map(|_| ()),
                Ok(()),
                "the keyed lookup served the existing row at the maximum instead of minting",
            );
        }
        txn.commit();
        assert!(
            owner.encode().is_ok(),
            "the committed maximum is still a complete artifact",
        );

        let mut txn = admitted(&mut owner);
        (kind.mint)(&mut txn, kind.maximum);
        assert_eq!(
            txn.encode().map(|_| ()),
            Err(kind.verdict),
            "the boundary sits exactly at the maximum: one more row crosses",
        );
    }
}

/// The duplicate-constant law, the constant-side sibling of the duplicate string hit:
/// interning the same integer, text, or date twice returns the same id and mutates
/// nothing, so a repeated constant costs no row and leaves the artifact byte-identical.
#[test]
fn a_duplicate_constant_hit_returns_the_same_id_and_mutates_nothing() {
    let mut owner = exporting_owner();
    let mut txn = admitted(&mut owner);
    let first_int = txn.intern_int(4242).expect("a within-domain mint");
    let first_text = txn.intern_text("duplicate").expect("a within-domain mint");
    let first_date = txn.intern_date(19_000).expect("a within-domain mint");
    txn.commit();
    let minted = owner.encode().expect("the minted draft encodes").bytes;

    let mut txn = admitted(&mut owner);
    assert_eq!(
        txn.intern_int(4242).expect("a within-domain mint"),
        first_int,
        "a duplicate integer hit reuses its row",
    );
    assert_eq!(
        txn.intern_text("duplicate").expect("a within-domain mint"),
        first_text,
        "a duplicate text hit reuses its row",
    );
    assert_eq!(
        txn.intern_date(19_000).expect("a within-domain mint"),
        first_date,
        "a duplicate date hit reuses its row",
    );
    txn.commit();

    assert_eq!(
        owner.encode().expect("the draft still encodes").bytes,
        minted,
        "three duplicate constant hits mutated nothing",
    );
}

/// The complete post-unwind savepoint law, whose three clauses hold together and not
/// merely one at a time: an admitted transaction unwinds, every owner is restored
/// exactly, the sibling and the reused admitted savepoint are both still stale because
/// the epoch stays rotated, and a freshly minted savepoint captures that rotated epoch
/// and admits normally.
#[test]
fn after_an_unwind_the_owners_restore_while_both_savepoints_stay_stale() {
    let mut owner = exporting_owner();
    let clean = owner.encode().expect("the base draft encodes").bytes;

    let sibling = owner.savepoint();
    let admitted_token = owner.savepoint();

    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut txn = owner
            .begin_transaction(admitted_token)
            .expect("a fresh savepoint admits");
        txn.intern_string("during-the-unwind")
            .expect("a within-domain mint");
        txn.intern_int(777).expect("a within-domain mint");
        panic!("the body raises after mutating its owners");
    }));
    assert!(unwound.is_err(), "the panic reached the catch");

    assert_eq!(
        owner.encode().expect("the restored draft encodes").bytes,
        clean,
        "the armed guard restored every owner during the unwind",
    );

    assert!(
        matches!(
            owner.begin_transaction(sibling),
            Err(DraftStateError::StaleEpoch)
        ),
        "the sibling savepoint stays stale: admission consumed the epoch it captured",
    );
    assert!(
        owner.begin_transaction(owner.savepoint()).is_ok(),
        "a savepoint minted after the unwind captures the rotated epoch and admits",
    );
    assert_eq!(
        owner.encode().expect("the draft still encodes").bytes,
        clean,
        "neither the refusal nor the fresh admission changed an owner",
    );
}

/// The multiple-policy permutation law: when a coupled batch crosses two table policies,
/// the fence returns the canonical minimum — the lower-ranked kind in the legacy walk's
/// own candidate order — whichever order the two crossings were induced in.
///
/// This is the property that makes the policy result a function of the draft rather than
/// of the traversal that built it: a lowering pass that happens to intern before it
/// declares, or the reverse, cannot change which limit a program is reported against.
#[test]
fn every_pair_of_policy_crossings_yields_the_canonical_minimum_in_either_order() {
    struct Crossing {
        /// Position in the legacy walk's candidate order; the lower rank is canonical.
        rank: usize,
        cross: fn(&mut DraftTxn<'_>),
        verdict: ImageBuildError,
    }
    let crossings = [
        Crossing {
            rank: 0,
            cross: |txn| {
                for index in 0..=MAX_STRINGS {
                    txn.intern_string(&format!("s{index:05}"))
                        .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyStrings,
        },
        Crossing {
            rank: 2,
            cross: |txn| {
                for value in 0..=(MAX_CONSTS as i64) {
                    txn.intern_int(value).expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyConsts,
        },
        Crossing {
            rank: 3,
            cross: |txn| {
                let name = txn.intern_string("T").expect("a within-domain mint");
                for _ in 0..=MAX_TYPES {
                    txn.add_record_type(RecordTypeDef {
                        name,
                        fields: Vec::new(),
                    })
                    .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyTypes,
        },
        Crossing {
            rank: 4,
            cross: |txn| {
                let name = txn.intern_string("E").expect("a within-domain mint");
                for _ in 0..=MAX_ENUMS {
                    txn.add_enum_type(EnumTypeDef {
                        name,
                        variants: Vec::new(),
                    })
                    .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyEnums,
        },
        Crossing {
            rank: 5,
            cross: |txn| {
                for _ in 0..=MAX_COLLECTIONS {
                    txn.add_collection_type(CollectionTypeDef::List {
                        elem: ImageType::scalar(Scalar::Int),
                    })
                    .expect("a within-domain mint");
                }
            },
            verdict: ImageBuildError::TooManyCollections,
        },
    ];

    for first in &crossings {
        for second in &crossings {
            if first.rank == second.rank {
                continue;
            }
            let canonical = if first.rank < second.rank {
                first.verdict.clone()
            } else {
                second.verdict.clone()
            };

            let mut owner = exporting_owner();
            let clean = owner.encode().expect("the base draft encodes").bytes;
            {
                let mut txn = admitted(&mut owner);
                (first.cross)(&mut txn);
                (second.cross)(&mut txn);
                assert_eq!(
                    txn.encode().map(|_| ()),
                    Err(canonical.clone()),
                    "the canonical minimum does not depend on which crossing came first",
                );
            }
            assert_eq!(
                owner.encode().expect("the restored draft encodes").bytes,
                clean,
                "rollback restored both crossings and the whole ledger byte for byte",
            );

            let mut txn = admitted(&mut owner);
            (first.cross)(&mut txn);
            (second.cross)(&mut txn);
            txn.commit();
            assert_eq!(
                owner.encode().map(|_| ()),
                Err(canonical),
                "the committed pair keeps the same canonical minimum",
            );
        }
    }
}
