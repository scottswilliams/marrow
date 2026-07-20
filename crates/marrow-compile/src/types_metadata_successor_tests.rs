use super::*;

use crate::durable::DurableRegistry;
use marrow_image::{CollectionTypeDef, EnumTypeDef, FieldDef, ImageType, RecordTypeDef, Scalar};
use marrow_project::IdentityLedger;

fn name(text: &str) -> TypeExpr {
    TypeExpr::Name {
        text: text.to_string(),
        span: SourceSpan::default(),
    }
}

fn struct_template(template_name: &str, params: &[&str]) -> TypeTemplate {
    TypeTemplate {
        name: template_name.to_string(),
        file: "src/main.mw".to_string(),
        name_span: SourceSpan::default(),
        reserved: None,
        type_params: params
            .iter()
            .map(|param| ((*param).to_string(), None))
            .collect(),
        body: TemplateBody::Struct(
            params
                .iter()
                .map(|param| (param.to_ascii_lowercase(), name(param)))
                .collect(),
        ),
    }
}

fn enum_template(template_name: &str, param: &str) -> TypeTemplate {
    TypeTemplate {
        name: template_name.to_string(),
        file: "src/main.mw".to_string(),
        name_span: SourceSpan::default(),
        reserved: None,
        type_params: vec![(param.to_string(), None)],
        body: TemplateBody::Enum(vec![TemplateVariant {
            name: "item".to_string(),
            payload: vec![TemplatePayload {
                name: "value".to_string(),
                ty: name(param),
            }],
        }]),
    }
}

fn test_registry(templates: Vec<TypeTemplate>) -> TypeRegistry {
    TypeRegistry {
        aliases: BTreeMap::new(),
        nominals: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        records: Vec::new(),
        type_templates: templates,
        generics: RefCell::default(),
        collections: RefCell::default(),
    }
}

