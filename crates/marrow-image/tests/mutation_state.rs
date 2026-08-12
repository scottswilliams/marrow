//! Differential pins over the draft mutation surface, frozen before the draft
//! transaction and table-policy ledger restructure (IMGTABLE01).
//!
//! Each test pins the exact **current** behavior of one mutation entry point — the
//! non-atomic hole, the raw-index panics, the silent identity overwrite, the
//! admit-now-refuse-at-encode split of the flat families, and the failed-mutation
//! state the atomic owners already guarantee. A sanctioned change family (F-1..F-5 of
//! the lane design) may flip a pinned outcome; the flipping lane cites the pin it
//! flips and updates it in the same commit. A pin that fails without such a citation
//! is a regression.

use marrow_image::bounds::{
    MAX_COLLECTIONS, MAX_CONSTS, MAX_ENUMS, MAX_EXPORTS, MAX_FUNCTIONS, MAX_STRING_BYTES,
    MAX_STRINGS, MAX_STRUCT_LEAVES, MAX_TEST_ENTRIES, MAX_TYPES,
};
use marrow_image::{
    AdmittedGraphInputPlan, AdmittedRoot, CollectionTypeDef, DeclarationMemberDef,
    DeclarationMemberShape, DurableIndexShape, EnumTypeDef, ExportId, FuncId, FunctionDef,
    ImageBuildError, ImageDraft, ImageType, Instr, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef,
    Scalar, SemanticTarget, TypeId,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const SECOND_PLACEMENT_ID: [u8; 16] = [0x1b; 16];
const FIELD_ID: [u8; 16] = [0x0e; 16];
const INDEX_ID: [u8; 16] = [0x3b; 16];

/// One required int field member, minting its value shape into `draft`'s arena.
fn one_field_members(draft: &mut ImageDraft) -> Vec<DeclarationMemberDef> {
    let value = draft.value_shapes_mut().scalar(Scalar::Int);
    vec![DeclarationMemberDef {
        parent: None,
        shape: DeclarationMemberShape::Field {
            id: LedgerIdBytes::from_bytes(FIELD_ID),
            required: true,
            value,
        },
    }]
}

/// Declare the one fixture Product (one record type, one field member) under `plan`.
fn declare_fixture_product(draft: &mut ImageDraft, plan: &AdmittedGraphInputPlan) {
    let name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    let members = one_field_members(draft);
    draft
        .declare_product(plan, LedgerIdBytes::from_bytes(PRODUCT_ID), record, members)
        .expect("a well-formed declaration");
}

/// Append one keyless root over the fixture Product, named and placed by `n`.
fn admit_fixture_root(
    draft: &mut ImageDraft,
    plan: &AdmittedGraphInputPlan,
    placement: [u8; 16],
) -> AdmittedRoot {
    let name = draft.intern_string(std::str::from_utf8(&placement[..1]).unwrap_or("r"));
    draft
        .add_root_occurrence(
            plan,
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name,
                keys: Vec::new(),
                placement: LedgerIdBytes::from_bytes(placement),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared")
}

/// A zero-argument unit function of `code`, appended and expected to be admitted.
fn unit_function(draft: &mut ImageDraft, name: &str, code: Vec<Instr>) -> FuncId {
    let name = draft.intern_string(name);
    let source = draft.intern_string("src/main.mw");
    draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code,
            spans: Vec::new(),
        })
        .expect("every site operand is live")
}

// ---- F-1: the add_root_occurrence hole.

