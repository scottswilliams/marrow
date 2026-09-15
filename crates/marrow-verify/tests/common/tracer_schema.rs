//! The durable fixtures every verifier test binary builds its images over, with one
//! owner each: the tracer `Counter { value:int required, label:string sparse }` at
//! `^counters(name:string)` and its indexed and group/branch variants, the fixed ledger
//! ids a hostile mutation targets, the function builder that mints every fixture's
//! `FunctionDef`, and the typed verdict pins compare. Reached as a `#[path]` module
//! beside `admitted`.

use marrow_image::{
    AdmittedRoot, DeclarationMember, DeclarationMemberDef, DeclarationMemberShape, DraftTxn,
    DurableIndexComponent, DurableIndexShape, ExportId, FieldDef, FuncId, FunctionDef, ImageDraft,
    ImageType, Instr, KeyColumn, LedgerIdBytes, PlannedSiteRef, RecordTypeDef, RootOccurrenceDef,
    Scalar, SemanticTarget, SpanEntry, TypeId, ValueShapeNodeId,
};
use marrow_verify::{VerifyPhase, verify};

use super::admitted_helper::admitted;
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

/// The verifier's answer for one image: it verified, or the phase that owns the violated
/// invariant refused it. Pins compare this typed verdict, not a rendered `image.*` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    Refused(VerifyPhase),
}

pub fn verdict_of(bytes: &[u8]) -> Verdict {
    match verify(bytes) {
        Ok(_) => Verdict::Verified,
        Err(rejection) => Verdict::Refused(rejection.phase()),
    }
}

/// Add a function to `draft`. Every fixture mints its functions here, so a `FunctionDef`
/// literal — and the source path and per-instruction spans every fixture shares — has one
/// spelling. `local_count` is stated rather than derived: a body may hold scratch locals
/// beyond its parameters.
pub fn add_fn(
    draft: &mut DraftTxn<'_>,
    name: &str,
    params: Vec<ImageType>,
    ret: ImageType,
    local_count: u16,
    code: Vec<Instr>,
) -> FuncId {
    let source = ok(draft.intern_string("src/main.mw"));
    let name = ok(draft.intern_string(name));
    draft
        .add_function(FunctionDef {
            name,
            source,
            params,
            ret,
            local_count,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live")
}

/// A no-parameter `int`-returning function: the pure filler a fixture adds when its point
/// is the image's tables rather than its code.
pub fn add_int_fn(draft: &mut DraftTxn<'_>, name: &str, code: Vec<Instr>) -> FuncId {
    add_fn(
        draft,
        name,
        Vec::new(),
        ImageType::scalar(Scalar::Int),
        0,
        code,
    )
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

/// The tracer schema plus one mutating export `put(k:string, v:int)` whose body is what
/// `code` builds from that schema's site operands.
pub fn put_export(code: impl FnOnce(&Sites) -> Vec<Instr>) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let code = code(&sites);
    let func = add_fn(
        &mut draft,
        "put",
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ImageType::Unit,
        2,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.commit();
    draft_owner
}

/// The fixed ids of the indexed tracer graph's two managed indexes.
pub const BY_LABEL_INDEX_ID: [u8; 16] = [0x70; 16];
pub const BY_VALUE_INDEX_ID: [u8; 16] = [0x71; 16];

/// The well-formed nonunique `byLabel` projection: the sparse `label` field then the
/// identity key, the complete-suffix shape a nonunique index requires.
pub fn by_label_projection() -> Vec<DurableIndexComponent> {
    vec![
        DurableIndexComponent::Field(LedgerIdBytes::from_bytes(LABEL_FIELD_ID)),
        DurableIndexComponent::Key(LedgerIdBytes::from_bytes(ROOT_KEY_ID)),
    ]
}

/// The well-formed unique `byValue` projection: the single `value` scalar field, which a
/// unique index may carry without the identity suffix.
pub fn by_value_projection() -> Vec<DurableIndexComponent> {
    vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
        VALUE_FIELD_ID,
    ))]
}

/// The tracer counters root plus a nonunique `byLabel(label, k)` and a unique
/// `byValue(value)`. `by_label_components` overrides the first index's projection so a
/// hostile test can malform it while the unique `byValue` stays well formed.
pub fn indexed_draft(
    by_label_components: Vec<DurableIndexComponent>,
) -> (ImageDraft, AdmittedRoot) {
    indexed_draft_full(by_label_components, by_value_projection())
}

/// [`indexed_draft`] with both projections overridable, so a hostile test can malform
/// either the nonunique or the unique index.
pub fn indexed_draft_full(
    by_label_components: Vec<DurableIndexComponent>,
    by_value_components: Vec<DurableIndexComponent>,
) -> (ImageDraft, AdmittedRoot) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
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
    let shapes = scalar_shapes(&mut draft);
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
                keys: vec![KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: vec![
                    DurableIndexShape {
                        id: LedgerIdBytes::from_bytes(BY_LABEL_INDEX_ID),
                        unique: false,
                        components: by_label_components,
                    },
                    DurableIndexShape {
                        id: LedgerIdBytes::from_bytes(BY_VALUE_INDEX_ID),
                        unique: true,
                        components: by_value_components,
                    },
                ]
                .into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    (draft_owner, admitted)
}