fn site() -> MintSite<'static> {
    MintSite {
        file: "src/main.mw",
        span: SourceSpan {
            line: 3,
            column: 9,
            ..SourceSpan::default()
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RowState {
    Filling(Option<BodySnapshot>),
    Ready(BodySnapshot),
    Rejected(ResolveRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodySnapshot {
    Struct(Vec<(String, GArg)>),
    Enum(Vec<(String, Vec<(String, GArg)>)>),
}

fn body_snapshot(body: &InstBody) -> BodySnapshot {
    match body {
        InstBody::Struct(fields) => BodySnapshot::Struct(fields.clone()),
        InstBody::Enum(variants) => BodySnapshot::Enum(
            variants
                .iter()
                .map(|variant| (variant.name.clone(), variant.payload.clone()))
                .collect(),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RowSnapshot {
    template: usize,
    args: Vec<GArg>,
    id: TypeInstId,
    state: RowState,
    dependents: Vec<usize>,
}

#[derive(Debug, PartialEq, Eq)]
struct OwnerSnapshot {
    rows: Vec<RowSnapshot>,
    collections: Vec<CollSpec>,
    fn_base: u16,
    functions: Vec<(usize, Vec<GArg>, u16)>,
    queue: Vec<(usize, Vec<GArg>, u16)>,
    batch_start: Option<usize>,
    fill_rows: Vec<(TypeInstKey, usize)>,
    fill_stack: Vec<usize>,
    fill_failures: Vec<(usize, ResolveRefusal)>,
    limit: u8,
    payload_count: usize,
}

fn owner_snapshot(registry: &TypeRegistry) -> OwnerSnapshot {
    let generics = registry.generics.borrow();
    OwnerSnapshot {
        rows: generics
            .type_insts
            .iter()
            .map(|inst| {
                let state = match &inst.state {
                    TypeInstState::Filling { staged } => {
                        RowState::Filling(staged.as_ref().map(body_snapshot))
                    }
                    TypeInstState::Ready(body) => RowState::Ready(body_snapshot(body)),
                    TypeInstState::Rejected(refusal) => RowState::Rejected(*refusal),
                };
                RowSnapshot {
                    template: inst.template,
                    args: inst.args.clone(),
                    id: inst.id,
                    state,
                    dependents: inst.dependents.clone(),
                }
            })
            .collect(),
        collections: registry.collections.borrow().clone(),
        fn_base: generics.fn_base,
        functions: generics
            .fn_insts
            .iter()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
            .collect(),
        queue: generics
            .fn_queue
            .iter()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
            .collect(),
        batch_start: generics.fill_batch_start,
        fill_rows: generics
            .fill_rows
            .iter()
            .map(|(key, row)| (*key, *row))
            .collect(),
        fill_stack: generics.fill_stack.clone(),
        fill_failures: generics.fill_failures.clone(),
        limit: match generics.limit {
            LimitState::Open => 0,
            LimitState::Pending(_) => 1,
            LimitState::Reported => 2,
        },
        payload_count: generics.collection_payloads.len(),
    }
}

fn draft_fingerprint(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
    let encoded = draft.encode().expect("test draft encodes");
    (encoded.bytes, encoded.image_id)
}

trait HiddenOutcome {
    fn hidden(self) -> bool;
}

impl<T> HiddenOutcome for Option<T> {
    fn hidden(self) -> bool {
        self.is_none()
    }
}

impl<T, E> HiddenOutcome for Result<T, E> {
    fn hidden(self) -> bool {
        self.is_err()
    }
}

trait CollectionIndexOutcome {
    fn into_index(self) -> Option<u16>;
}

impl CollectionIndexOutcome for u16 {
    fn into_index(self) -> Option<u16> {
        Some(self)
    }
}

impl<E> CollectionIndexOutcome for Result<u16, E> {
    fn into_index(self) -> Option<u16> {
        self.ok()
    }
}

trait TemplateIndexOutcome {
    fn into_template(self) -> Option<usize>;
}

impl TemplateIndexOutcome for usize {
    fn into_template(self) -> Option<usize> {
        Some(self)
    }
}

impl<E> TemplateIndexOutcome for Result<usize, E> {
    fn into_template(self) -> Option<usize> {
        self.ok()
    }
}

trait AnchorOutcome {
    fn into_anchor(self) -> Option<String>;
}

impl AnchorOutcome for Option<String> {
    fn into_anchor(self) -> Option<String> {
        self
    }
}

impl<E> AnchorOutcome for Result<String, E> {
    fn into_anchor(self) -> Option<String> {
        self.ok()
    }
}

impl<E> AnchorOutcome for Result<Option<String>, E> {
    fn into_anchor(self) -> Option<String> {
        self.ok().flatten()
    }
}

trait GraphOutcome {
    fn into_graph(self) -> Option<ValueGraph>;
}

impl GraphOutcome for ValueGraph {
    fn into_graph(self) -> Option<ValueGraph> {
        Some(self)
    }
}

impl<E> GraphOutcome for Result<ValueGraph, E> {
    fn into_graph(self) -> Option<ValueGraph> {
        self.ok()
    }
}

fn mint_ready(registry: &TypeRegistry, draft: &mut ImageDraft, template: usize) -> TypeInstId {
    registry
        .mint_type_instance(draft, template, &[GArg::Scalar(ScalarType::Int)], site())
        .expect("control instantiation is Ready")
}

#[test]
fn missing_template_index_is_rejected_before_minting_owner_state() {
    let registry = test_registry(Vec::new());
    let mut draft = ImageDraft::new();
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);

    let result =
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site());

    assert!(result.is_err());
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn wrong_argument_counts_are_rejected_before_minting_owner_state() {
    let mut observations = Vec::new();
    for args in [
        Vec::new(),
        vec![
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Bool),
            GArg::Scalar(ScalarType::Text),
        ],
    ] {
        let registry = test_registry(vec![struct_template("Pair", &["T", "U"])]);
        let mut draft = ImageDraft::new();
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);
        let result = registry.mint_type_instance(&mut draft, 0, &args, site());
        observations.push(
            result.is_err()
                && owner_snapshot(&registry) == owner_before
                && draft_fingerprint(&draft) == draft_before,
        );
    }

    assert_eq!(observations, vec![true, true]);
}

#[test]
fn malformed_ready_metadata_is_hidden_from_semantic_readers() {
    let mut observations = Vec::new();

    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let missing_template = mint_ready(&registry, &mut draft, 0);
    registry.generics.borrow_mut().type_insts[0].template = usize::MAX;
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let instantiation_hidden = registry.instantiation_of(missing_template).hidden();
    let body_hidden = registry.type_inst_body(missing_template).hidden();
    observations.push(
        instantiation_hidden
            && body_hidden
            && owner_snapshot(&registry) == owner_before
            && draft_fingerprint(&draft) == draft_before,
    );

    for args in [
        Vec::new(),
        vec![
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Bool),
        ],
    ] {
        let registry = test_registry(vec![struct_template("Box", &["T"])]);
        let mut draft = ImageDraft::new();
        let id = mint_ready(&registry, &mut draft, 0);
        registry.generics.borrow_mut().type_insts[0].args = args;
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);
        let instantiation_hidden = registry.instantiation_of(id).hidden();
        let body_hidden = registry.type_inst_body(id).hidden();
        let anchor_hidden = registry.inst_anchor_spelling(id).hidden();
        let graph_hidden = ValueGraph::build(&registry).into_graph().is_none();
        observations.push(
            instantiation_hidden
                && body_hidden
                && anchor_hidden
                && graph_hidden
                && owner_snapshot(&registry) == owner_before
                && draft_fingerprint(&draft) == draft_before,
        );
    }

    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let id = mint_ready(&registry, &mut draft, 0);
    let orphan_name = draft.intern_string("OrphanStruct");
    let orphan = draft.add_record_type(RecordTypeDef {
        name: orphan_name,
        fields: Vec::new(),
    });
    registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Struct(orphan)];
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let instantiation_hidden = registry.instantiation_of(id).hidden();
    let body_hidden = registry.type_inst_body(id).hidden();
    let anchor_hidden = registry.inst_anchor_spelling(id).hidden();
    let graph_hidden = ValueGraph::build(&registry).into_graph().is_none();
    observations.push(
        instantiation_hidden
            && body_hidden
            && anchor_hidden
            && graph_hidden
            && owner_snapshot(&registry) == owner_before
            && draft_fingerprint(&draft) == draft_before,
    );

    assert_eq!(observations, vec![true; 4]);
}

#[test]
fn missing_template_ready_metadata_is_hidden_from_anchor_spelling() {
    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let id = mint_ready(&registry, &mut draft, 0);
    registry.generics.borrow_mut().type_insts[0].template = usize::MAX;
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);

    let hidden = registry.inst_anchor_spelling(id).hidden();

    assert!(hidden);
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn missing_template_ready_metadata_is_rejected_by_value_graph() {
    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let _ = mint_ready(&registry, &mut draft, 0);
    registry.generics.borrow_mut().type_insts[0].template = usize::MAX;
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);

    let hidden = ValueGraph::build(&registry).into_graph().is_none();

    assert!(hidden);
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn malformed_reserved_option_ready_metadata_is_hidden_from_all_readers() {
    let (registry, mut draft) = reserved_registry();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site())
        .expect("control Option is Ready");
    let orphan_name = draft.intern_string("OrphanStruct");
    let orphan = draft.add_record_type(RecordTypeDef {
        name: orphan_name,
        fields: Vec::new(),
    });
    registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Struct(orphan)];
    let id = TypeInstId::Enum(option);
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);

    let instantiation_hidden = registry.instantiation_of(id).hidden();
    let body_hidden = registry.type_inst_body(id).hidden();
    let option_hidden = registry.as_option(option).hidden();
    let variants_hidden = registry.enum_variants(option).hidden();
    let anchor_hidden = registry.enum_anchor_spelling(option).hidden();
    let graph_hidden = ValueGraph::build(&registry).into_graph().is_none();

    assert!(instantiation_hidden);
    assert!(body_hidden);
    assert!(option_hidden);
    assert!(variants_hidden);
    assert!(anchor_hidden);
    assert!(graph_hidden);
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn durable_build_uses_one_metadata_session_across_stores_and_repeated_enum_anchors() {
    const IDS: &str = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 01010101010101010101010101010101\n\
         id product First 02020202020202020202020202020202\n\
         id field First.value 03030303030303030303030303030303\n\
         id root first 04040404040404040404040404040404\n\
         id key first.id 05050505050505050505050505050505\n\
         id product Second 06060606060606060606060606060606\n\
         id field Second.value 07070707070707070707070707070707\n\
         id root second 08080808080808080808080808080808\n\
         id key second.id 09090909090909090909090909090909\n\
         id sum Result[Option[int],Option[int]] 10101010101010101010101010101010\n\
         id member Result[Option[int],Option[int]].ok 11111111111111111111111111111111\n\
         id member Result[Option[int],Option[int]].err 12121212121212121212121212121212\n\
         id sum Option[int] 13131313131313131313131313131313\n\
         id member Option[int].none 14141414141414141414141414141414\n\
         id member Option[int].some 15151515151515151515151515151515\n\
         high-water 0\n\
         end\n";

    let parsed = marrow_syntax::parse_source(
        r#"module main

resource First {
    required value: Result<Option<int>, Option<int>>
}

resource Second {
    required value: Result<Option<int>, Option<int>>
}

store ^first[id: int]: First
store ^second[id: int]: Second
"#,
    );
    assert!(parsed.diagnostics.is_empty());
    let resources = vec![
        (
            "src/main.mw".to_string(),
            parsed.file.resource("First").expect("First exists"),
        ),
        (
            "src/main.mw".to_string(),
            parsed.file.resource("Second").expect("Second exists"),
        ),
    ];
    let stores = vec![
        (
            "src/main.mw".to_string(),
            parsed.file.store("first").expect("first exists"),
        ),
        (
            "src/main.mw".to_string(),
            parsed.file.store("second").expect("second exists"),
        ),
    ];
    let mut draft = ImageDraft::new();
    let mut diagnostics = Vec::new();
    let records = TypeRegistry::build(&mut draft, &[], &[], &[], &[], &resources, &mut diagnostics);
    assert!(diagnostics.is_empty());
    let shared = records.by_name("First").expect("First record").fields[0].ty;
    assert!(matches!(shared, GArg::Enum(_)));
    assert_eq!(
        records.by_name("Second").expect("Second record").fields[0].ty,
        shared,
        "both stores reuse the same Ready Result instantiation"
    );
    let ledger = IdentityLedger::parse(IDS.as_bytes()).expect("fixture ledger parses");

    let (outcome, builds) = count_metadata_directory_builds(|| {
        DurableRegistry::build(
            &mut draft,
            &records,
            &resources,
            &stores,
            Some(&ledger),
            &mut diagnostics,
        )
    });

    assert_eq!(builds, 1, "one metadata session spans the complete build");
    let durable = outcome.expect("valid durable registry builds");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    for root_name in ["first", "second"] {
        assert_eq!(
            durable
                .root_by_name(root_name)
                .and_then(|root| root.field("value"))
                .map(|field| field.ty),
            Some(shared),
            "{root_name} reaches the repeated enum shape"
        );
    }
}