/// **This pins the pre-restructure behavior; the sanctioned F-1 change may flip it,
/// citing this pin.** `add_root_occurrence` is not atomic: `push_under` commits the
/// occurrence row, then `publish` can still refuse, so the caller gets an error *and*
/// the draft keeps a live row the encoder will see.
///
/// The reachable `publish` `None` arm is the index-ordinal narrowing at
/// `product.rs` (`u16::try_from(index)` inside `OccurrenceGraph::publish`): it fails
/// exactly when some index ordinal in `0..indexes.len()` exceeds `u16::MAX`, i.e. when
/// `indexes.len() >= u16::MAX as usize + 2 = 65_537`. Nothing bounds the index list
/// before that point — `push_under` checks only the plan's root budget and the row
/// ordinal — so a caller-supplied 65 537-entry index list reaches it through the public
/// API. The other `None` arm (`live()` missing the row) is unreachable here: the
/// selector was just published for the freshly pushed row.
///
/// After F-1, `publish` becomes a preflight and the same input is an error with **no**
/// live row; this pin's three liveness assertions are the ones that flip.
#[test]
fn a_failed_publish_leaves_a_live_occurrence_row() {
    let over_ordinal_indexes = usize::from(u16::MAX) + 2;
    let plan = AdmittedGraphInputPlan::admit(1, 1, 8).expect("a one-root budget");
    let mut draft = ImageDraft::new();
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    declare_fixture_product(&mut draft, &plan);
    let name = draft.intern_string("r");
    // The ids need not be distinct: the ordinal arithmetic, not identity, is what
    // refuses, and the pinned encode refusal is the index-count bound.
    let indexes = vec![
        DurableIndexShape {
            id: LedgerIdBytes::from_bytes(INDEX_ID),
            unique: false,
            components: Vec::new(),
        };
        over_ordinal_indexes
    ];
    assert!(
        draft
            .add_root_occurrence(
                &plan,
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                    indexes: indexes.into(),
                },
            )
            .is_err(),
        "an occurrence whose index ordinals cannot all be spelled is refused",
    );

    // Liveness, three ways: the contract view already carries the row ...
    assert_eq!(
        draft.contract_view().roots().len(),
        1,
        "the refused occurrence's row is live in the draft",
    );
    // ... the row spent the one admitted root, so a well-formed occurrence is now over
    // budget ...
    let second_name = draft.intern_string("s");
    assert!(
        draft
            .add_root_occurrence(
                &plan,
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name: second_name,
                    keys: Vec::new(),
                    placement: LedgerIdBytes::from_bytes(SECOND_PLACEMENT_ID),
                    indexes: Vec::new().into(),
                },
            )
            .is_err(),
        "the orphan row spent the plan's one-root budget",
    );
    // ... and the encoder reports the orphan row's own index-count fault, which it
    // could only do over a row it holds.
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyIndexes),
        "the encoder sees the orphan row and refuses its index count",
    );
}

// ---- F-2: the flat families admit past their policy cap; only encode refuses.

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `intern_string` admits past `MAX_STRINGS` unconditionally; the
/// policy walk at encode is the sole refusal owner.
#[test]
fn the_string_pool_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    for n in 0..=MAX_STRINGS {
        draft.intern_string(&format!("s{n}"));
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyStrings),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** An over-long string is interned without refusal; the policy walk
/// at encode is the sole refusal owner.
#[test]
fn an_over_long_string_is_admitted_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    draft.intern_string(&"x".repeat(MAX_STRING_BYTES + 1));
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::StringTooLong),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `intern_int` admits past `MAX_CONSTS` unconditionally; the
/// policy walk at encode is the sole refusal owner.
#[test]
fn the_const_pool_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    for n in 0..=MAX_CONSTS {
        draft.intern_int(n as i64);
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyConsts),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `add_record_type` admits past `MAX_TYPES` unconditionally; the
/// policy walk at encode is the sole refusal owner.
#[test]
fn the_type_table_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("R");
    for _ in 0..=MAX_TYPES {
        draft.add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        });
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyTypes),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `add_enum_type` admits past `MAX_ENUMS` unconditionally; the
/// policy walk at encode is the sole refusal owner.
#[test]
fn the_enum_table_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("E");
    for _ in 0..=MAX_ENUMS {
        draft.add_enum_type(EnumTypeDef {
            name,
            variants: Vec::new(),
        });
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyEnums),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `add_collection_type` admits past `MAX_COLLECTIONS`
/// unconditionally; the policy walk at encode is the sole refusal owner.
#[test]
fn the_collection_table_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    for _ in 0..=MAX_COLLECTIONS {
        draft.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        });
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyCollections),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `add_function`'s validate-then-push admission validates site
/// operands only — it does **not** hold the function cap. A draft one function past
/// `MAX_FUNCTIONS` still admits every append (the last minted `FuncId` is the count
/// past the cap), and the policy walk at encode is the sole cap refusal owner.
#[test]
fn the_function_table_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    let mut last = None;
    for n in 0..=MAX_FUNCTIONS {
        last = Some(unit_function(
            &mut draft,
            &format!("f{n}"),
            vec![Instr::Return],
        ));
    }
    assert_eq!(
        last.expect("one past the cap was appended").index(),
        MAX_FUNCTIONS as u16,
        "the over-cap append still minted the next id",
    );
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyFunctions),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `add_export` admits past `MAX_EXPORTS` unconditionally; the
/// policy walk at encode is the sole refusal owner.
#[test]
fn the_export_table_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    // The coherence walk demands distinct targets and distinct export ids, so the
    // over-cap table is otherwise coherent and only the cap can refuse.
    for n in 0..=MAX_EXPORTS {
        let name = format!("f{n}");
        let func = unit_function(&mut draft, &name, vec![Instr::Return]);
        draft.add_export(ExportId::of_local("m", &name), func);
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyExports),
    );
}

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `add_test_entry` admits past `MAX_TEST_ENTRIES` unconditionally;
/// the policy walk at encode is the sole refusal owner.
#[test]
fn the_test_entry_table_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    // The coherence walk demands unique names and unique targets, so the over-cap
    // table is otherwise coherent and only the cap can refuse.
    for n in 0..=MAX_TEST_ENTRIES {
        let label = format!("t{n}");
        let func = unit_function(&mut draft, &label, vec![Instr::Return]);
        let name = draft.intern_string(&label);
        draft.add_test_entry(name, func);
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyTestEntries),
    );
}

