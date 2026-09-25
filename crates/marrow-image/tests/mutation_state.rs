//! Differential pins over the draft mutation surface: refusal atomicity, the typed
//! hostile refusals, the set-once identity law, the nonblocking policy admissions, and
//! the failed-mutation state the atomic owners guarantee.
//!
//! Each test pins one mutation entry point's exact behavior, so a change that alters
//! an outcome here alters the surface's contract.

use marrow_image::bounds::{
    MAX_COLLECTIONS, MAX_CONSTS, MAX_ENUMS, MAX_EXPORTS, MAX_FUNCTIONS, MAX_STRING_BYTES,
    MAX_STRINGS, MAX_STRUCT_LEAVES, MAX_TEST_ENTRIES, MAX_TYPES,
};
use marrow_image::{
    AdmittedGraphInputPlan, AdmittedRoot, CollectionTypeDef, DeclarationMemberDef,
    DeclarationMemberShape, DraftStateError, DraftTxn, DurableIndexShape, EnumTypeDef, ExportId,
    FieldDef, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, TypeId,
};
use marrow_test_support::admitted_plan;

use marrow_test_support::ledger_ids::{
    APPLICATION_ID, FIELD_ID, INDEX_ID, PLACEMENT_ID, PRODUCT_ID, SECOND_PLACEMENT_ID,
};

use marrow_test_support::fixture_graph::{
    admit_root, declare_product, empty_record, unit_function,
};

/// One required int field member, minting its value shape into `draft`'s arena.
fn one_field_members(draft: &mut DraftTxn<'_>) -> Vec<DeclarationMemberDef> {
    let value = draft
        .value_scalar(Scalar::Int)
        .expect("the test arena mints");
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
fn declare_fixture_product(draft: &mut DraftTxn<'_>, plan: &AdmittedGraphInputPlan) {
    let record = empty_record(draft, "R");
    let members = one_field_members(draft);
    declare_product(
        draft,
        plan,
        LedgerIdBytes::from_bytes(PRODUCT_ID),
        record,
        members,
    );
}

/// Append one keyless root over the fixture Product at `placement`, spelled `name`.
fn admit_fixture_root(
    draft: &mut DraftTxn<'_>,
    plan: &AdmittedGraphInputPlan,
    name: &str,
    placement: [u8; 16],
) -> AdmittedRoot {
    admit_root(
        draft,
        plan,
        LedgerIdBytes::from_bytes(PRODUCT_ID),
        name,
        LedgerIdBytes::from_bytes(placement),
        Vec::new(),
        Vec::new(),
    )
}

// ---- Root-occurrence admission atomicity.

/// Publication is a preflight inside the admission, so an occurrence whose
/// managed-index ordinals cannot all be addressed is one typed refusal *before* any
/// row is pushed: no live row, an unspent plan budget, and nothing for the encoder
/// to see.
#[test]
fn a_refused_occurrence_leaves_no_live_row_and_spends_no_budget() {
    let over_ordinal_indexes = usize::from(u16::MAX) + 2;
    let plan = AdmittedGraphInputPlan::admit(1, 1, 8);
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    declare_fixture_product(&mut draft, &plan);
    let name = draft.intern_string("r").expect("a within-domain mint");
    // The ids need not be distinct: the ordinal domain, not identity, is what the
    // preflight refuses.
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
        "an occurrence whose index ordinals cannot all be addressed is refused",
    );

    // Atomicity, three ways: no live row ...
    assert_eq!(
        draft.contract_view().roots().len(),
        0,
        "the refused occurrence left no live row",
    );
    // ... the budget is unspent, so a well-formed occurrence still admits under the
    // one-root plan ...
    let second_name = draft.intern_string("s").expect("a within-domain mint");
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
        .expect("the refusal spent no budget");
    // ... and the encoder sees nothing of the refused occurrence: the surviving
    // well-formed graph encodes.
    assert!(
        draft.encode().is_ok(),
        "the encoder sees no orphan row and no orphan index count",
    );
}

// ---- The flat families admit past their policy cap; only encode refuses.

/// Every flat family, the appends that drive it one past its bound, and the result the
/// policy walk reports for the draft they leave behind.
///
/// One pin covers every row: the mutation surface admits each of these appends
/// unconditionally — no cap is held at the seam — so the policy walk at encode is the
/// sole refusal owner. Each filler leaves the rest of the draft coherent, so only the
/// cap under test can refuse.
type OverCapFamily = (&'static str, fn(&mut DraftTxn<'_>), ImageBuildError);