#[test]
fn invalid_ready_option_argument_stops_before_durable_anchor_resolution() {
    const IDS: &str = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Resource 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id field Resource.value 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id root resources 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id key resources.id 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         high-water 0\n\
         end\n";

    let parsed = marrow_syntax::parse_source(
        "module main\n\nresource Resource {\n    required value: Option<int>\n}\n\nstore ^resources[id: int]: Resource\n",
    );
    assert!(parsed.diagnostics.is_empty());
    let resource = parsed
        .file
        .resource("Resource")
        .expect("fixture resource exists");
    let store = parsed
        .file
        .store("resources")
        .expect("fixture store exists");
    let resources = vec![("src/main.mw".to_string(), resource)];
    let stores = vec![("src/main.mw".to_string(), store)];
    let mut draft = ImageDraft::new();
    let mut diagnostics = Vec::new();
    let registry =
        TypeRegistry::build(&mut draft, &[], &[], &[], &[], &resources, &mut diagnostics);
    assert!(diagnostics.is_empty());
    let option = match registry
        .by_name("Resource")
        .expect("resource record exists")
        .fields[0]
        .ty
    {
        GArg::Enum(option) => option,
        _ => panic!("resource field is Option-shaped"),
    };
    let orphan_name = draft.intern_string("OrphanStruct");
    let orphan = draft.add_record_type(RecordTypeDef {
        name: orphan_name,
        fields: Vec::new(),
    });
    registry
        .generics
        .borrow_mut()
        .type_insts
        .iter_mut()
        .find(|inst| inst.id == TypeInstId::Enum(option))
        .expect("Ready Option row exists")
        .args = vec![GArg::Struct(orphan)];
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let ledger = IdentityLedger::parse(IDS.as_bytes()).expect("fixture ledger parses");

    let outcome = DurableRegistry::build(
        &mut draft,
        &registry,
        &resources,
        &stores,
        Some(&ledger),
        &mut diagnostics,
    );

    assert!(matches!(
        outcome,
        Err(GenericInvariant::TypeArgumentTargetMissing(target))
            if target == GArg::Struct(orphan)
    ));
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
    assert!(diagnostics.is_empty());
}

fn reserved_registry() -> (TypeRegistry, ImageDraft) {
    let mut draft = ImageDraft::new();
    let mut diagnostics = Vec::new();
    let registry = TypeRegistry::build(&mut draft, &[], &[], &[], &[], &[], &mut diagnostics);
    assert!(diagnostics.is_empty());
    (registry, draft)
}

#[test]
fn reserved_option_and_result_require_exact_argument_slices() {
    let mut observations = Vec::new();

    for args in [
        Vec::new(),
        vec![
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Bool),
        ],
    ] {
        let (registry, mut draft) = reserved_registry();
        let option = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site())
            .expect("control Option is Ready");
        registry.generics.borrow_mut().type_insts[0].args = args;
        observations.push(registry.as_option(option).hidden());
    }

    for args in [
        vec![GArg::Scalar(ScalarType::Int)],
        vec![
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Bool),
            GArg::Scalar(ScalarType::Text),
        ],
    ] {
        let (registry, mut draft) = reserved_registry();
        let result_template = registry
            .reserved_template(Reserved::Result)
            .into_template()
            .expect("reserved Result template exists");
        let result_id = registry
            .mint_type_instance(
                &mut draft,
                result_template,
                &[
                    GArg::Scalar(ScalarType::Int),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(),
            )
            .ok()
            .and_then(|id| match id {
                TypeInstId::Enum(id) => Some(id),
                TypeInstId::Record(_) => None,
            })
            .expect("control Result is Ready");
        registry.generics.borrow_mut().type_insts[0].args = args;
        observations.push(registry.as_result(result_id).hidden());
    }

    assert_eq!(observations, vec![true, true, true, true]);
}