/// A `Book { title:string required }` root at `^books(id:int)` whose durable member tree
/// adds a static `details` group (holding `pages:int`) and a keyed `notes(noteId:string)`
/// branch (holding `text:string required`). The record carries only the top-level `title`
/// field plus the group's record slot, so the record/member-tree cross-check passes. When
/// `with_site` is true a field site and a reading export are added; otherwise a pure
/// filler function completes the image.
pub fn group_branch_draft(with_site: bool) -> (ImageDraft, AdmittedRoot) {
    group_branch_draft_with_branch_record(with_site, true)
}

/// As [`group_branch_draft`], but the branch's materialized record marks its `text` field
/// with `branch_record_required`. The branch *member* always marks `text` required, so
/// `false` builds an image whose branch record disagrees with its member fields.
pub fn group_branch_draft_with_branch_record(
    with_site: bool,
    branch_record_required: bool,
) -> (ImageDraft, AdmittedRoot) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let shapes = scalar_shapes(&mut draft);
    let book = ok(draft.intern_string("Book"));
    let title = ok(draft.intern_string("title"));
    // The `details` group's own leaf record, referenced by the root record's trailing
    // group slot; its `pages` leaf ties to the group member's direct field.
    let details_qualified = ok(draft.intern_string("Book.details"));
    let details_pages = ok(draft.intern_string("pages"));
    let details_record = ok(draft.add_record_type(RecordTypeDef {
        name: details_qualified,
        fields: vec![FieldDef {
            name: details_pages,
            ty: ImageType::scalar(Scalar::Int),
            required: false,
        }],
    }));
    let details = ok(draft.intern_string("details"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![
            FieldDef {
                name: title,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
            FieldDef {
                name: details,
                ty: ImageType::Record {
                    idx: details_record,
                    optional: false,
                },
                required: true,
            },
        ],
    }));
    let root = ok(draft.intern_string("books"));
    let notes = ok(draft.intern_string("notes"));
    let notes_qualified = ok(draft.intern_string("Book.notes"));
    let notes_text = ok(draft.intern_string("text"));
    let notes_record = ok(draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: branch_record_required,
        }],
    }));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![
                field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Text),
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Group {
                        id: LedgerIdBytes::from_bytes(GROUP_ID),
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes(BRANCH_PLACEMENT_ID),
                        name: notes,
                        record: notes_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Text,
                            id: LedgerIdBytes::from_bytes(BRANCH_KEY_ID),
                        }],
                    },
                },
                field_member(shapes, Some(1), GROUP_FIELD_ID, false, Scalar::Int),
                field_member(shapes, Some(2), BRANCH_FIELD_ID, true, Scalar::Text),
            ],
        )
        .expect("a well-formed declaration");
    let admitted = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    if with_site {
        let site = book_title_site(&mut draft, &admitted);
        let code = vec![Instr::LocalGet(0), Instr::DurReadField(site), Instr::Return];
        let func = add_fn(
            &mut draft,
            "read",
            vec![ImageType::scalar(Scalar::Int)],
            ImageType::opt_scalar(Scalar::Text),
            1,
            code,
        );
        draft.add_export(ExportId::of_local("", "read"), func);
    } else {
        let zero = ok(draft.intern_int(0));
        let func = add_int_fn(
            &mut draft,
            "label",
            vec![Instr::ConstLoad(zero), Instr::Return],
        );
        draft.add_export(ExportId::of_local("", "label"), func);
    }
    draft.commit();
    (draft_owner, admitted)
}

/// The fixed ids of the group/branch graph's non-root nodes.
pub const GROUP_ID: [u8; 16] = [0x20; 16];
pub const GROUP_FIELD_ID: [u8; 16] = [0x21; 16];
pub const BRANCH_PLACEMENT_ID: [u8; 16] = [0x30; 16];
pub const BRANCH_KEY_ID: [u8; 16] = [0x31; 16];
pub const BRANCH_FIELD_ID: [u8; 16] = [0x32; 16];

/// The group/branch graph's root Product declares `[title field, details group, notes
/// branch]`, so these name its addressable nodes: the root's own `title` field, the whole
/// `details` group node, that group's `pages` field leaf, the `notes` branch entry, and
/// that branch's `text` field leaf. Each binds the one target its node kind admits.
pub fn book_title_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let members = product_members(draft);
    site(
        draft,
        root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    )
}

pub fn book_group_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let members = product_members(draft);
    site(
        draft,
        root.occurrence(),
        members[1].path(),
        SemanticTarget::GroupEntry,
    )
}

pub fn book_group_field_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let group = product_members(draft)[1].path().clone();
    let pages = draft
        .members_of(&group)
        .expect("the declaration row is live")[0]
        .path()
        .clone();
    site(draft, root.occurrence(), &pages, SemanticTarget::FieldLeaf)
}

pub fn book_branch_entry_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let members = product_members(draft);
    site(
        draft,
        root.occurrence(),
        members[2].path(),
        SemanticTarget::WholePayload,
    )
}

pub fn book_branch_field_site(draft: &mut DraftTxn<'_>, root: &AdmittedRoot) -> PlannedSiteRef {
    let branch = product_members(draft)[2].path().clone();
    let text = draft
        .members_of(&branch)
        .expect("the declaration row is live")[0]
        .path()
        .clone();
    site(draft, root.occurrence(), &text, SemanticTarget::FieldLeaf)
}
