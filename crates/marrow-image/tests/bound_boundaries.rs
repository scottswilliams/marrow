//! Boundary known-answer tests: the narrow width bounds that did NOT widen
//! with the record field width still bite at exactly their chosen value. The dense
//! inline-composite leaf count admits `MAX_STRUCT_LEAVES` and refuses one more; the
//! index projection width admits `MAX_INDEX_COMPONENTS` and refuses one more. These
//! exercise the encoder's `check_bounds` recheck (the same constants the independent
//! verifier rechecks), so a future re-coupling to the widened record width is
//! conspicuous, not silent.

use marrow_image::bounds::{
    MAX_ADMITTED_DECLARATION_COMMANDS, MAX_ADMITTED_PRODUCT_DECLARATIONS,
    MAX_ADMITTED_ROOT_OCCURRENCES, MAX_DURABLE_DEPTH, MAX_INDEX_COMPONENTS, MAX_STRUCT_LEAVES,
};
use marrow_image::{
    AdmittedGraphInputPlan, DeclarationMemberDef, DeclarationMemberShape, DurableContractGraph,
    DurableGraphInputRefusal, DurableIndexComponent, DurableIndexShape, ExportId, FunctionDef,
    ImageBuildError, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootOccurrenceDef, Scalar, SpanEntry, TypeId,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const FIELD_ID: [u8; 16] = [0x0e; 16];
const INDEX_ID: [u8; 16] = [0x3b; 16];

/// A distinct 16-byte ledger id seeded by `n` (its low byte), for the many-component
/// index projections below. Kept below the reserved fixed ids above.
fn component_id(n: usize) -> LedgerIdBytes {
    // Two seed bytes carry the distinctness this helper promises. Past them the ids
    // silently repeat and a bound test would pass over a smaller set than it named.
    assert!(
        n <= u16::MAX as usize,
        "component seed exceeds its two bytes"
    );
    let mut bytes = [0x40u8; 16];
    bytes[0] = n as u8;
    bytes[1] = (n >> 8) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// A minimal encodable draft carrying a `main` returning `0`, one record type, and one
/// keyed root whose Product declares `members` and whose occurrence carries `indexes`.
/// The declared member graph and the index shapes are the only things the callers vary,
/// so the bound under test is the sole reason an encode fails.
fn encode_root(
    members: impl FnOnce(&mut ImageDraft) -> Vec<DeclarationMemberDef>,
    indexes: Vec<DurableIndexShape>,
) -> Result<(), ImageBuildError> {
    let mut draft = ImageDraft::new();
    let members = members(&mut draft);
    let type_name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let root_name = draft.intern_string("r");
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            members,
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: indexes.into(),
            },
        )
        .expect("the Product is declared");
    let src = draft.intern_string("src/main.mw");
    let main_name = draft.intern_string("main");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let main = draft
        .add_function(FunctionDef {
            name: main_name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.encode().map(|_| ())
}

/// A Product whose single field member carries a dense struct value of `leaves` scalar
/// leaves.
fn members_with_struct_field(draft: &mut ImageDraft, leaves: usize) -> Vec<DeclarationMemberDef> {
    let values = draft.value_shapes_mut();
    let int = values.scalar(Scalar::Int);
    let value = values.struct_shape(vec![int; leaves]);
    vec![DeclarationMemberDef {
        parent: None,
        shape: DeclarationMemberShape::Field {
            id: LedgerIdBytes::from_bytes(FIELD_ID),
            required: false,
            value,
        },
    }]
}

/// One nonunique index projecting `components` field components.
fn indexes_with_components(components: usize) -> Vec<DurableIndexShape> {
    vec![DurableIndexShape {
        id: LedgerIdBytes::from_bytes(INDEX_ID),
        unique: false,
        components: (0..components)
            .map(|n| DurableIndexComponent::Field(component_id(n)))
            .collect(),
    }]
}

#[test]
fn a_dense_struct_at_the_leaf_limit_encodes() {
    assert_eq!(
        encode_root(
            |draft| members_with_struct_field(draft, MAX_STRUCT_LEAVES),
            Vec::new(),
        ),
        Ok(()),
        "a struct value of exactly MAX_STRUCT_LEAVES leaves is admitted",
    );
}

#[test]
fn a_dense_struct_one_leaf_over_the_limit_is_refused() {
    assert_eq!(
        encode_root(
            |draft| members_with_struct_field(draft, MAX_STRUCT_LEAVES + 1),
            Vec::new(),
        ),
        Err(ImageBuildError::TooManyStructLeaves),
        "one leaf past the dense-composite limit is refused as TooManyStructLeaves",
    );
}

/// The value bounds are rechecked over the whole arena, not over the shapes a
/// declaration happens to reference.
///
/// The arena is the draft's own retained state. A node past a value bound is a producer
/// defect wherever it came from, and deciding it by reachability would make the same draft
/// encode or refuse depending on a traversal — while paying for a reachability walk to
/// learn something no correct producer can produce. This pins the declared precondition:
/// a draft whose declarations are all within bounds still refuses when its arena holds an
/// over-wide shape nothing references.
#[test]
fn an_over_wide_shape_no_declaration_references_still_refuses() {
    assert_eq!(
        encode_root(
            |draft| {
                let members = members_with_struct_field(draft, MAX_STRUCT_LEAVES);
                let values = draft.value_shapes_mut();
                let int = values.scalar(Scalar::Int);
                let _unreferenced = values.struct_shape(vec![int; MAX_STRUCT_LEAVES + 1]);
                members
            },
            Vec::new(),
        ),
        Err(ImageBuildError::TooManyStructLeaves),
        "an unreferenced over-wide shape in the draft's arena refuses the encode",
    );
}

#[test]
fn an_index_at_the_component_limit_encodes() {
    assert_eq!(
        encode_root(
            |_| Vec::new(),
            indexes_with_components(MAX_INDEX_COMPONENTS)
        ),
        Ok(()),
        "an index projecting exactly MAX_INDEX_COMPONENTS components is admitted",
    );
}

#[test]
fn an_index_one_component_over_the_limit_is_refused() {
    assert_eq!(
        encode_root(
            |_| Vec::new(),
            indexes_with_components(MAX_INDEX_COMPONENTS + 1)
        ),
        Err(ImageBuildError::TooManyIndexComponents),
        "one component past the projection limit is refused as TooManyIndexComponents",
    );
}

// ---- The admitted graph input plan's own ceilings.

/// The construction budget refuses a term beyond what may be handed to the durable graph,
/// and admits every term at its ceiling.
///
/// This is the plan's teeth: the entry points check a caller's input against the plan's
/// counts, so a plan carrying unadmitted counts would make every one of those checks
/// vacuous. Each ceiling sits exactly one past the bound whose refusal owner keeps its
/// refusal — the encoder's `TooManyRoots` over a complete graph, and its
/// `TooManyDurableMembers` over an over-wide declaration — so an admitted N+1 still
/// reaches the owner that reports it, while N+2 is never handed over at all.
#[test]
fn a_construction_budget_beyond_the_admitted_intake_is_refused() {
    assert!(
        AdmittedGraphInputPlan::admit(
            MAX_ADMITTED_PRODUCT_DECLARATIONS,
            MAX_ADMITTED_ROOT_OCCURRENCES,
            MAX_ADMITTED_DECLARATION_COMMANDS,
        )
        .is_some(),
        "every term at its ceiling is an admitted budget",
    );

    for (products, roots, commands, term) in [
        (
            MAX_ADMITTED_PRODUCT_DECLARATIONS + 1,
            MAX_ADMITTED_ROOT_OCCURRENCES,
            MAX_ADMITTED_DECLARATION_COMMANDS,
            "Product declarations",
        ),
        (
            MAX_ADMITTED_PRODUCT_DECLARATIONS,
            MAX_ADMITTED_ROOT_OCCURRENCES + 1,
            MAX_ADMITTED_DECLARATION_COMMANDS,
            "root occurrences",
        ),
        (
            MAX_ADMITTED_PRODUCT_DECLARATIONS,
            MAX_ADMITTED_ROOT_OCCURRENCES,
            MAX_ADMITTED_DECLARATION_COMMANDS + 1,
            "declaration commands",
        ),
    ] {
        assert!(
            AdmittedGraphInputPlan::admit(products, roots, commands).is_none(),
            "one past the admitted {term} ceiling is not a budget any image could carry",
        );
    }

    assert!(
        AdmittedGraphInputPlan::admit(usize::MAX, usize::MAX, usize::MAX).is_none(),
        "an unbounded wish is refused rather than saturated into a budget",
    );
}

/// A budget the plan admits cannot be spent past its own counts: the entry point refuses a
/// command vector wider than the plan admitted, before any row is appended.
///
/// The refusal is the plan's, not the encoder's — a narrower plan binds earlier than the
/// member bound does — and it leaves the draft holding no declaration at all.
#[test]
fn a_command_vector_wider_than_its_budget_appends_no_row() {
    let plan = AdmittedGraphInputPlan::admit(1, 1, 1).expect("a one-command budget");
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    let value = draft.value_shapes_mut().scalar(Scalar::Int);
    let two = vec![
        DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: LedgerIdBytes::from_bytes(FIELD_ID),
                required: true,
                value,
            },
        },
        DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: component_id(1),
                required: false,
                value,
            },
        },
    ];
    assert!(
        draft
            .declare_product(&plan, LedgerIdBytes::from_bytes(PRODUCT_ID), record, two)
            .is_err(),
        "a command vector past the admitted count is refused",
    );
    assert!(
        draft
            .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
            .is_none(),
        "the refused declaration appended no row",
    );
}