#[test]
fn reserved_result_valid_length_checks_both_argument_targets() {
    let mut observations = Vec::new();

    for invalid_ok in [true, false] {
        let (registry, mut draft) = reserved_registry();
        let result_template = registry
            .reserved_template(Reserved::Result)
            .into_template()
            .expect("reserved Result template exists");
        let result = registry
            .mint_type_instance(
                &mut draft,
                result_template,
                &[
                    GArg::Scalar(ScalarType::Int),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(),
            )
            .ok()
            .and_then(|id| match id {
                TypeInstId::Enum(id) => Some(id),
                TypeInstId::Record(_) => None,
            })
            .expect("control Result is Ready");
        let orphan_name = draft.intern_string("OrphanResultArgument");
        let orphan = draft.add_record_type(RecordTypeDef {
            name: orphan_name,
            fields: Vec::new(),
        });
        let args = if invalid_ok {
            vec![GArg::Struct(orphan), GArg::Scalar(ScalarType::Bool)]
        } else {
            vec![GArg::Scalar(ScalarType::Int), GArg::Struct(orphan)]
        };
        registry
            .generics
            .borrow_mut()
            .type_insts
            .iter_mut()
            .find(|inst| inst.id == TypeInstId::Enum(result))
            .expect("Ready Result row exists")
            .args = args;
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);

        observations.push(
            registry.as_result(result).hidden()
                && owner_snapshot(&registry) == owner_before
                && draft_fingerprint(&draft) == draft_before,
        );
    }

    assert_eq!(observations, vec![true, true]);
}

fn target_case_with_registry(registry: TypeRegistry, mut draft: ImageDraft, target: GArg) -> bool {
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let result = registry.mint_type_instance(&mut draft, 0, &[target], site());
    result.is_err()
        && owner_snapshot(&registry) == owner_before
        && draft_fingerprint(&draft) == draft_before
}

fn target_case(draft: ImageDraft, target: GArg) -> bool {
    target_case_with_registry(
        test_registry(vec![struct_template("Box", &["T"])]),
        draft,
        target,
    )
}

fn seed_collection(registry: &TypeRegistry, draft: &mut ImageDraft, spec: CollSpec) -> u16 {
    let def = match spec {
        CollSpec::List { elem } => CollectionTypeDef::List { elem: elem.image() },
        CollSpec::Map { key, value } => CollectionTypeDef::Map {
            key: key.image(),
            value: value.image(),
        },
    };
    let id = draft.add_collection_type(def);
    assert_eq!(id.index() as usize, registry.collections.borrow().len());
    registry.collections.borrow_mut().push(spec);
    id.index()
}

#[test]
fn missing_non_scalar_targets_are_rejected_before_ready_publication() {
    let mut record_draft = ImageDraft::new();
    let record_name = record_draft.intern_string("OrphanRecord");
    let record = record_draft.add_record_type(RecordTypeDef {
        name: record_name,
        fields: Vec::new(),
    });

    let mut enum_draft = ImageDraft::new();
    let enum_name = enum_draft.intern_string("OrphanEnum");
    let enum_id = enum_draft.add_enum_type(EnumTypeDef {
        name: enum_name,
        variants: Vec::new(),
    });

    let observations = vec![
        target_case(ImageDraft::new(), GArg::Nominal(NominalId(0))),
        target_case(record_draft.clone(), GArg::Struct(record)),
        target_case(record_draft, GArg::Group(record)),
        target_case(enum_draft, GArg::Enum(enum_id)),
        target_case(ImageDraft::new(), GArg::Param(0)),
    ];

    assert_eq!(observations, vec![true; 5]);
}