// ---- F-3: the raw-index setters panic on a foreign or stale id.

/// **This pins the pre-restructure behavior; the sanctioned F-3 change may flip it,
/// citing this pin.** `set_record_fields` resolves its `TypeId` by raw `Vec` indexing
/// (`self.types[ty.0 as usize]` in `draft.rs`), so an id this draft never minted is an
/// index-out-of-bounds panic, not a typed refusal.
#[test]
#[should_panic(expected = "index out of bounds: the len is 1 but the index is 5")]
fn set_record_fields_with_a_foreign_id_panics() {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("R");
    draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    draft.set_record_fields(TypeId::from_index(5), Vec::new());
}

/// **This pins the pre-restructure behavior; the sanctioned F-3 change may flip it,
/// citing this pin.** `set_enum_variants` resolves its `EnumId` by raw `Vec` indexing
/// (`self.enums[id.0 as usize]` in `draft.rs`), so an id minted by another draft is an
/// index-out-of-bounds panic against this draft's table, not a typed refusal.
#[test]
#[should_panic(expected = "index out of bounds: the len is 0 but the index is 0")]
fn set_enum_variants_with_a_foreign_id_panics() {
    let mut other = ImageDraft::new();
    let name = other.intern_string("E");
    let foreign = other.add_enum_type(EnumTypeDef {
        name,
        variants: Vec::new(),
    });

    let mut draft = ImageDraft::new();
    draft.set_enum_variants(foreign, Vec::new());
}

// ---- F-4: set_application_identity silently overwrites a divergent id.

/// **This pins the pre-restructure behavior; the sanctioned F-4 change may flip it,
/// citing this pin.** `set_application_identity` is an unguarded slot write: a second
/// call with a *different* id silently wins, where the Product route latches a sticky
/// conflict for the analogous divergent reclaim. After F-4 the divergent second call
/// latches a conflict instead; the equal-reset half stays admitted.
#[test]
fn a_divergent_application_identity_silently_overwrites() {
    let first = LedgerIdBytes::from_bytes([0x01; 16]);
    let second = LedgerIdBytes::from_bytes([0x02; 16]);
    let mut draft = ImageDraft::new();

    draft.set_application_identity(first);
    assert_eq!(draft.contract_view().application(), Some(first));

    draft.set_application_identity(second);
    assert_eq!(
        draft.contract_view().application(),
        Some(second),
        "the divergent second id wins without any recorded conflict",
    );
}

// ---- F-5: the raw value-shape arena escape admits what only encode refuses.

