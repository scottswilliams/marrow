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
    DeclarationMemberShape, DraftStateError, DraftTxn, DurableIndexShape, EnumTypeDef, ExportId,
    FieldDef, FuncId, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, TypeId,
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

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const SECOND_PLACEMENT_ID: [u8; 16] = [0x1b; 16];
const FIELD_ID: [u8; 16] = [0x0e; 16];
const INDEX_ID: [u8; 16] = [0x3b; 16];

/// One required int field member, minting its value shape into `draft`'s arena.
fn one_field_members(draft: &mut DraftTxn<'_>) -> Vec<DeclarationMemberDef> {
    let value = draft.value_scalar(Scalar::Int);
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
    draft: &mut DraftTxn<'_>,
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
fn unit_function(draft: &mut DraftTxn<'_>, name: &str, code: Vec<Instr>) -> FuncId {
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
/// **Flipped under the sanctioned F-1 change, citing the pre-restructure pin this
/// test carried** (`a_failed_publish_leaves_a_live_occurrence_row`): publication is
/// now a preflight inside the admission, so an occurrence whose managed-index
/// ordinals cannot all be addressed is one typed refusal **before** any row is
/// pushed — the error comes with no live row, the plan's budget is unspent, and the
/// encoder sees nothing of it. The within-occurrence index ordinal widened with the
/// flip; the preflight's typed refusal replaces the old `u16::try_from` narrowing
/// arm that fired only after the row had landed.
#[test]
fn a_refused_occurrence_leaves_no_live_row_and_spends_no_budget() {
    let over_ordinal_indexes = usize::from(u16::MAX) + 2;
    let plan = AdmittedGraphInputPlan::admit(1, 1, 8).expect("a one-root budget");
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    declare_fixture_product(&mut draft, &plan);
    let name = draft.intern_string("r");
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
    let second_name = draft.intern_string("s");
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

// ---- F-2: the flat families admit past their policy cap; only encode refuses.

/// **This pins the pre-restructure behavior; the sanctioned F-2 change may flip it,
/// citing this pin.** `intern_string` admits past `MAX_STRINGS` unconditionally; the
/// policy walk at encode is the sole refusal owner.
#[test]
fn the_string_pool_admits_past_its_cap_and_only_encode_refuses() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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

// ---- F-3: the fill setters refuse a foreign or stale id with a typed error.

/// **Flipped under the sanctioned F-3 change, citing the pre-restructure pin this
/// test carried** (`set_record_fields_with_a_foreign_id_panics`): the fill is a
/// checked lookup — an id this draft never minted is the closed typed refusal, never
/// a panic — and the refusal mutates nothing.
#[test]
fn set_record_fields_with_a_foreign_id_is_refused() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let name = draft.intern_string("R");
    draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    assert_eq!(
        draft.set_record_fields(TypeId::from_index(5), Vec::new()),
        Err(DraftStateError::ForeignDraft),
    );
    assert_eq!(draft.record_type_count(), 1, "the refusal mutated nothing");
}

/// **Flipped under the sanctioned F-3 change, citing the pre-restructure pin this
/// test carried** (`set_enum_variants_with_a_foreign_id_panics`): an id minted by
/// another draft is the closed typed refusal against this draft's table, never a
/// panic, and the refusal mutates nothing.
#[test]
fn set_enum_variants_with_a_foreign_id_is_refused() {
    let mut other_owner = ImageDraft::new();
    let mut other = admitted(&mut other_owner);
    let name = other.intern_string("E");
    let foreign = other.add_enum_type(EnumTypeDef {
        name,
        variants: Vec::new(),
    });

    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    assert_eq!(
        draft.set_enum_variants(foreign, Vec::new()),
        Err(DraftStateError::ForeignDraft),
    );
    assert_eq!(draft.enum_type_count(), 0, "the refusal mutated nothing");
}

/// The one-time-fill half of the same law: a second fill of one row is the typed
/// refusal, never an overwrite, and the first fill's definition survives.
#[test]
fn a_second_fill_of_one_row_is_refused_without_overwriting() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let name = draft.intern_string("R");
    let field = draft.intern_string("f");
    let record = draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
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
        Err(DraftStateError::IncoherentToken),
        "a second fill is refused",
    );
    draft.commit();
    let image = draft_owner.encode().expect("the filled draft encodes");
    assert!(
        !image.bytes.is_empty(),
        "the first fill's definition survived"
    );
}