#[test]
fn missing_collection_target_is_rejected_before_table_indexing() {
    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let result = registry.mint_type_instance(&mut draft, 0, &[GArg::Collection(0)], site());

    assert!(result.is_err());
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

fn resource_target_fixture() -> (TypeRegistry, ImageDraft, TypeId, TypeId) {
    let mut draft = ImageDraft::new();
    let group_name = draft.intern_string("Details");
    let group_id = draft.add_record_type(RecordTypeDef {
        name: group_name,
        fields: Vec::new(),
    });
    let root_name = draft.intern_string("Resource");
    let root_id = draft.add_record_type(RecordTypeDef {
        name: root_name,
        fields: Vec::new(),
    });
    let mut registry = test_registry(vec![struct_template("Box", &["T"])]);
    registry.records.push(RecordInfo {
        type_id: root_id,
        name: "Resource".to_string(),
        fields: Vec::new(),
        groups: vec![GroupInfo {
            name: "details".to_string(),
            type_id: group_id,
            fields: Vec::new(),
        }],
    });
    (registry, draft, root_id, group_id)
}

#[test]
fn wrong_family_record_targets_are_rejected_before_ready_publication() {
    let (group_registry, group_draft, _, group_id) = resource_target_fixture();
    let (root_struct_registry, root_struct_draft, root_struct_id, _) = resource_target_fixture();
    let (root_group_registry, root_group_draft, root_group_id, _) = resource_target_fixture();

    let mut struct_draft = ImageDraft::new();
    let struct_name = struct_draft.intern_string("Point");
    let struct_id = struct_draft.add_record_type(RecordTypeDef {
        name: struct_name,
        fields: Vec::new(),
    });
    let mut struct_registry = test_registry(vec![struct_template("Box", &["T"])]);
    struct_registry.structs.push(StructInfo {
        type_id: struct_id,
        name: "Point".to_string(),
        fields: Vec::new(),
    });

    let generic_registry = test_registry(vec![
        struct_template("Box", &["T"]),
        struct_template("Inner", &["T"]),
    ]);
    let mut generic_draft = ImageDraft::new();
    let generic = mint_ready(&generic_registry, &mut generic_draft, 1);
    let TypeInstId::Record(generic_id) = generic else {
        panic!("Inner is a struct template")
    };

    assert!(target_case_with_registry(
        group_registry,
        group_draft,
        GArg::Struct(group_id),
    ));
    assert!(target_case_with_registry(
        root_struct_registry,
        root_struct_draft,
        GArg::Struct(root_struct_id),
    ));
    assert!(target_case_with_registry(
        root_group_registry,
        root_group_draft,
        GArg::Group(root_group_id),
    ));
    assert!(target_case_with_registry(
        struct_registry,
        struct_draft,
        GArg::Group(struct_id),
    ));
    assert!(target_case_with_registry(
        generic_registry,
        generic_draft,
        GArg::Group(generic_id),
    ));
}

#[test]
fn in_range_collections_recursively_validate_every_nested_target() {
    let mut observations = Vec::new();

    let list_registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut list_draft = ImageDraft::new();
    let orphan_name = list_draft.intern_string("OrphanStruct");
    let orphan = list_draft.add_record_type(RecordTypeDef {
        name: orphan_name,
        fields: Vec::new(),
    });
    let list = seed_collection(
        &list_registry,
        &mut list_draft,
        CollSpec::List {
            elem: GArg::Struct(orphan),
        },
    );
    observations.push(target_case_with_registry(
        list_registry,
        list_draft,
        GArg::Collection(list),
    ));

    let map_registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut map_draft = ImageDraft::new();
    let map = seed_collection(
        &map_registry,
        &mut map_draft,
        CollSpec::Map {
            key: GArg::Nominal(NominalId(0)),
            value: GArg::Scalar(ScalarType::Bool),
        },
    );
    observations.push(target_case_with_registry(
        map_registry,
        map_draft,
        GArg::Collection(map),
    ));

    let nested_registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut nested_draft = ImageDraft::new();
    let orphan_name = nested_draft.intern_string("OrphanEnum");
    let orphan = nested_draft.add_enum_type(EnumTypeDef {
        name: orphan_name,
        variants: Vec::new(),
    });
    let inner = seed_collection(
        &nested_registry,
        &mut nested_draft,
        CollSpec::List {
            elem: GArg::Enum(orphan),
        },
    );
    let outer = seed_collection(
        &nested_registry,
        &mut nested_draft,
        CollSpec::Map {
            key: GArg::Scalar(ScalarType::Text),
            value: GArg::Collection(inner),
        },
    );
    observations.push(target_case_with_registry(
        nested_registry,
        nested_draft,
        GArg::Collection(outer),
    ));

    assert_eq!(observations, vec![true; 3]);
}

#[test]
fn nested_non_ready_generic_targets_are_rejected_without_outer_publication() {
    let mut observations = Vec::new();
    for (is_enum, rejected) in [(false, false), (false, true), (true, false), (true, true)] {
        let inner_template = if is_enum {
            enum_template("Inner", "T")
        } else {
            struct_template("Inner", &["T"])
        };
        let registry = test_registry(vec![inner_template, struct_template("Outer", &["T"])]);
        let mut draft = ImageDraft::new();
        let inner = mint_ready(&registry, &mut draft, 0);
        let inner_arg = match inner {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        registry.generics.borrow_mut().type_insts[0].state = if rejected {
            TypeInstState::Rejected(ResolveRefusal::Unsupported)
        } else {
            TypeInstState::Filling { staged: None }
        };
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);
        let result = registry.mint_type_instance(&mut draft, 1, &[inner_arg], site());
        observations.push(
            result.is_err()
                && owner_snapshot(&registry) == owner_before
                && draft_fingerprint(&draft) == draft_before,
        );
    }

    assert_eq!(observations, vec![true; 4]);
}

#[test]
fn proof_clone_parameter_remains_local_to_the_discarded_owner() {
    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let draft = ImageDraft::new();
    let registry_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let clone = registry
        .clone_for_generic_check()
        .expect("stable registry clones for template proof");
    let mut clone_draft = draft.clone();
    let id = clone
        .mint_type_instance(&mut clone_draft, 0, &[GArg::Param(0)], site())
        .expect("proof-only parameter is legal");
    let clone_rows = clone.generics.borrow();
    let cloned = clone_rows
        .type_insts
        .iter()
        .find(|inst| inst.id == id)
        .expect("proof row exists");

    let body = match &cloned.state {
        TypeInstState::Ready(body) => body_snapshot(body),
        TypeInstState::Filling { .. } | TypeInstState::Rejected(_) => {
            panic!("proof row must be Ready")
        }
    };

    let mut expected = ImageDraft::new();
    let box_name = expected.intern_string("Box");
    let field_name = expected.intern_string("t");
    let expected_box = expected.add_record_type(RecordTypeDef {
        name: box_name,
        fields: vec![FieldDef {
            name: field_name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });

    assert_eq!(cloned.args, vec![GArg::Param(0)]);
    assert_eq!(
        body,
        BodySnapshot::Struct(vec![("t".to_string(), GArg::Param(0))])
    );
    assert_eq!(id, TypeInstId::Record(expected_box));
    assert_eq!(
        draft_fingerprint(&clone_draft),
        draft_fingerprint(&expected)
    );
    assert_eq!(owner_snapshot(&registry), registry_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn unknown_value_target_is_not_an_ordinary_zero_edge_node() {
    let registry = test_registry(vec![struct_template("Outer", &["T"])]);
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("Orphan");
    let orphan = draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });

    let rejected = match registry.mint_type_instance(&mut draft, 0, &[GArg::Struct(orphan)], site())
    {
        Err(_) => true,
        Ok(_) => ValueGraph::build(&registry).into_graph().is_none(),
    };

    assert!(rejected);
}

#[test]
fn valid_group_adds_no_value_containment_edge() {
    let mut draft = ImageDraft::new();
    let root_name = draft.intern_string("Resource");
    let root = draft.add_record_type(RecordTypeDef {
        name: root_name,
        fields: Vec::new(),
    });
    let group_name = draft.intern_string("Details");
    let group = draft.add_record_type(RecordTypeDef {
        name: group_name,
        fields: Vec::new(),
    });
    let mut registry = test_registry(vec![struct_template("Outer", &["T"])]);
    registry.records.push(RecordInfo {
        type_id: root,
        name: "Resource".to_string(),
        fields: Vec::new(),
        groups: vec![GroupInfo {
            name: "details".to_string(),
            type_id: group,
            fields: Vec::new(),
        }],
    });
    let outer = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Group(group)], site())
        .expect("a real group is a valid non-containing argument");
    let graph = ValueGraph::build(&registry)
        .into_graph()
        .expect("valid graph builds");
    let node = match outer {
        TypeInstId::Record(id) => ValueNode::Record(id),
        TypeInstId::Enum(id) => ValueNode::Enum(id),
    };
    let index = graph
        .nodes
        .iter()
        .position(|candidate| *candidate == node)
        .expect("outer node is present");

    assert!(graph.edges[index].is_empty());
}

#[test]
fn valid_collection_target_remains_accepted() {
    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let collection = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .into_index()
        .expect("aligned collection owners mint");
    let id = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Collection(collection)], site())
        .expect("valid collection target is accepted");

    let mut expected = ImageDraft::new();
    let expected_collection = expected.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Int),
    });
    let box_name = expected.intern_string("Box");
    let field_name = expected.intern_string("t");
    let expected_box = expected.add_record_type(RecordTypeDef {
        name: box_name,
        fields: vec![FieldDef {
            name: field_name,
            ty: ImageType::Collection {
                idx: expected_collection.index(),
                optional: false,
            },
            required: true,
        }],
    });

    assert_eq!(collection, expected_collection.index());
    assert_eq!(id, TypeInstId::Record(expected_box));
    assert_eq!(draft_fingerprint(&draft), draft_fingerprint(&expected));
}