// ---- Nesting depth is bounded where the rows are made.

/// A command vector nesting `depth` groups, each the parent of the next.
///
/// The innermost group declares no member of its own: nesting depth is a property of the
/// body, not of anything the body happens to hold, so the deepest row here is an empty
/// group and it is over-deep exactly when its own level is.
fn nested_groups(depth: usize) -> Vec<DeclarationMemberDef> {
    (0..depth)
        .map(|level| DeclarationMemberDef {
            parent: level
                .checked_sub(1)
                .map(|parent| u16::try_from(parent).expect("the fixture nests within u16")),
            shape: DeclarationMemberShape::Group {
                id: component_id(level),
            },
        })
        .collect()
}

/// The depth an over-deep repro states: far past `MAX_DURABLE_DEPTH`, and past every
/// per-walk stack a bounded owner could absorb, so a route that admits it admits an
/// arbitrary one.
const OVER_DEEP_STEPS: usize = 8_000;

/// **The construction-bounded depth invariant, on the draft's route.** A command vector
/// nesting past `MAX_DURABLE_DEPTH` is refused where the rows are made, so no route out of
/// the draft — a member walk, a projected site path, a contract identity — ever meets a
/// node deeper than a semantic path can name.
#[test]
fn a_draft_refuses_an_over_deep_command_vector() {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });

    assert!(
        draft
            .declare_product(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                record,
                nested_groups(OVER_DEEP_STEPS),
            )
            .is_err(),
        "a command vector nesting {OVER_DEEP_STEPS} deep is refused at construction",
    );
    assert!(
        draft
            .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
            .is_none(),
        "the refused declaration appended no row",
    );
}