const OVER_CAP_FAMILIES: &[OverCapFamily] = &[
    ("strings", fill_strings, ImageBuildError::TooManyStrings),
    (
        "string bytes",
        fill_over_long_string,
        ImageBuildError::StringTooLong,
    ),
    ("consts", fill_consts, ImageBuildError::TooManyConsts),
    ("types", fill_types, ImageBuildError::TooManyTypes),
    ("enums", fill_enums, ImageBuildError::TooManyEnums),
    (
        "collections",
        fill_collections,
        ImageBuildError::TooManyCollections,
    ),
    (
        "functions",
        fill_functions,
        ImageBuildError::TooManyFunctions,
    ),
    ("exports", fill_exports, ImageBuildError::TooManyExports),
    (
        "test entries",
        fill_test_entries,
        ImageBuildError::TooManyTestEntries,
    ),
];

fn fill_strings(draft: &mut DraftTxn<'_>) {
    for n in 0..=MAX_STRINGS {
        draft
            .intern_string(&format!("s{n}"))
            .expect("a within-domain mint");
    }
}

fn fill_over_long_string(draft: &mut DraftTxn<'_>) {
    draft
        .intern_string(&"x".repeat(MAX_STRING_BYTES + 1))
        .expect("a within-domain mint");
}

fn fill_consts(draft: &mut DraftTxn<'_>) {
    for n in 0..=MAX_CONSTS {
        draft.intern_int(n as i64).expect("a within-domain mint");
    }
}

fn fill_types(draft: &mut DraftTxn<'_>) {
    let name = draft.intern_string("R").expect("a within-domain mint");
    for _ in 0..=MAX_TYPES {
        draft
            .add_record_type(RecordTypeDef {
                name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
    }
}

fn fill_enums(draft: &mut DraftTxn<'_>) {
    let name = draft.intern_string("E").expect("a within-domain mint");
    for _ in 0..=MAX_ENUMS {
        draft
            .add_enum_type(EnumTypeDef {
                name,
                variants: Vec::new(),
            })
            .expect("a within-domain mint");
    }
}

fn fill_collections(draft: &mut DraftTxn<'_>) {
    for _ in 0..=MAX_COLLECTIONS {
        draft
            .add_collection_type(CollectionTypeDef::List {
                elem: ImageType::scalar(Scalar::Int),
            })
            .expect("a within-domain mint");
    }
}

fn fill_functions(draft: &mut DraftTxn<'_>) {
    let mut last = None;
    for n in 0..=MAX_FUNCTIONS {
        last = Some(unit_function(draft, &format!("f{n}"), vec![Instr::Return]));
    }
    // `add_function`'s validate-then-push admission validates site operands only: the
    // over-cap append still mints the next id rather than refusing.
    assert_eq!(
        last.expect("one past the cap was appended").index(),
        MAX_FUNCTIONS as u16,
    );
}

fn fill_exports(draft: &mut DraftTxn<'_>) {
    // The coherence walk demands distinct targets and distinct export ids.
    for n in 0..=MAX_EXPORTS {
        let name = format!("f{n}");
        let func = unit_function(draft, &name, vec![Instr::Return]);
        draft.add_export(ExportId::of_local("m", &name), func);
    }
}

fn fill_test_entries(draft: &mut DraftTxn<'_>) {
    // The coherence walk demands unique names and unique targets.
    for n in 0..=MAX_TEST_ENTRIES {
        let label = format!("t{n}");
        let func = unit_function(draft, &label, vec![Instr::Return]);
        let name = draft.intern_string(&label).expect("a within-domain mint");
        draft.add_test_entry(name, func);
    }
}

#[test]
fn every_flat_family_admits_past_its_cap_and_only_encode_refuses() {
    for (family, fill, expected) in OVER_CAP_FAMILIES {
        let mut draft_owner = ImageDraft::new();
        let mut draft = draft_owner.begin_transaction();
        fill(&mut draft);
        assert_eq!(
            draft.encode().map(|_| ()).as_ref().err(),
            Some(expected),
            "{family}",
        );
    }
}

// ---- The fill setters refuse out-of-range ordinals and repeated fills.

/// An out-of-range record ordinal receives a typed refusal; the row count is unchanged.
#[test]
fn set_record_fields_with_an_out_of_range_id_is_refused() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let name = draft.intern_string("R").expect("a within-domain mint");
    draft
        .add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    assert_eq!(
        draft.set_record_fields(TypeId::from_index(5), Vec::new()),
        Err(DraftStateError::ForeignDraft),
    );
    assert_eq!(
        draft.record_type_count(),
        1,
        "the record count is unchanged"
    );
}

/// The other draft's enum ordinal is out of range for this empty table. The typed
/// refusal leaves its row count unchanged.
#[test]
fn set_enum_variants_with_an_out_of_range_id_is_refused() {
    let mut other_owner = ImageDraft::new();
    let mut other = other_owner.begin_transaction();
    let name = other.intern_string("E").expect("a within-domain mint");
    let foreign = other
        .add_enum_type(EnumTypeDef {
            name,
            variants: Vec::new(),
        })
        .expect("a within-domain mint");

    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    assert_eq!(
        draft.set_enum_variants(foreign, Vec::new()),
        Err(DraftStateError::ForeignDraft),
    );
    assert_eq!(draft.enum_type_count(), 0, "the enum count is unchanged");
}

/// A second fill receives the typed refusal and the committed draft remains encodable.
#[test]
fn a_second_fill_is_refused_and_the_draft_remains_encodable() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let name = draft.intern_string("R").expect("a within-domain mint");
    let field = draft.intern_string("f").expect("a within-domain mint");
    let record = draft
        .reserve_record_type(name)
        .expect("a within-domain mint");
    draft
        .set_record_fields(
            record,
            vec![FieldDef {
                name: field,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        )
        .expect("the reserved row fills once");
    assert_eq!(
        draft.set_record_fields(record, Vec::new()),
        Err(DraftStateError::RowState),
        "a second fill is refused",
    );
    draft.commit();
    let image = draft_owner.encode().expect("the filled draft encodes");
    assert!(!image.bytes.is_empty(), "the encoded image is nonempty");
}

// ---- The application identity is set-once-or-same with a sticky latch.

/// The first set stores the identity, an equal reset is an idempotent no-op, and a
/// divergent replacement latches the sticky conflict the fence reports: the first
/// identity is retained, never silently overwritten.
#[test]
fn a_divergent_application_identity_latches_a_sticky_conflict() {
    let first = LedgerIdBytes::from_bytes([0x01; 16]);
    let second = LedgerIdBytes::from_bytes([0x02; 16]);
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();

    draft.set_application_identity(first);
    assert_eq!(draft.contract_view().application(), Some(first));

    // The equal reset stays admitted: an idempotent no-op, no conflict.
    draft.set_application_identity(first);
    assert_eq!(draft.contract_view().application(), Some(first));
    assert!(
        !matches!(
            draft.encode().map(|_| ()),
            Err(ImageBuildError::ApplicationIdentityConflict)
        ),
        "an equal reset latches nothing",
    );

    draft.set_application_identity(second);
    assert_eq!(
        draft.contract_view().application(),
        Some(first),
        "the first identity is retained",
    );
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::ApplicationIdentityConflict),
        "the divergence is a sticky coherence fact the fence reports",
    );
}