#[test]
fn valid_generic_anchor_bytes_remain_stable() {
    let (registry, mut draft) = reserved_registry();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site())
        .expect("valid Option is Ready");

    assert_eq!(
        registry
            .enum_anchor_spelling(option)
            .into_anchor()
            .as_deref(),
        Some("Option[int]")
    );
}

fn take_resolve_invariant<T>(result: Result<T, ResolveError>) -> GenericInvariant {
    match result {
        Err(ResolveError::Invariant(invariant)) => invariant,
        Err(ResolveError::Refusal(_)) => {
            panic!("compiler metadata corruption must not become a semantic refusal")
        }
        Ok(_) => panic!("compiler metadata corruption must fail closed"),
    }
}

fn take_reader_invariant<T>(result: Result<T, GenericInvariant>) -> GenericInvariant {
    match result {
        Err(invariant) => invariant,
        Ok(_) => panic!("malformed Ready metadata must not reach a semantic reader"),
    }
}

fn invariant_family_tag(invariant: GenericInvariant) -> u8 {
    match invariant {
        GenericInvariant::ProofClone(error) => {
            let _ = error;
            0
        }
        GenericInvariant::CacheState(state) => {
            let _ = state;
            1
        }
        GenericInvariant::ReservedTemplateMissing(reserved) => {
            let _ = reserved;
            2
        }
        GenericInvariant::TypeTemplateMissing(template) => {
            let _ = template;
            3
        }
        GenericInvariant::TypeArgumentCountMismatch {
            template,
            expected,
            actual,
        } => {
            let _ = (template, expected, actual);
            4
        }
        GenericInvariant::TemplateKindMismatch {
            template,
            expected,
            actual,
        } => {
            let _ = (template, expected, actual);
            5
        }
        GenericInvariant::TypeBodyKindMismatch { id, body } => {
            let _ = (id, body);
            6
        }
        GenericInvariant::ReadyBodyMissing(id) => {
            let _ = id;
            7
        }
        GenericInvariant::ReadyEnumVariantMissing {
            id,
            template,
            variant,
        } => {
            let _ = (id, template, variant);
            8
        }
        GenericInvariant::TypeIdentityCollision(id) => {
            let _ = id;
            9
        }
        GenericInvariant::TypeInstantiationKeyCollision { first, duplicate } => {
            let _ = (first, duplicate);
            10
        }
        GenericInvariant::TypeArgumentOrderViolation { owner, target } => {
            let _ = (owner, target);
            11
        }
        GenericInvariant::TypeArgumentTargetMissing(target) => {
            let _ = target;
            12
        }
        GenericInvariant::TypeArgumentParameter(param) => {
            let _ = param;
            13
        }
        GenericInvariant::CollectionIndexMismatch {
            kind,
            cache_index,
            draft_index,
        } => {
            let _ = (kind, cache_index, draft_index);
            14
        }
        GenericInvariant::ReadyBodyShapeMismatch(id) => {
            let _ = id;
            15
        }
    }
}

#[test]
fn generic_invariant_family_is_closed() {
    assert_eq!(
        invariant_family_tag(GenericInvariant::TypeTemplateMissing(7)),
        3
    );
}

#[test]
fn template_for_args_reports_exact_missing_and_count_causes() {
    let registry = test_registry(vec![struct_template("Pair", &["T", "U"])]);
    let before = owner_snapshot(&registry);

    assert!(matches!(
        registry.template_for_args(usize::MAX, &[]),
        Err(GenericInvariant::TypeTemplateMissing(usize::MAX))
    ));
    assert!(matches!(
        registry.template_for_args(0, &[]),
        Err(GenericInvariant::TypeArgumentCountMismatch {
            template: 0,
            expected: 2,
            actual: 0,
        })
    ));
    assert!(matches!(
        registry.template_for_args(
            0,
            &[
                GArg::Scalar(ScalarType::Int),
                GArg::Scalar(ScalarType::Bool),
                GArg::Scalar(ScalarType::Text),
            ],
        ),
        Err(GenericInvariant::TypeArgumentCountMismatch {
            template: 0,
            expected: 2,
            actual: 3,
        })
    ));
    assert!(
        registry
            .template_for_args(
                0,
                &[
                    GArg::Scalar(ScalarType::Int),
                    GArg::Scalar(ScalarType::Bool),
                ],
            )
            .is_ok()
    );
    assert_eq!(owner_snapshot(&registry), before);
}

#[test]
fn reserved_result_application_reports_missing_and_wrong_kind_causes() {
    let registry = test_registry(Vec::new());
    assert_eq!(
        take_resolve_invariant(registry.application_template("Result")),
        GenericInvariant::ReservedTemplateMissing(Reserved::Result)
    );

    let mut result = struct_template("Result", &["T", "E"]);
    result.reserved = Some(Reserved::Result);
    let registry = test_registry(vec![result]);
    assert_eq!(
        take_resolve_invariant(registry.application_template("Result")),
        GenericInvariant::TemplateKindMismatch {
            template: 0,
            expected: TypeInstKind::Enum,
            actual: TypeInstKind::Struct,
        }
    );
}

