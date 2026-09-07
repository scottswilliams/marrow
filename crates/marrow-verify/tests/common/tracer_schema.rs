//! The tracer durable schema every verifier test binary builds its images over: a
//! `Counter { value:int required, label:string sparse }` at `^counters(name:string)`,
//! its fixed ledger ids, and the small encoding helpers the pins share. Reached as a
//! `#[path]` module beside `admitted`.

use marrow_image::{
    DeclarationMember, DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef,
    FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, PlannedSiteRef,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, TypeId, ValueShapeNodeId,
};
use marrow_verify::verify;

use super::admitted_plan::admitted_plan;
use super::site_seam::site;

/// One within-domain draft mint, unwrapped: every fixture mint here is far inside
/// the checked carrier domain.
pub fn ok<T>(minted: Result<T, marrow_image::DraftStateError>) -> T {
    minted.expect("a within-domain mint")
}

/// The tracer graph's fixed ledger ids, shared by the durable-schema builders and
/// the byte-forgery helpers so a hostile mutation can target one precisely.
pub const APPLICATION_ID: [u8; 16] = [0x0a; 16];
pub const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
pub const ROOT_KEY_ID: [u8; 16] = [0x0c; 16];
pub const PRODUCT_ID: [u8; 16] = [0x0d; 16];
pub const VALUE_FIELD_ID: [u8; 16] = [0x0e; 16];
pub const LABEL_FIELD_ID: [u8; 16] = [0x0f; 16];

/// The direct members of the Product every fixture in this file declares, in
/// declaration order.
pub fn product_members(draft: &ImageDraft) -> Vec<DeclarationMember> {
    draft
        .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
        .expect("the fixture Product is declared")
}

/// One flat declaration command for a stored scalar field of `parent` (`None` is a
/// direct member of the Product).
pub fn field_member(
    shapes: ScalarShapes,
    parent: Option<u32>,
    id: [u8; 16],
    required: bool,
    scalar: Scalar,
) -> DeclarationMemberDef {
    DeclarationMemberDef {
        parent,
        shape: DeclarationMemberShape::Field {
            id: LedgerIdBytes::from_bytes(id),
            required,
            value: shapes.of(scalar),
        },
    }
}

/// The bare scalar value shapes of one draft's arena.
///
/// A member row references a value shape rather than owning one, so a fixture mints the
/// closed scalar set into its draft first and then states its members. Minting is
/// interning, so this is idempotent and every fixture of one draft shares the same ids.
#[derive(Clone, Copy)]
pub struct ScalarShapes {
    pub int: ValueShapeNodeId,
    pub text: ValueShapeNodeId,
    pub bool_: ValueShapeNodeId,
    pub bytes: ValueShapeNodeId,
    pub date: ValueShapeNodeId,
    pub instant: ValueShapeNodeId,
    pub duration: ValueShapeNodeId,
}

impl ScalarShapes {
    pub fn of(self, scalar: Scalar) -> ValueShapeNodeId {
        match scalar {
            Scalar::Int => self.int,
            Scalar::Text => self.text,
            Scalar::Bool => self.bool_,
            Scalar::Bytes => self.bytes,
            Scalar::Date => self.date,
            Scalar::Instant => self.instant,
            Scalar::Duration => self.duration,
        }
    }
}

pub fn scalar_shapes(draft: &mut DraftTxn<'_>) -> ScalarShapes {
    ScalarShapes {
        int: draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints"),
        text: draft
            .value_scalar(Scalar::Text)
            .expect("the test arena mints"),
        bool_: draft
            .value_scalar(Scalar::Bool)
            .expect("the test arena mints"),
        bytes: draft
            .value_scalar(Scalar::Bytes)
            .expect("the test arena mints"),
        date: draft
            .value_scalar(Scalar::Date)
            .expect("the test arena mints"),
        instant: draft
            .value_scalar(Scalar::Instant)
            .expect("the test arena mints"),
        duration: draft
            .value_scalar(Scalar::Duration)
            .expect("the test arena mints"),
    }
}

/// The tracer `Counter` record's declaration commands: `value:int` required then
/// `label:string` sparse, matching the `durable_schema` record fields so the
/// verifier's member-tree/record cross-check passes.
pub fn counters_members(shapes: ScalarShapes) -> Vec<DeclarationMemberDef> {
    vec![
        field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Int),
        field_member(shapes, None, LABEL_FIELD_ID, false, Scalar::Text),
    ]
}

pub fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

pub fn code_of(bytes: &[u8]) -> String {
    verify(bytes)
        .err()
        .map(|r| r.code().to_string())
        .unwrap_or_else(|| "VERIFIED".to_string())
}

/// The tracer schema's three durable operation sites. A site operand is minted only by
/// [`ImageDraft::request_site`], so a test names one of these sites by threading the
/// operand its own draft returned; there is no way to write a site number by hand.
pub struct Sites {
    pub record: TypeId,
    /// The root entry's whole-payload site.
    pub entry: PlannedSiteRef,
    /// The required `value:int` field leaf.
    pub value: PlannedSiteRef,
    /// The sparse `label:string` field leaf.
    pub label: PlannedSiteRef,
}

/// Build the tracer-like durable schema into `draft`: a `Counter { value:int
/// required, label:string sparse }` at root `^counters(name:string)`, returning the
/// entry, required-field, and sparse-field site operands.
pub fn durable_schema(draft: &mut DraftTxn<'_>) -> Sites {
    durable_schema_with_keys(
        draft,
        vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
        }],
    )
}

pub fn durable_schema_with_keys(draft: &mut DraftTxn<'_>, keys: Vec<KeyColumn>) -> Sites {
    let counter = ok(draft.intern_string("Counter"));
    let value = ok(draft.intern_string("value"));
    let label = ok(draft.intern_string("label"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    }));
    let root = ok(draft.intern_string("counters"));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let shapes = scalar_shapes(draft);
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            counters_members(shapes),
        )
        .expect("a well-formed declaration");
    let admitted = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys,
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let members = product_members(draft);
    let entry = site(
        draft,
        admitted.occurrence(),
        admitted.placement_path(),
        SemanticTarget::WholePayload,
    );
    let value = site(
        draft,
        admitted.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let label = site(
        draft,
        admitted.occurrence(),
        members[1].path(),
        SemanticTarget::FieldLeaf,
    );
    Sites {
        record,
        entry,
        value,
        label,
    }
}

/// Add a single mutating export with two `string` key params (slots 0 and 1) over
/// the tracer schema in `draft`, whose body is `code`, and encode it. Used by the
/// presence-lattice hostiles, where the guard proves one slot and the strict set
/// names a slot. The caller interns any consts in the same draft first.
pub fn finish_two_key(mut draft: DraftTxn<'_>, code: Vec<Instr>) -> Vec<u8> {
    let src = ok(draft.intern_string("src/main.mw"));
    let name = ok(draft.intern_string("put"));
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: vec![
                ImageType::scalar(Scalar::Text),
                ImageType::scalar(Scalar::Text),
            ],
            ret: ImageType::Unit,
            local_count: 2,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}