/// **This pins the pre-restructure behavior; the sanctioned F-5 change may flip it,
/// citing this pin.** `value_shapes_mut` hands out the raw `&mut` arena, so an
/// over-wide struct shape is appended without refusal — observable as arena growth —
/// and the encoder's whole-arena recheck is the sole refusal owner, even when no
/// declaration references the shape.
#[test]
fn an_over_wide_raw_arena_append_succeeds_and_only_encode_refuses() {
    let mut draft = ImageDraft::new();
    let values = draft.value_shapes_mut();
    let int = values.scalar(Scalar::Int);
    values.struct_shape(vec![int; MAX_STRUCT_LEAVES + 1]);
    assert_eq!(
        draft.value_shapes().len(),
        2,
        "the raw append admitted the scalar and the over-wide struct",
    );
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

// ---- Failed-mutation state: the atomic owners leave the draft untouched.

/// A failed `request_site` leaves the site plan unchanged: no row is appended, the
/// demand map still answers, and later mints continue at the next ordinal. (The draft's
/// private stamp cursor does advance on this path — deliberately, per the stale-binding
/// design — which the fresh-stamp pin below covers.)
///
/// The plan's state is read through its public faces: an operand's `Debug` renders the
/// logical ordinal the plan minted, so ordinal continuity across the failure is row-count
/// invariance.
#[test]
fn a_failed_site_request_leaves_the_site_plan_unchanged() {
    let (mut draft, root) = {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        declare_fixture_product(&mut draft, &admitted_plan());
        let root = admit_fixture_root(&mut draft, &admitted_plan(), PLACEMENT_ID);
        (draft, root)
    };
    let placement_handle = draft
        .bind_occurrence_site(
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("the root admits a whole-payload site");
    let placement_site = draft
        .request_site(&placement_handle)
        .expect("the binding is live");
    assert_eq!(format!("{placement_site:?}"), "0");

    // A handle whose rows a discarded proof appended is stale once the proof drops.
    let stale = {
        let mut guard = draft.template_proof();
        let proof = guard.proof_draft();
        let extra = admit_fixture_root(proof, &admitted_plan(), SECOND_PLACEMENT_ID);
        proof
            .bind_occurrence_site(
                extra.occurrence(),
                extra.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("the extra root admits a whole-payload site")
    };
    assert!(
        draft.request_site(&stale).is_err(),
        "a handle over a discarded row does not mint",
    );

    // The failure appended no row: a fresh distinct demand mints the *next* ordinal,
    // and the retained demand still answers with the id it was given.
    let members = draft
        .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
        .expect("declared");
    let field_handle = draft
        .bind_occurrence_site(
            root.occurrence(),
            members[0].path(),
            SemanticTarget::FieldLeaf,
        )
        .expect("the field admits a field-leaf site");
    let field_site = draft.request_site(&field_handle).expect("a live binding");
    assert_eq!(
        format!("{field_site:?}"),
        "1",
        "the failed request consumed no site ordinal",
    );
    let repeat = draft
        .request_site(&placement_handle)
        .expect("the retained demand still answers");
    assert_eq!(repeat, placement_site, "the retained demand keeps its id");
}

/// A handle minted by another draft is refused by `request_site` without touching the
/// plan: cross-draft authority is checked before anything is spent, so the refused
/// draft's next mint is still ordinal zero.
#[test]
fn a_foreign_handle_is_refused_without_touching_the_plan() {
    let build = || {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        declare_fixture_product(&mut draft, &admitted_plan());
        let root = admit_fixture_root(&mut draft, &admitted_plan(), PLACEMENT_ID);
        (draft, root)
    };
    let (mut mine, my_root) = build();
    let (theirs, their_root) = build();
    let foreign = theirs
        .bind_occurrence_site(
            their_root.occurrence(),
            their_root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("their root admits a whole-payload site");

    assert!(
        mine.request_site(&foreign).is_err(),
        "another draft's handle does not mint here",
    );

    let handle = mine
        .bind_occurrence_site(
            my_root.occurrence(),
            my_root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("my root admits a whole-payload site");
    let site = mine.request_site(&handle).expect("a live binding");
    assert_eq!(
        format!("{site:?}"),
        "0",
        "the foreign refusal spent nothing from this plan",
    );
}

/// A failed `add_function` appends no row: the validate-then-push admission spends the
/// operand evidence before the push, so a body carrying another draft's operand is
/// refused whole and the next successful append still mints `FuncId` zero.
#[test]
fn a_failed_function_append_leaves_no_function_row() {
    let mut other = ImageDraft::new();
    other.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    declare_fixture_product(&mut other, &admitted_plan());
    let other_root = admit_fixture_root(&mut other, &admitted_plan(), PLACEMENT_ID);
    let handle = other
        .bind_occurrence_site(
            other_root.occurrence(),
            other_root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("the root admits a whole-payload site");
    let foreign_site = other.request_site(&handle).expect("a live binding");

    let mut draft = ImageDraft::new();
    let name = draft.intern_string("f");
    let source = draft.intern_string("src/main.mw");
    assert!(
        draft
            .add_function(FunctionDef {
                name,
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                code: vec![Instr::DurExists(foreign_site), Instr::Return],
                spans: Vec::new(),
            })
            .is_err(),
        "a body carrying another draft's operand is refused",
    );
    let admitted = unit_function(&mut draft, "f", vec![Instr::Return]);
    assert_eq!(
        admitted.index(),
        0,
        "the refused body appended no function row",
    );
}

/// A discarded proof rolls back `intern_text`'s two-table compound whole: the string
/// row and the constant row it appended are both truncated, the finished draft encodes
/// byte-identically, and re-interning after the drop re-mints the exact ids the proof
/// held — proof that both tables were restored to their pre-proof lengths.
#[test]
fn a_discarded_proof_rolls_back_the_intern_text_compound() {
    let mut draft = ImageDraft::new();
    let main = unit_function(&mut draft, "main", vec![Instr::Return]);
    draft.add_export(ExportId::of_local("m", "main"), main);
    let before = draft.encode().expect("a fitting draft").bytes;

    let proof_const = {
        let mut guard = draft.template_proof();
        guard.proof_draft().intern_text("throwaway-text")
    };
    let after = draft.encode().expect("a fitting draft").bytes;
    assert_eq!(before, after, "the compound appended nothing that survived");

    let re_minted = draft.intern_text("throwaway-text");
    assert_eq!(
        re_minted.index(),
        proof_const.index(),
        "both tables were truncated to their pre-proof lengths, so the same ids re-mint",
    );
}

// ---- Transaction machinery: stamps stay monotone across a rewind.

/// A rewound-then-reappended row carries a **fresh** stamp: the stamp cursor is
/// deliberately not restored by a rewind, so an operand minted before the rewind is
/// refused after the identical rows are re-minted — even though the old and new
/// operands compare equal, equality being over the logical ordinal alone.
#[test]
fn a_rewound_and_reappended_row_refuses_the_operand_minted_before_the_rewind() {
    let mut draft = ImageDraft::new();
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));

    let old_site = {
        let mut guard = draft.template_proof();
        let proof = guard.proof_draft();
        declare_fixture_product(proof, &admitted_plan());
        let root = admit_fixture_root(proof, &admitted_plan(), PLACEMENT_ID);
        let handle = proof
            .bind_occurrence_site(
                root.occurrence(),
                root.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("the root admits a whole-payload site");
        proof.request_site(&handle).expect("a live binding")
    };

    // The identical rows re-mint at the same ordinals, with fresh stamps.
    declare_fixture_product(&mut draft, &admitted_plan());
    let root = admit_fixture_root(&mut draft, &admitted_plan(), PLACEMENT_ID);
    let handle = draft
        .bind_occurrence_site(
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("the root admits a whole-payload site");
    let new_site = draft.request_site(&handle).expect("a live binding");

    assert_eq!(
        old_site, new_site,
        "the two operands carry one logical ordinal and compare equal",
    );
    let name = draft.intern_string("f");
    let source = draft.intern_string("src/main.mw");
    assert!(
        draft
            .add_function(FunctionDef {
                name,
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                code: vec![Instr::DurExists(old_site), Instr::Return],
                spans: Vec::new(),
            })
            .is_err(),
        "the pre-rewind operand stands on a stamp no live row carries",
    );
    let admitted = unit_function(
        &mut draft,
        "f",
        vec![Instr::DurExists(new_site), Instr::Return],
    );
    assert_eq!(
        admitted.index(),
        0,
        "the fresh operand is admitted where the stale one was refused",
    );
}