// ---- F-4: set_application_identity is set-once-or-same with a sticky latch.

/// **Flipped under the sanctioned F-4 change, citing the pre-restructure pin this
/// test carried** (`a_divergent_application_identity_silently_overwrites`): the
/// first set stores the identity, an equal reset is an idempotent no-op, and a
/// divergent replacement latches the sticky conflict the fence reports — the first
/// identity is retained, never silently overwritten.
#[test]
fn a_divergent_application_identity_latches_a_sticky_conflict() {
    let first = LedgerIdBytes::from_bytes([0x01; 16]);
    let second = LedgerIdBytes::from_bytes([0x02; 16]);
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);

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

// ---- F-5: the raw arena escape is deleted; the typed appenders remain nonblocking.

/// **Flipped to the design of record's F-5 disposition (post-build ruling F2; design
/// draft 2 §10 F-5), citing the two pins this test carried**
/// (`an_over_wide_raw_arena_append_succeeds_and_only_encode_refuses`, then
/// `an_over_wide_typed_arena_append_is_admitted_and_only_encode_refuses`): the typed
/// appenders are checked — an over-wide struct is the typed carrier-domain refusal
/// at the surface and a leaf minted by another arena is the typed foreign refusal,
/// never an out-of-range panic. Neither refusal mutates the arena; the fence's
/// whole-arena walk keeps the same bounds as defense in depth.
#[test]
fn an_over_wide_or_foreign_typed_arena_append_is_refused_and_mutates_nothing() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let int = draft.value_scalar(Scalar::Int);
    assert_eq!(
        draft.value_struct(vec![int; MAX_STRUCT_LEAVES + 1]),
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
        let mut other = admitted(&mut other_owner);
        other.value_scalar(Scalar::Int);
        other.value_scalar(Scalar::Text)
    };
    assert_eq!(
        draft.value_struct(vec![foreign]),
        Err(DraftStateError::ForeignDraft),
        "the foreign leaf is the typed refusal, never a panic",
    );
    assert_eq!(
        draft.value_enum(
            LedgerIdBytes::from_bytes([0x50; 16]),
            vec![(LedgerIdBytes::from_bytes([0x51; 16]), vec![foreign])],
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
        let mut draft = admitted(&mut draft_owner);
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

    // A handle whose rows a discarded transaction appended is stale once it drops.
    draft.commit();
    let stale = {
        let mut proof = admitted(&mut draft_owner);
        let extra = admit_fixture_root(&mut proof, &admitted_plan(), SECOND_PLACEMENT_ID);
        proof
            .bind_occurrence_site(
                extra.occurrence(),
                extra.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("the extra root admits a whole-payload site")
    };
    let mut draft = admitted(&mut draft_owner);
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
        let mut draft = admitted(&mut draft_owner);
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        declare_fixture_product(&mut draft, &admitted_plan());
        let root = admit_fixture_root(&mut draft, &admitted_plan(), PLACEMENT_ID);
        draft.commit();
        (draft_owner, root)
    };
    let (mut mine_owner, my_root) = build();
    let mut mine = admitted(&mut mine_owner);
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
    let mut other = admitted(&mut other_owner);
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

    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let main = unit_function(&mut draft, "main", vec![Instr::Return]);
    draft.add_export(ExportId::of_local("m", "main"), main);
    let before = draft.encode().expect("a fitting draft").bytes;

    draft.commit();
    let proof_const = {
        let mut proof = admitted(&mut draft_owner);
        proof.intern_text("throwaway-text")
    };
    let mut draft = admitted(&mut draft_owner);
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));

    draft.commit();
    let old_site = {
        let mut proof = admitted(&mut draft_owner);
        declare_fixture_product(&mut proof, &admitted_plan());
        let root = admit_fixture_root(&mut proof, &admitted_plan(), PLACEMENT_ID);
        let handle = proof
            .bind_occurrence_site(
                root.occurrence(),
                root.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("the root admits a whole-payload site");
        proof.request_site(&handle).expect("a live binding")
    };
    let mut draft = admitted(&mut draft_owner);

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