fn assert_exact_target_invariant(registry: TypeRegistry, mut draft: ImageDraft, target: GArg) {
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant =
        take_resolve_invariant(registry.mint_type_instance(&mut draft, 0, &[target], site()));

    assert_eq!(
        invariant,
        GenericInvariant::TypeArgumentTargetMissing(target)
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn argument_targets_report_exact_private_causes_without_publication() {
    let mut record_draft = ImageDraft::new();
    let record_name = record_draft.intern_string("OrphanRecord");
    let record = record_draft.add_record_type(RecordTypeDef {
        name: record_name,
        fields: Vec::new(),
    });
    let mut enum_draft = ImageDraft::new();
    let enum_name = enum_draft.intern_string("OrphanEnum");
    let enum_id = enum_draft.add_enum_type(EnumTypeDef {
        name: enum_name,
        variants: Vec::new(),
    });

    assert_exact_target_invariant(
        test_registry(vec![struct_template("Box", &["T"])]),
        ImageDraft::new(),
        GArg::Nominal(NominalId(0)),
    );
    assert_exact_target_invariant(
        test_registry(vec![struct_template("Box", &["T"])]),
        record_draft.clone(),
        GArg::Struct(record),
    );
    assert_exact_target_invariant(
        test_registry(vec![struct_template("Box", &["T"])]),
        record_draft,
        GArg::Group(record),
    );
    assert_exact_target_invariant(
        test_registry(vec![struct_template("Box", &["T"])]),
        enum_draft,
        GArg::Enum(enum_id),
    );
    assert_exact_target_invariant(
        test_registry(vec![struct_template("Box", &["T"])]),
        ImageDraft::new(),
        GArg::Collection(0),
    );

    let (group_registry, group_draft, _, group_id) = resource_target_fixture();
    assert_exact_target_invariant(group_registry, group_draft, GArg::Struct(group_id));
    let (root_registry, root_draft, root_id, _) = resource_target_fixture();
    assert_exact_target_invariant(root_registry, root_draft, GArg::Struct(root_id));
    let (root_registry, root_draft, root_id, _) = resource_target_fixture();
    assert_exact_target_invariant(root_registry, root_draft, GArg::Group(root_id));

    let mut struct_draft = ImageDraft::new();
    let struct_name = struct_draft.intern_string("Point");
    let struct_id = struct_draft.add_record_type(RecordTypeDef {
        name: struct_name,
        fields: Vec::new(),
    });
    let mut struct_registry = test_registry(vec![struct_template("Box", &["T"])]);
    struct_registry.structs.push(StructInfo {
        type_id: struct_id,
        name: "Point".to_string(),
        fields: Vec::new(),
    });
    assert_exact_target_invariant(struct_registry, struct_draft, GArg::Group(struct_id));

    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant = take_resolve_invariant(registry.mint_type_instance(
        &mut draft,
        0,
        &[GArg::Param(7)],
        site(),
    ));
    assert_eq!(invariant, GenericInvariant::TypeArgumentParameter(7));
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);

    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let orphan_name = draft.intern_string("NestedOrphan");
    let orphan = draft.add_record_type(RecordTypeDef {
        name: orphan_name,
        fields: Vec::new(),
    });
    let collection = seed_collection(
        &registry,
        &mut draft,
        CollSpec::List {
            elem: GArg::Struct(orphan),
        },
    );
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant = take_resolve_invariant(registry.mint_type_instance(
        &mut draft,
        0,
        &[GArg::Collection(collection)],
        site(),
    ));
    assert_eq!(
        invariant,
        GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan))
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn nested_non_ready_targets_report_the_exact_missing_body() {
    for rejected in [false, true] {
        let registry = test_registry(vec![
            struct_template("Outer", &["T"]),
            struct_template("Inner", &["T"]),
        ]);
        let mut draft = ImageDraft::new();
        let inner = mint_ready(&registry, &mut draft, 1);
        let row = registry
            .generics
            .borrow()
            .type_insts
            .iter()
            .position(|inst| inst.id == inner)
            .expect("Inner row exists");
        registry.generics.borrow_mut().type_insts[row].state = if rejected {
            TypeInstState::Rejected(ResolveRefusal::Unsupported)
        } else {
            TypeInstState::Filling { staged: None }
        };
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);
        let target = match inner {
            TypeInstId::Record(id) => GArg::Struct(id),
            TypeInstId::Enum(id) => GArg::Enum(id),
        };
        let invariant =
            take_resolve_invariant(registry.mint_type_instance(&mut draft, 0, &[target], site()));

        assert_eq!(invariant, GenericInvariant::ReadyBodyMissing(inner));
        assert_eq!(owner_snapshot(&registry), owner_before);
        assert_eq!(draft_fingerprint(&draft), draft_before);
    }
}

#[test]
fn ready_readers_preserve_the_exact_argument_invariant() {
    let (registry, mut draft) = reserved_registry();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site())
        .expect("control Option is Ready");
    let orphan_name = draft.intern_string("ReaderOrphan");
    let orphan = draft.add_record_type(RecordTypeDef {
        name: orphan_name,
        fields: Vec::new(),
    });
    registry
        .generics
        .borrow_mut()
        .type_insts
        .iter_mut()
        .find(|inst| inst.id == TypeInstId::Enum(option))
        .expect("Ready Option row exists")
        .args = vec![GArg::Struct(orphan)];
    let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan));
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);

    assert_eq!(
        take_reader_invariant(registry.instantiation_of(TypeInstId::Enum(option))),
        expected
    );
    assert_eq!(
        take_reader_invariant(registry.type_inst_body(TypeInstId::Enum(option))),
        expected
    );
    assert_eq!(take_reader_invariant(registry.as_option(option)), expected);
    assert_eq!(
        take_reader_invariant(registry.enum_variants(option)),
        expected
    );
    assert_eq!(
        take_reader_invariant(registry.enum_anchor_spelling(option)),
        expected
    );
    assert_eq!(
        take_reader_invariant(ValueGraph::build(&registry)),
        expected
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);

    for invalid_ok in [true, false] {
        let (registry, mut draft) = reserved_registry();
        let template = registry
            .application_template("Result")
            .expect("reserved Result template exists");
        let result = registry
            .mint_type_instance(
                &mut draft,
                template,
                &[
                    GArg::Scalar(ScalarType::Int),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(),
            )
            .ok()
            .and_then(|id| match id {
                TypeInstId::Enum(id) => Some(id),
                TypeInstId::Record(_) => None,
            })
            .expect("control Result is Ready");
        let orphan_name = draft.intern_string("ResultReaderOrphan");
        let orphan = draft.add_record_type(RecordTypeDef {
            name: orphan_name,
            fields: Vec::new(),
        });
        let args = if invalid_ok {
            vec![GArg::Struct(orphan), GArg::Scalar(ScalarType::Bool)]
        } else {
            vec![GArg::Scalar(ScalarType::Int), GArg::Struct(orphan)]
        };
        registry
            .generics
            .borrow_mut()
            .type_insts
            .iter_mut()
            .find(|inst| inst.id == TypeInstId::Enum(result))
            .expect("Ready Result row exists")
            .args = args;
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);

        assert_eq!(
            take_reader_invariant(registry.as_result(result)),
            GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan))
        );
        assert_eq!(owner_snapshot(&registry), owner_before);
        assert_eq!(draft_fingerprint(&draft), draft_before);
    }
}