// ---- The value-shape appenders are checked and the raw arena escape is deleted.

/// The value-shape appenders are checked at the transaction surface: an over-wide
/// struct is the typed carrier-domain refusal and a leaf minted by another arena is
/// the typed foreign refusal, never an out-of-range panic. Neither refusal mutates the
/// arena; the fence's whole-arena walk keeps the same bounds as defense in depth.
#[test]
fn an_over_wide_or_foreign_typed_arena_append_is_refused_and_mutates_nothing() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let int = draft
        .value_scalar(Scalar::Int)
        .expect("the test arena mints");
    assert_eq!(
        draft.value_struct(vec![("v".into(), int); MAX_STRUCT_LEAVES + 1]),
        Err(DraftStateError::CarrierDomain),
        "the over-wide append is the typed carrier-domain refusal",
    );
    assert_eq!(
        draft.value_shapes().len(),
        1,
        "the refused append entered nothing",
    );

    // A leaf minted by another draft's arena, out of range for this one.
    let foreign = {
        let mut other_owner = ImageDraft::new();
        let mut other = other_owner.begin_transaction();
        other
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        other
            .value_scalar(Scalar::Text)
            .expect("the test arena mints")
    };
    assert_eq!(
        draft.value_struct(vec![("v".into(), foreign)]),
        Err(DraftStateError::ForeignDraft),
        "the foreign leaf is the typed refusal, never a panic",
    );
    assert_eq!(
        draft.value_enum(
            LedgerIdBytes::from_bytes([0x50; 16]),
            vec![(
                LedgerIdBytes::from_bytes([0x51; 16]),
                vec![("v".into(), foreign)]
            )],
        ),
        Err(DraftStateError::ForeignDraft),
        "the foreign payload leaf is the typed refusal, never a panic",
    );
    assert_eq!(draft.value_shapes().len(), 1, "still nothing entered");
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
    let mut draft_owner = ImageDraft::new();
    let (mut draft, root) = {
        let mut draft = draft_owner.begin_transaction();
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        declare_fixture_product(&mut draft, &admitted_plan());
        let root = admit_fixture_root(&mut draft, &admitted_plan(), "r", PLACEMENT_ID);
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

    // A handle whose rows a discarded transaction appended is stale once it drops.
    draft.commit();
    let stale = {
        let mut proof = draft_owner.begin_transaction();
        let extra = admit_fixture_root(&mut proof, &admitted_plan(), "s", SECOND_PLACEMENT_ID);
        proof
            .bind_occurrence_site(
                extra.occurrence(),
                extra.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("the extra root admits a whole-payload site")
    };
    let mut draft = draft_owner.begin_transaction();
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
        let mut draft_owner = ImageDraft::new();
        let mut draft = draft_owner.begin_transaction();
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        declare_fixture_product(&mut draft, &admitted_plan());
        let root = admit_fixture_root(&mut draft, &admitted_plan(), "r", PLACEMENT_ID);
        draft.commit();
        (draft_owner, root)
    };
    let (mut mine_owner, my_root) = build();
    let mut mine = mine_owner.begin_transaction();
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
    let mut other_owner = ImageDraft::new();
    let mut other = other_owner.begin_transaction();
    other.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    declare_fixture_product(&mut other, &admitted_plan());
    let other_root = admit_fixture_root(&mut other, &admitted_plan(), "r", PLACEMENT_ID);
    let handle = other
        .bind_occurrence_site(
            other_root.occurrence(),
            other_root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("the root admits a whole-payload site");
    let foreign_site = other.request_site(&handle).expect("a live binding");

    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let name = draft.intern_string("f").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    assert!(
        draft
            .add_function(FunctionDef {
                name,
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                code: vec![Instr::DurExists(foreign_site.clone()), Instr::Return],
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

    let reserved = draft.reserve_function().expect("a vacant slot");
    assert_eq!(reserved.index(), 1);
    assert!(draft.function_code(reserved).is_none());
    let invalid = FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        code: vec![Instr::DurExists(foreign_site), Instr::Return],
        spans: Vec::new(),
    };
    assert!(draft.fill_function(reserved, invalid).is_err());
    assert!(draft.function_code(reserved).is_none());
    assert_eq!(draft.function_count(), 2);
    let valid = FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        code: vec![Instr::Return],
        spans: Vec::new(),
    };
    draft
        .fill_function(reserved, valid.clone())
        .expect("the refused fill spent nothing");
    let before = draft.encode().expect("both slots are filled").bytes;
    assert_eq!(
        draft.fill_function(reserved, valid),
        Err(DraftStateError::RowState)
    );
    assert_eq!(
        draft.function_code(reserved),
        Some([Instr::Return].as_slice())
    );
    assert_eq!(
        draft.encode().expect("double fill changed nothing").bytes,
        before
    );
}

/// A discarded proof rolls back `intern_text`'s two-table compound whole: the string
/// row and the constant row it appended are both truncated, the finished draft encodes
/// byte-identically, and re-interning after the drop re-mints the exact ids the proof
/// held — proof that both tables were restored to their pre-proof lengths.
#[test]
fn a_discarded_proof_rolls_back_the_intern_text_compound() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let main = unit_function(&mut draft, "main", vec![Instr::Return]);
    draft.add_export(ExportId::of_local("m", "main"), main);
    let before = draft.encode().expect("a fitting draft").bytes;

    draft.commit();
    let proof_const = {
        let mut proof = draft_owner.begin_transaction();
        proof
            .intern_text("throwaway-text")
            .expect("a within-domain mint")
    };
    let mut draft = draft_owner.begin_transaction();
    let after = draft.encode().expect("a fitting draft").bytes;
    assert_eq!(before, after, "the compound appended nothing that survived");

    let re_minted = draft
        .intern_text("throwaway-text")
        .expect("a within-domain mint");
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));

    draft.commit();
    let old_site = {
        let mut proof = draft_owner.begin_transaction();
        declare_fixture_product(&mut proof, &admitted_plan());
        let root = admit_fixture_root(&mut proof, &admitted_plan(), "r", PLACEMENT_ID);
        let handle = proof
            .bind_occurrence_site(
                root.occurrence(),
                root.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("the root admits a whole-payload site");
        proof.request_site(&handle).expect("a live binding")
    };
    let mut draft = draft_owner.begin_transaction();

    // The identical rows re-mint at the same ordinals, with fresh stamps.
    declare_fixture_product(&mut draft, &admitted_plan());
    let root = admit_fixture_root(&mut draft, &admitted_plan(), "r", PLACEMENT_ID);
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
    let name = draft.intern_string("f").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
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