/// **The same invariant on the durable graph's own route.** The independent verifier
/// builds through this owner rather than through a draft, so the bound belongs to the rows
/// and not to one of the two owners that make them.
#[test]
fn a_durable_graph_refuses_an_over_deep_command_vector() {
    let mut graph = DurableContractGraph::new();

    assert_eq!(
        graph.declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            TypeId::from_index(0),
            nested_groups(OVER_DEEP_STEPS),
        ),
        Err(DurableGraphInputRefusal::OverDepth),
        "a command vector nesting {OVER_DEEP_STEPS} deep is refused at construction",
    );
    assert!(
        graph.contract_view().semantic_nodes().is_empty(),
        "the refused declaration appended no row",
    );
}

/// The boundary itself: a body at `MAX_DURABLE_DEPTH` is admitted and one level past it is
/// refused, on both routes and whether or not the deepest body declares a member.
#[test]
fn the_member_nesting_bound_admits_its_own_depth_and_refuses_one_more() {
    for (depth, admitted) in [(MAX_DURABLE_DEPTH, true), (MAX_DURABLE_DEPTH + 1, false)] {
        let mut draft = ImageDraft::new();
        let name = draft.intern_string("R");
        let record = draft.add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        });
        assert_eq!(
            draft
                .declare_product(
                    &admitted_plan(),
                    LedgerIdBytes::from_bytes(PRODUCT_ID),
                    record,
                    nested_groups(depth),
                )
                .is_ok(),
            admitted,
            "a draft admits a body at depth {depth}: {admitted}",
        );

        let mut graph = DurableContractGraph::new();
        assert_eq!(
            graph
                .declare_product(
                    &admitted_plan(),
                    LedgerIdBytes::from_bytes(PRODUCT_ID),
                    TypeId::from_index(0),
                    nested_groups(depth),
                )
                .is_ok(),
            admitted,
            "the durable graph admits a body at depth {depth}: {admitted}",
        );
    }
}