#[test]
fn reserved_readers_report_exact_argument_counts() {
    for args in [
        Vec::new(),
        vec![
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Bool),
        ],
    ] {
        let (registry, mut draft) = reserved_registry();
        let template = registry
            .application_template("Option")
            .expect("reserved Option template exists");
        let option = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site())
            .expect("control Option is Ready");
        registry
            .generics
            .borrow_mut()
            .type_insts
            .iter_mut()
            .find(|inst| inst.id == TypeInstId::Enum(option))
            .expect("Ready Option row exists")
            .args = args;
        let actual = registry
            .generics
            .borrow()
            .type_insts
            .iter()
            .find(|inst| inst.id == TypeInstId::Enum(option))
            .expect("Ready Option row exists")
            .args
            .len();

        assert_eq!(
            take_reader_invariant(registry.as_option(option)),
            GenericInvariant::TypeArgumentCountMismatch {
                template,
                expected: 1,
                actual,
            }
        );
    }

    for args in [
        vec![GArg::Scalar(ScalarType::Int)],
        vec![
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Bool),
            GArg::Scalar(ScalarType::Text),
        ],
    ] {
        let (registry, mut draft) = reserved_registry();
        let template = registry
            .application_template("Result")
            .expect("reserved Result template exists");
        let result = registry
            .mint_type_instance(
                &mut draft,
                template,
                &[
                    GArg::Scalar(ScalarType::Int),
                    GArg::Scalar(ScalarType::Bool),
                ],
                site(),
            )
            .ok()
            .and_then(|id| match id {
                TypeInstId::Enum(id) => Some(id),
                TypeInstId::Record(_) => None,
            })
            .expect("control Result is Ready");
        let actual = args.len();
        registry
            .generics
            .borrow_mut()
            .type_insts
            .iter_mut()
            .find(|inst| inst.id == TypeInstId::Enum(result))
            .expect("Ready Result row exists")
            .args = args;

        assert_eq!(
            take_reader_invariant(registry.as_result(result)),
            GenericInvariant::TypeArgumentCountMismatch {
                template,
                expected: 2,
                actual,
            }
        );
    }
}

#[test]
fn typed_struct_instance_owner_returns_only_a_ready_record() {
    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let record = registry
        .mint_struct_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site())
        .expect("valid struct instance is proven Ready");
    assert_eq!(record.index(), 0);

    let registry = test_registry(vec![enum_template("Maybe", "T")]);
    let mut draft = ImageDraft::new();
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant = take_resolve_invariant(registry.mint_struct_instance(
        &mut draft,
        0,
        &[GArg::Scalar(ScalarType::Int)],
        site(),
    ));
    assert_eq!(
        invariant,
        GenericInvariant::TemplateKindMismatch {
            template: 0,
            expected: TypeInstKind::Struct,
            actual: TypeInstKind::Enum,
        }
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);

    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let record = registry
        .mint_struct_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site())
        .expect("control struct instance is Ready");
    registry
        .generics
        .borrow_mut()
        .type_insts
        .iter_mut()
        .find(|inst| inst.id == TypeInstId::Record(record))
        .expect("Ready struct row exists")
        .state = TypeInstState::Ready(InstBody::Enum(Vec::new()));
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant = take_resolve_invariant(registry.mint_struct_instance(
        &mut draft,
        0,
        &[GArg::Scalar(ScalarType::Int)],
        site(),
    ));
    assert_eq!(
        invariant,
        GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Record(record),
            body: TypeInstKind::Enum,
        }
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}

#[test]
fn typed_enum_variant_owner_returns_the_selected_ready_member() {
    let registry = test_registry(vec![enum_template("Maybe", "T")]);
    let mut draft = ImageDraft::new();
    let witness = registry
        .mint_enum_variant_instance(
            &mut draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            EnumVariantSelection {
                index: 0,
                name: "item",
            },
            site(),
        )
        .expect("valid enum member is proven Ready");
    assert_eq!(witness.variant, 0);
    assert_eq!(witness.enum_id.index(), 0);

    let registry = test_registry(vec![struct_template("Box", &["T"])]);
    let mut draft = ImageDraft::new();
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant = take_resolve_invariant(registry.mint_enum_variant_instance(
        &mut draft,
        0,
        &[GArg::Scalar(ScalarType::Int)],
        EnumVariantSelection {
            index: 0,
            name: "item",
        },
        site(),
    ));
    assert_eq!(
        invariant,
        GenericInvariant::TemplateKindMismatch {
            template: 0,
            expected: TypeInstKind::Enum,
            actual: TypeInstKind::Struct,
        }
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);

    for renamed in [false, true] {
        let registry = test_registry(vec![enum_template("Maybe", "T")]);
        let mut draft = ImageDraft::new();
        let witness = registry
            .mint_enum_variant_instance(
                &mut draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                EnumVariantSelection {
                    index: 0,
                    name: "item",
                },
                site(),
            )
            .expect("control enum member is Ready");
        let variants = if renamed {
            vec![InstVariant {
                name: "renamed".to_string(),
                payload: Vec::new(),
            }]
        } else {
            Vec::new()
        };
        registry
            .generics
            .borrow_mut()
            .type_insts
            .iter_mut()
            .find(|inst| inst.id == TypeInstId::Enum(witness.enum_id))
            .expect("Ready enum row exists")
            .state = TypeInstState::Ready(InstBody::Enum(variants));
        let owner_before = owner_snapshot(&registry);
        let draft_before = draft_fingerprint(&draft);
        let invariant = take_resolve_invariant(registry.mint_enum_variant_instance(
            &mut draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            EnumVariantSelection {
                index: 0,
                name: "item",
            },
            site(),
        ));
        assert_eq!(
            invariant,
            GenericInvariant::ReadyEnumVariantMissing {
                id: witness.enum_id,
                template: 0,
                variant: 0,
            }
        );
        assert_eq!(owner_snapshot(&registry), owner_before);
        assert_eq!(draft_fingerprint(&draft), draft_before);
    }

    let registry = test_registry(vec![enum_template("Maybe", "T")]);
    let mut draft = ImageDraft::new();
    let witness = registry
        .mint_enum_variant_instance(
            &mut draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            EnumVariantSelection {
                index: 0,
                name: "item",
            },
            site(),
        )
        .expect("control enum member is Ready");
    registry
        .generics
        .borrow_mut()
        .type_insts
        .iter_mut()
        .find(|inst| inst.id == TypeInstId::Enum(witness.enum_id))
        .expect("Ready enum row exists")
        .state = TypeInstState::Ready(InstBody::Struct(Vec::new()));
    let owner_before = owner_snapshot(&registry);
    let draft_before = draft_fingerprint(&draft);
    let invariant = take_resolve_invariant(registry.mint_enum_variant_instance(
        &mut draft,
        0,
        &[GArg::Scalar(ScalarType::Int)],
        EnumVariantSelection {
            index: 0,
            name: "item",
        },
        site(),
    ));
    assert_eq!(
        invariant,
        GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(witness.enum_id),
            body: TypeInstKind::Struct,
        }
    );
    assert_eq!(owner_snapshot(&registry), owner_before);
    assert_eq!(draft_fingerprint(&draft), draft_before);
}
