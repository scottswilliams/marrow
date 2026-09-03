use super::*;

use marrow_image::ImageDraft;

use marrow_image::EnumTypeDef;

use crate::compile::admitted;
use marrow_syntax::{Declaration, parse_source};

/// A fresh armed transaction over its own leaked owner, for fixtures that never
/// touch the owner again.
fn fresh_draft() -> DraftTxn<'static> {
    let owner: &'static mut ImageDraft = Box::leak(Box::new(ImageDraft::new()));
    admitted(owner)
}

/// The wide collection id at `index`, spelled compactly for the corpus.
fn coll(index: u16) -> CollTypeId {
    CollTypeId::from_index(index)
}

fn name(text: &str) -> TypeExpr {
    TypeExpr::Name {
        text: text.to_string(),
        segment_spans: Vec::new(),
        span: SourceSpan::default(),
    }
}

fn apply(head: &str, args: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Apply {
        head: head.to_string(),
        head_span: SourceSpan::default(),
        args,
        span: SourceSpan::default(),
    }
}

fn template(name: &str, fields: Vec<(&str, TypeExpr)>) -> TypeTemplate {
    TypeTemplate {
        name: name.to_string(),
        file: Some(crate::test_file_identity("src/main.mw")),
        name_span: SourceSpan::default(),
        reserved: None,
        type_params: vec![("T".to_string(), None)],
        body: TemplateBody::Struct(
            fields
                .into_iter()
                .map(|(field, ty)| (field.to_string(), ty))
                .collect(),
        ),
    }
}

fn enum_template(name: &str, payload: TypeExpr) -> TypeTemplate {
    TypeTemplate {
        name: name.to_string(),
        file: Some(crate::test_file_identity("src/main.mw")),
        name_span: SourceSpan::default(),
        reserved: None,
        type_params: vec![("T".to_string(), None)],
        body: TemplateBody::Enum(
            vec![TemplateVariant {
                name: "value".to_string(),
                payload: vec![TemplatePayload {
                    name: "item".to_string(),
                    ty: payload,
                }],
            }]
            .into(),
        ),
    }
}

/// Merge a finished generic transfer into a fresh collector and read the
/// complete ordered rows, panicking on a limited terminal (these fixtures
/// stay far below the ceilings).
fn ordered(outcome: GenericDiagnostics) -> Vec<SourceDiagnostic> {
    let mut collector = DiagnosticCollector::new();
    outcome.merge_into(&mut collector);
    collector.finish().expect_complete()
}

fn registry(templates: Vec<TypeTemplate>) -> TypeRegistry {
    TypeRegistry {
        record_declarations: Vec::new(),
        named: DeclarationLedger::new(
            DeclarationNamespace::NamedType,
            DeclarationBudget::default(),
        ),
        members: DeclarationLedger::new(
            DeclarationNamespace::ResourceMember,
            DeclarationBudget::default(),
        ),
        aliases: BTreeMap::new(),
        nominals: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        records: Vec::new(),
        type_templates: templates,
        generics: RefCell::default(),
        collections: RefCell::default(),
        collection_index: RefCell::default(),
        row_directory: RefCell::default(),
        coordinates: DeclarationCoordinates::default(),
    }
}

fn site(line: u32) -> MintSite<'static> {
    MintSite {
        file: crate::test_main_file_identity(),
        span: SourceSpan {
            line,
            column: 9,
            ..SourceSpan::default()
        },
    }
}

fn row<'a>(registry: &'a TypeRegistry, name: &str) -> std::cell::Ref<'a, TypeInst> {
    std::cell::Ref::map(registry.generics.borrow(), |generics| {
        generics
            .type_insts
            .iter()
            .find(|inst| registry.type_templates[inst.template].name == name)
            .expect("named row exists")
    })
}

#[derive(Debug, PartialEq, Eq)]
enum StableLimit {
    Open,
    PendingRow(SourceDiagnostic),
    Reported,
}

#[derive(Debug, PartialEq, Eq)]
enum StableRowState {
    Filling,
    Staged,
    Ready,
    RejectedLimit,
    RejectedUnsupported,
    RejectedDeclaration,
}

#[derive(Debug, PartialEq, Eq)]
struct StableRow {
    template: usize,
    args: Vec<GArg>,
    id: TypeInstId,
    state: StableRowState,
    body: Option<StableBody>,
    dependents: Vec<usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum StableBody {
    Struct(Vec<(String, GArg)>),
    Enum(Vec<(String, Vec<(String, GArg)>)>),
}

fn stable_body(body: &InstBody) -> StableBody {
    match body {
        InstBody::Struct(fields) => StableBody::Struct(fields.clone()),
        InstBody::Enum(variants) => StableBody::Enum(
            variants
                .iter()
                .map(|variant| (variant.name.clone(), variant.payload.clone()))
                .collect(),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StableSnapshot {
    rows: Vec<StableRow>,
    collections: Vec<CollSpec>,
    fn_base: u16,
    functions: Vec<(usize, Vec<GArg>, u16)>,
    queue: Vec<(usize, Vec<GArg>, u16)>,
    fill_batch_start: Option<usize>,
    fill_rows: Vec<(TypeInstKey, usize)>,
    fill_stack: Vec<usize>,
    fill_failures: Vec<(usize, ResolveRefusal)>,
    limit: StableLimit,
    payloads: crate::diag::CollectorProbe,
    build_invariant: Option<GenericInvariant>,
    // The lockstep secondary indexes and the swapped argument domain: an isolation probe
    // must observe a missed index purge or a stuck `TemplateProof` domain, not only the
    // primary append-only owners. `HashMap` equality is content-based, so these compare
    // regardless of iteration order.
    type_index: HashMap<(usize, Vec<GArg>), usize>,
    fn_index: HashMap<(usize, Vec<GArg>), usize>,
    collection_index: HashMap<CollSpec, CollTypeId>,
    argument_domain: ArgumentDomain,
}

fn stable_snapshot(registry: &TypeRegistry) -> StableSnapshot {
    let generics = registry.generics.borrow();
    let rows = generics
        .type_insts
        .iter()
        .map(|inst| {
            let (state, body) = match &inst.state {
                TypeInstState::Filling { staged: None } => (StableRowState::Filling, None),
                TypeInstState::Filling { staged: Some(body) } => {
                    (StableRowState::Staged, Some(stable_body(body)))
                }
                TypeInstState::Ready(body) => (StableRowState::Ready, Some(stable_body(body))),
                TypeInstState::Rejected(ResolveRefusal::Limit) => {
                    (StableRowState::RejectedLimit, None)
                }
                TypeInstState::Rejected(ResolveRefusal::Unsupported) => {
                    (StableRowState::RejectedUnsupported, None)
                }
                TypeInstState::Rejected(ResolveRefusal::RefusedDeclaration(_)) => {
                    (StableRowState::RejectedDeclaration, None)
                }
            };
            StableRow {
                template: inst.template,
                args: inst.args.clone(),
                id: inst.id,
                state,
                body,
                dependents: inst.dependents.clone(),
            }
        })
        .collect();
    let functions = generics
        .fn_insts
        .iter()
        .map(|inst| (inst.template, inst.args.clone(), inst.func))
        .collect();
    let queue = generics
        .fn_queue
        .iter()
        .map(|inst| (inst.template, inst.args.clone(), inst.func))
        .collect();
    let limit = match &generics.limit {
        LimitState::Open => StableLimit::Open,
        LimitState::Pending(diagnostic) => StableLimit::PendingRow(diagnostic.clone()),
        LimitState::Reported => StableLimit::Reported,
    };
    StableSnapshot {
        rows,
        collections: registry.collections.borrow().clone(),
        fn_base: generics.fn_base,
        functions,
        queue,
        fill_batch_start: generics.fill_batch_start,
        fill_rows: generics
            .fill_rows
            .iter()
            .map(|(key, index)| (*key, *index))
            .collect(),
        fill_stack: generics.fill_stack.clone(),
        fill_failures: generics.fill_failures.clone(),
        limit,
        payloads: generics.collection_payloads.probe(),
        build_invariant: generics.build_invariant,
        type_index: generics.type_index.clone(),
        fn_index: generics.fn_index.clone(),
        collection_index: registry.collection_index.borrow().clone(),
        argument_domain: generics.argument_domain,
    }
}

fn draft_snapshot(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
    let encoded = draft.encode().expect("test draft encodes");
    (encoded.bytes, encoded.image_id)
}

/// Replay the coherence validation a template proof relies on over every settled row:
/// build the full identity directory and revalidate each ready instantiation body. The
/// production proof path reuses the shared directory the mint path already built, so this
/// drives the same machinery (`MetadataScratch::try_new` + `ready_inst_body_with`)
/// directly to assert it still rejects a corrupted owner. It is read-only, mirroring the
/// pass's admission check.
fn validate_ready_metadata(registry: &TypeRegistry) -> Result<(), GenericInvariant> {
    let view = registry.metadata_view();
    let mut scratch = MetadataScratch::try_new(&view)?;
    for inst in &view.generics.type_insts {
        view.ready_inst_body_with(inst, &mut scratch)?;
    }
    Ok(())
}

fn add_declared_struct(
    registry: &mut TypeRegistry,
    draft: &mut DraftTxn<'_>,
    name: &str,
    fields: Vec<(&str, GArg)>,
) -> TypeId {
    let record_name = draft.intern_string(name).expect("a within-domain mint");
    let mut image_fields = Vec::with_capacity(fields.len());
    let mut field_infos = Vec::with_capacity(fields.len());
    for (field, ty) in fields {
        let field_name = draft.intern_string(field).expect("a within-domain mint");
        image_fields.push(FieldDef {
            name: field_name,
            ty: ty.image(),
            required: true,
        });
        field_infos.push(FieldInfo {
            name: field.to_string(),
            ty,
            required: true,
        });
    }
    let type_id = draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields: image_fields,
        })
        .expect("a within-domain mint");
    registry.structs.push(StructInfo {
        type_id,
        name: name.to_string(),
        fields: field_infos,
        verdict: DeclarationVerdict::Accepted,
    });
    type_id
}

fn add_resource_record(
    registry: &mut TypeRegistry,
    draft: &mut DraftTxn<'_>,
    name: &str,
) -> TypeId {
    let record_name = draft.intern_string(name).expect("a within-domain mint");
    let type_id = draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    registry.records.push(RecordInfo {
        type_id,
        name: name.to_string(),
        fields: Vec::new(),
        groups: Vec::new(),
    });
    type_id
}

fn add_resource_group(
    registry: &mut TypeRegistry,
    draft: &mut DraftTxn<'_>,
    record: usize,
    name: &str,
) -> TypeId {
    let group_name = draft.intern_string(name).expect("a within-domain mint");
    let type_id = draft
        .add_record_type(RecordTypeDef {
            name: group_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    registry.records[record].groups.push(GroupInfo {
        name: name.to_string(),
        type_id,
        fields: Vec::new(),
    });
    type_id
}

type MetadataOwnerSnapshot = (
    Vec<TypeId>,
    Vec<Vec<TypeId>>,
    Vec<TypeId>,
    Vec<EnumId>,
    StableSnapshot,
);

fn metadata_owner_snapshot(registry: &TypeRegistry) -> MetadataOwnerSnapshot {
    (
        registry
            .records
            .iter()
            .map(|record| record.type_id)
            .collect(),
        registry
            .records
            .iter()
            .map(|record| record.groups.iter().map(|group| group.type_id).collect())
            .collect(),
        registry.structs.iter().map(|info| info.type_id).collect(),
        registry.enums.iter().map(|info| info.enum_id).collect(),
        stable_snapshot(registry),
    )
}

fn assert_metadata_unchanged(
    registry: &TypeRegistry,
    draft: &ImageDraft,
    owner: &MetadataOwnerSnapshot,
    image: &(Vec<u8>, marrow_image::ImageId),
) {
    assert_eq!(&metadata_owner_snapshot(registry), owner);
    assert_eq!(&draft_snapshot(draft), image);
}

fn active_registry() -> TypeRegistry {
    let mut registry = registry(vec![template("Active", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(1))
        .expect("seed row mints ready");
    let mut generics = registry.generics.borrow_mut();
    let prior = std::mem::replace(
        &mut generics.type_insts[0].state,
        TypeInstState::Rejected(ResolveRefusal::Unsupported),
    );
    let TypeInstState::Ready(body) = prior else {
        panic!("seed row is Ready")
    };
    generics.type_insts[0].state = TypeInstState::Filling { staged: Some(body) };
    generics.fill_batch_start = Some(0);
    let id = generics.type_insts[0].id;
    generics.fill_rows.insert(id.into(), 0);
    drop(generics);
    registry
}

fn cache_invariant_name(cause: GenericCacheInvariant) -> &'static str {
    match cause {
        GenericCacheInvariant::ActiveBatchMissing => "active batch missing",
        GenericCacheInvariant::ActiveBatchRange => "active batch range",
        GenericCacheInvariant::ActiveRowCardinality => "active row cardinality",
        GenericCacheInvariant::ActiveRowKeyMismatch => "active row key mismatch",
        GenericCacheInvariant::ActiveFillStackNotEmpty => "active fill stack not empty",
        GenericCacheInvariant::FailureIndexOutOfRange => "failure index out of range",
        GenericCacheInvariant::DependentIndexOutOfRange => "dependent index out of range",
        GenericCacheInvariant::StableRowInActiveBatch => "stable row in active batch",
        GenericCacheInvariant::IncompleteRowWithoutRefusal => "incomplete row without refusal",
        GenericCacheInvariant::FillingReuseOutsideBatch => "Filling reuse outside batch",
        GenericCacheInvariant::SettledRowMissing => "settled row missing",
        GenericCacheInvariant::SettledRowStillFilling => "settled row still Filling",
        GenericCacheInvariant::FillStackMismatch => "fill stack mismatch",
        GenericCacheInvariant::MintIndexDrift => "mint index drift",
        GenericCacheInvariant::MintKeyAlreadyPresent => "mint key already present",
    }
}

#[test]
fn settlement_precommit_faults_are_exact_and_read_only() {
    #[derive(Clone, Copy)]
    enum Fault {
        MissingBatch,
        BatchRange,
        NonemptyStack,
        RowCardinality,
        RowKey,
        FailureRange,
        StableRow,
        DependentRange,
        IncompleteRow,
    }

    let cases = [
        (
            Fault::MissingBatch,
            GenericCacheInvariant::ActiveBatchMissing,
        ),
        (Fault::BatchRange, GenericCacheInvariant::ActiveBatchRange),
        (
            Fault::NonemptyStack,
            GenericCacheInvariant::ActiveFillStackNotEmpty,
        ),
        (
            Fault::RowCardinality,
            GenericCacheInvariant::ActiveRowCardinality,
        ),
        (Fault::RowKey, GenericCacheInvariant::ActiveRowKeyMismatch),
        (
            Fault::FailureRange,
            GenericCacheInvariant::FailureIndexOutOfRange,
        ),
        (
            Fault::StableRow,
            GenericCacheInvariant::StableRowInActiveBatch,
        ),
        (
            Fault::DependentRange,
            GenericCacheInvariant::DependentIndexOutOfRange,
        ),
        (
            Fault::IncompleteRow,
            GenericCacheInvariant::IncompleteRowWithoutRefusal,
        ),
    ];

    for (fault, expected) in cases {
        let registry = active_registry();
        {
            let mut generics = registry.generics.borrow_mut();
            match fault {
                Fault::MissingBatch => generics.fill_batch_start = None,
                Fault::BatchRange => generics.fill_batch_start = Some(2),
                Fault::NonemptyStack => generics.fill_stack.push(0),
                Fault::RowCardinality => generics.fill_rows.clear(),
                Fault::RowKey => {
                    let key = TypeInstKey::from(generics.type_insts[0].id);
                    generics.fill_rows.insert(key, 1);
                }
                Fault::FailureRange => generics
                    .fill_failures
                    .push((1, ResolveRefusal::Unsupported)),
                Fault::StableRow => {
                    let prior = std::mem::replace(
                        &mut generics.type_insts[0].state,
                        TypeInstState::Rejected(ResolveRefusal::Unsupported),
                    );
                    let TypeInstState::Filling { staged: Some(body) } = prior else {
                        panic!("active row has a staged body")
                    };
                    generics.type_insts[0].state = TypeInstState::Ready(body);
                }
                Fault::DependentRange => generics.type_insts[0].dependents.push(1),
                Fault::IncompleteRow => {
                    generics.type_insts[0].state = TypeInstState::Filling { staged: None };
                }
            }
        }
        let before = stable_snapshot(&registry);
        assert_eq!(
            registry.settle_fill_batch(),
            Err(ResolveError::Invariant(GenericInvariant::CacheState(
                expected
            ))),
            "{}",
            cache_invariant_name(expected)
        );
        assert_eq!(stable_snapshot(&registry), before);
    }
}

#[test]
fn nonsettlement_cache_faults_are_exact_and_read_only() {
    let mut registry = active_registry();
    registry.generics.borrow_mut().fill_batch_start = None;
    registry.generics.borrow_mut().fill_rows.clear();
    let id = registry.generics.borrow().type_insts[0].id;
    let before = stable_snapshot(&registry);
    let mut draft = fresh_draft();
    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2),),
        Err(ResolveError::Invariant(GenericInvariant::CacheState(
            GenericCacheInvariant::FillingReuseOutsideBatch,
        )))
    );
    assert_eq!(stable_snapshot(&registry), before);

    let missing_before = stable_snapshot(&registry);
    assert_eq!(
        registry.settled_type_result(1, id, AnyReadyInstance),
        Err(ResolveError::Invariant(GenericInvariant::CacheState(
            GenericCacheInvariant::SettledRowMissing,
        )))
    );
    assert_eq!(stable_snapshot(&registry), missing_before);

    let filling_before = stable_snapshot(&registry);
    assert_eq!(
        registry.settled_type_result(0, id, AnyReadyInstance),
        Err(ResolveError::Invariant(GenericInvariant::CacheState(
            GenericCacheInvariant::SettledRowStillFilling,
        )))
    );
    assert_eq!(stable_snapshot(&registry), filling_before);

    registry.generics.borrow_mut().fill_stack.push(1);
    let stack_before = stable_snapshot(&registry);
    assert_eq!(
        registry.finish_fill_stack(0),
        Err(ResolveError::Invariant(GenericInvariant::CacheState(
            GenericCacheInvariant::FillStackMismatch,
        )))
    );
    assert_eq!(stable_snapshot(&registry), stack_before);
}

#[test]
fn failed_fill_rejects_reverse_dependent_rows_without_poisoning_siblings() {
    let mut registry = registry(vec![
        template("Good", vec![("value", name("T"))]),
        template(
            "Outer",
            vec![
                ("good", apply("Good", vec![name("T")])),
                ("inner", apply("Inner", vec![name("T")])),
                ("bad", apply("Missing", vec![name("T")])),
            ],
        ),
        template("Inner", vec![("outer", apply("Outer", vec![name("T")]))]),
    ]);
    let mut draft = fresh_draft();
    assert_eq!(
        registry.mint_type_instance(&mut draft, 1, &[GArg::Scalar(ScalarType::Int)], site(10),),
        Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
    );

    assert!(matches!(
        row(&registry, "Good").state,
        TypeInstState::Ready(_)
    ));
    assert!(matches!(
        row(&registry, "Outer").state,
        TypeInstState::Rejected(ResolveRefusal::Unsupported)
    ));
    assert!(matches!(
        row(&registry, "Inner").state,
        TypeInstState::Rejected(ResolveRefusal::Unsupported)
    ));
    let (outer_id, before) = {
        let generics = registry.generics.borrow();
        (generics.type_insts[0].id, generics.type_insts.len())
    };
    assert_eq!(
        registry.mint_type_instance(&mut draft, 1, &[GArg::Scalar(ScalarType::Int)], site(20),),
        Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
    );
    let generics = registry.generics.borrow();
    assert_eq!(generics.type_insts.len(), before);
    assert_eq!(generics.type_insts[0].id, outer_id);
    assert!(generics.fill_stack.is_empty());
    assert!(
        generics
            .type_insts
            .iter()
            .all(|inst| !matches!(inst.state, TypeInstState::Filling { .. }))
    );
}

#[test]
fn collection_substitution_edges_reject_dependents_of_a_failed_outer_row() {
    let mut registry = registry(vec![
        template(
            "Outer",
            vec![
                (
                    "child",
                    apply(
                        "Child",
                        vec![apply("List", vec![apply("Outer", vec![name("T")])])],
                    ),
                ),
                ("bad", apply("Missing", vec![name("T")])),
            ],
        ),
        template("Child", vec![("value", name("T"))]),
    ]);
    let mut draft = fresh_draft();
    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(10),),
        Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
    );
    assert!(matches!(
        row(&registry, "Outer").state,
        TypeInstState::Rejected(ResolveRefusal::Unsupported)
    ));
    assert!(matches!(
        row(&registry, "Child").state,
        TypeInstState::Rejected(ResolveRefusal::Unsupported)
    ));
}

#[test]
fn mixed_fill_refusals_join_to_limit_without_poisoning_an_independent_row() {
    for failures in [
        vec![(0, ResolveRefusal::Unsupported), (1, ResolveRefusal::Limit)],
        vec![(1, ResolveRefusal::Limit), (0, ResolveRefusal::Unsupported)],
    ] {
        let mut registry = registry(vec![
            template("Outer", vec![("value", name("T"))]),
            template("Dependency", vec![("value", name("T"))]),
            template("Sibling", vec![("value", name("T"))]),
        ]);
        let mut draft = fresh_draft();
        for template in 0..3 {
            registry
                .mint_type_instance(
                    &mut draft,
                    template,
                    &[GArg::Scalar(ScalarType::Int)],
                    site(template as u32 + 1),
                )
                .expect("seed row mints ready");
        }

        let outer_id = {
            let mut generics = registry.generics.borrow_mut();
            for inst in &mut generics.type_insts {
                let prior = std::mem::replace(
                    &mut inst.state,
                    TypeInstState::Rejected(ResolveRefusal::Unsupported),
                );
                let TypeInstState::Ready(body) = prior else {
                    panic!("seed row is ready")
                };
                inst.state = TypeInstState::Filling { staged: Some(body) };
            }
            generics.fill_batch_start = Some(0);
            generics.fill_rows = generics
                .type_insts
                .iter()
                .enumerate()
                .map(|(index, inst)| (TypeInstKey::from(inst.id), index))
                .collect();
            // Outer depends on Dependency; Sibling is in the same batch but has
            // no path to either refusal and must remain Ready.
            generics.type_insts[1].dependents.push(0);
            generics.fill_failures = failures;
            generics.type_insts[0].id
        };

        registry
            .settle_fill_batch()
            .expect("well-formed provisional batch settles");
        assert_eq!(
            registry.settled_type_result(0, outer_id, AnyReadyInstance),
            Err(ResolveError::Refusal(ResolveRefusal::Limit)),
            "the settled row, not its local Unsupported, owns the return"
        );
        assert!(matches!(
            row(&registry, "Outer").state,
            TypeInstState::Rejected(ResolveRefusal::Limit)
        ));
        assert!(matches!(
            row(&registry, "Dependency").state,
            TypeInstState::Rejected(ResolveRefusal::Limit)
        ));
        assert!(matches!(
            row(&registry, "Sibling").state,
            TypeInstState::Ready(_)
        ));
        let generics = registry.generics.borrow();
        assert!(generics.fill_batch_start.is_none());
        assert!(generics.fill_rows.is_empty());
        assert!(generics.fill_stack.is_empty());
        assert!(generics.fill_failures.is_empty());
        assert_eq!(generics.fill_failures.capacity(), 0);
        assert!(generics.type_insts.iter().all(|inst| {
            !matches!(inst.state, TypeInstState::Filling { .. })
                && inst.dependents.is_empty()
                && inst.dependents.capacity() == 0
        }));
    }
}

#[test]
fn divergent_limit_rejects_dependents_and_reports_once() {
    let mut registry = registry(vec![template(
        "Grow",
        vec![("next", apply("Grow", vec![apply("List", vec![name("T")])]))],
    )]);
    let mut draft = fresh_draft();
    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(10),),
        Err(ResolveError::Refusal(ResolveRefusal::Limit))
    );
    let (first_row_id, before) = {
        let generics = registry.generics.borrow();
        (generics.type_insts[0].id, generics.type_insts.len())
    };
    assert!(
        registry
            .generics
            .borrow()
            .type_insts
            .iter()
            .all(|inst| matches!(inst.state, TypeInstState::Rejected(ResolveRefusal::Limit)))
    );
    assert!(registry.generics.borrow().fill_stack.is_empty());
    let first = ordered(registry.take_generic_diagnostics());
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].code(), Code::CheckInstantiationLimit.as_str());
    assert_eq!((first[0].line(), first[0].column()), (10, 9));
    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(20),),
        Err(ResolveError::Refusal(ResolveRefusal::Limit))
    );
    let generics = registry.generics.borrow();
    assert_eq!(generics.type_insts.len(), before);
    assert_eq!(generics.type_insts[0].id, first_row_id);
    drop(generics);
    assert!(ordered(registry.take_generic_diagnostics()).is_empty());
}

#[test]
fn rejected_rows_are_displayable_but_not_semantic_or_anchor_ready() {
    let mut registry = registry(vec![enum_template(
        "Bad",
        apply("Missing", vec![name("T")]),
    )]);
    let mut draft = fresh_draft();
    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(10),),
        Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
    );
    let id = row(&registry, "Bad").id;
    let TypeInstId::Enum(enum_id) = id else {
        panic!("enum template reserves an enum id")
    };
    assert_eq!(registry.inst_spelling(id).as_deref(), Some("Bad<int>"));
    assert!(registry.instantiation_of(id).unwrap().is_none());
    assert!(registry.type_inst_body(id).unwrap().is_none());
    assert!(registry.enum_variants(enum_id).unwrap().is_none());
    assert!(registry.enum_anchor_spelling(enum_id).unwrap().is_none());
    assert!(
        ValueGraph::build(&registry)
            .unwrap()
            .nodes
            .iter()
            .all(|node| *node != ValueNode::Enum(enum_id))
    );

    registry.generics.borrow_mut().type_insts[0].state = TypeInstState::Filling { staged: None };
    assert!(registry.inst_spelling(id).is_none());
    assert!(registry.instantiation_of(id).unwrap().is_none());
    assert!(registry.type_inst_body(id).unwrap().is_none());
    assert!(registry.enum_variants(enum_id).unwrap().is_none());
    assert!(registry.enum_anchor_spelling(enum_id).unwrap().is_none());
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
}

#[test]
fn template_proof_savepoint_isolates_a_failed_proof_and_transfers_once() {
    let mut registry = registry(vec![
        template("Leaf", vec![("value", name("T"))]),
        enum_template("Choice", apply("Leaf", vec![name("T")])),
        template(
            "Composite",
            vec![
                ("scalar", name("T")),
                ("record", apply("Leaf", vec![name("T")])),
                ("enum", apply("Choice", vec![name("T")])),
                ("collection", apply("List", vec![name("T")])),
            ],
        ),
    ]);
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let scalar = GArg::Scalar(ScalarType::Int);
    let leaf_id = registry
        .mint_type_instance(&mut draft, 0, &[scalar], site(2))
        .expect("stable record seed mints");
    let TypeInstId::Record(leaf_record) = leaf_id else {
        panic!("Leaf is a struct template")
    };
    let choice_id = registry
        .mint_type_instance(&mut draft, 1, &[scalar], site(3))
        .expect("stable enum seed mints");
    let TypeInstId::Enum(choice_enum) = choice_id else {
        panic!("Choice is an enum template")
    };
    let collection = registry
        .instantiate_list(&mut draft, scalar)
        .expect("aligned collection owners mint");
    let composite_id = registry
        .mint_type_instance(&mut draft, 2, &[scalar], site(4))
        .expect("representative record seed mints");
    registry
        .set_fn_base(37)
        .expect("a test base fits the function index carrier");
    let reserved = registry
        .reserve_fn_instance(7, vec![scalar], site(5))
        .expect("stable function row reserves");
    assert_eq!(reserved, 37);
    let before = stable_snapshot(&registry);
    assert_eq!(
        before.rows,
        vec![
            StableRow {
                template: 0,
                args: vec![scalar],
                id: leaf_id,
                state: StableRowState::Ready,
                body: Some(StableBody::Struct(vec![("value".to_string(), scalar)])),
                dependents: Vec::new(),
            },
            StableRow {
                template: 1,
                args: vec![scalar],
                id: choice_id,
                state: StableRowState::Ready,
                body: Some(StableBody::Enum(vec![(
                    "value".to_string(),
                    vec![("item".to_string(), GArg::Struct(leaf_record))],
                )])),
                dependents: Vec::new(),
            },
            StableRow {
                template: 2,
                args: vec![scalar],
                id: composite_id,
                state: StableRowState::Ready,
                body: Some(StableBody::Struct(vec![
                    ("scalar".to_string(), scalar),
                    ("record".to_string(), GArg::Struct(leaf_record)),
                    ("enum".to_string(), GArg::Enum(choice_enum)),
                    ("collection".to_string(), GArg::Collection(collection)),
                ])),
                dependents: Vec::new(),
            },
        ]
    );
    assert_eq!(before.collections, vec![CollSpec::List { elem: scalar }]);
    assert_eq!(before.fn_base, 37);
    assert_eq!(before.functions, vec![(7, vec![scalar], reserved)]);
    assert_eq!(before.queue, vec![(7, vec![scalar], reserved)]);
    let draft_before = draft_snapshot(&draft);

    let proof = registry
        .enter_template_proof(draft.record_type_count(), draft.enum_type_count())
        .expect("a settled open registry admits the proof pass");

    draft.commit();
    let outcome = {
        let mut proof_txn = admitted(&mut draft_owner);
        let proof_draft = &mut proof_txn;
        // The proof pass mints and diagnoses directly on the real registry and draft.
        let text = GArg::Scalar(ScalarType::Text);
        let proof_row = registry
            .mint_type_instance(proof_draft, 0, &[text], site(28))
            .expect("the proof mints a new isolated row on the real registry");
        assert!(matches!(proof_row, TypeInstId::Record(_)));
        let marker = proof_draft
            .intern_string("during-proof")
            .expect("a within-domain mint");
        proof_draft
            .add_record_type(RecordTypeDef {
                name: marker,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        let proof_collection = registry
            .instantiate_list(proof_draft, text)
            .expect("the proof mints a distinct collection on the real registry");
        assert_eq!(
            registry.collections.borrow().len(),
            2,
            "the proof appended its own collection row",
        );
        registry.record_collection_payload_rejection(
            site(29),
            "Payload",
            "value",
            proof_collection,
        );
        registry.record_limit(site(30), "the proof reached its local bound");

        // Simulate a proof that failed mid-fill, leaving the transient batch state dirty:
        // the guard must still restore the settled owner exactly. The dirty edges
        // reference only the appended row, which truncation drops.
        {
            let mut generics = registry.generics.borrow_mut();
            let dirty_row = generics.type_insts.len() - 1;
            let key = TypeInstKey::from(generics.type_insts[dirty_row].id);
            generics.fill_batch_start = Some(dirty_row);
            generics.fill_rows.insert(key, dirty_row);
            generics.fill_stack.push(dirty_row);
            generics.type_insts[dirty_row].dependents.push(dirty_row);
        }

        let outcome = registry.take_generic_diagnostics();
        registry.restore_generic_owners(proof);
        outcome
        // The armed guard drops here, discarding everything the proof appended.
    };
    let draft = admitted(&mut draft_owner);

    // The failed proof leaked nothing: the settled registry and the draft bytes are
    // exactly what they were before the pass.
    assert_eq!(
        stable_snapshot(&registry),
        before,
        "a failed proof leaves the settled registry structurally identical",
    );
    assert_eq!(
        draft_snapshot(&draft),
        draft_before,
        "a failed proof leaves the draft byte-identical",
    );

    // Only the proof's diagnostics cross back, transferred once in owner order.
    registry.adopt_generic_diagnostics(outcome);
    let adopted = ordered(registry.take_generic_diagnostics());
    assert_eq!(adopted.len(), 2);
    assert_eq!(adopted[0].code(), Code::CheckInstantiationLimit.as_str());
    assert_eq!(adopted[1].code(), Code::CheckUnsupported.as_str());
    assert!(ordered(registry.take_generic_diagnostics()).is_empty());
}

#[test]
fn template_proof_validates_every_ready_row_even_when_ids_are_duplicated() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("first Box row mints ready");
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(3))
        .expect("second Box row mints ready");
    let duplicate = {
        let mut generics = registry.generics.borrow_mut();
        let first_id = generics.type_insts[0].id;
        generics.type_insts[1].id = first_id;
        first_id
    };
    // The row identity was corrupted out of the append order, so the classified
    // directory must be discarded before a probe reclassifies the owners.
    registry.invalidate_row_directory();
    let expected = GenericInvariant::TypeIdentityCollision(duplicate);
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert!(matches!(
        registry.mint_type_instance(
            &mut draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(5),
        ),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert_eq!(registry.inst_spelling(duplicate), None);
    let arg = match duplicate {
        TypeInstId::Record(id) => GArg::Struct(id),
        TypeInstId::Enum(id) => GArg::Enum(id),
    };
    assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

/// A fill copies no template-body entries, whatever the instantiation count — the figure,
/// counted where a copy would happen rather than inferred from a process footprint.
///
/// A fill must read the declared entries while minting through the exclusively held
/// registry, and it reaches them through a handle the template still holds rather than a
/// body of its own. The counter charges a fill for the entries it owns privately, so a
/// fill that stops sharing charges itself here without the counting call changing.
///
/// No aggregate stands in for this. The issuance RSS gate measures an **aggregate resident
/// peak**, which cannot attribute a figure to one term, and its divergent corpora stop on
/// the 256-deep mint bound rather than the 4096-wide instantiation ceiling, so "driven to
/// the ceiling" is not a safe proxy for the number of copies either.
#[test]
fn a_fill_copies_no_template_body_entries() {
    const FIELDS: usize = 7;
    let names: Vec<String> = (0..FIELDS).map(|field| format!("f{field}")).collect();
    let fields: Vec<(&str, TypeExpr)> = names
        .iter()
        .map(|field| (field.as_str(), name("T")))
        .collect();

    let arguments = [
        GArg::Scalar(ScalarType::Int),
        GArg::Scalar(ScalarType::Bool),
        GArg::Scalar(ScalarType::Text),
    ];

    // Both fills, in one window: the struct arm and the enum arm reach their declared
    // entries by different code, so a figure over one of them says nothing about the other.
    let (_, counts) = crate::types::capture_scaling_counts(|| {
        let mut registry = registry(vec![
            template("Wide", fields),
            enum_template("Wrap", name("T")),
        ]);
        let mut draft = fresh_draft();
        for template in [0, 1] {
            for (index, argument) in arguments.iter().enumerate() {
                registry
                    .mint_type_instance(&mut draft, template, &[*argument], site(index as u32 + 2))
                    .expect("each distinct argument mints its own ready row");
            }
            // A repeated argument is deduped by the mint, so it reaches no body at all: the
            // term is linear in the *instantiation* count, not in the call count.
            registry
                .mint_type_instance(&mut draft, template, &[arguments[0]], site(9))
                .expect("a repeated argument reuses the row it already minted");
        }
    });

    assert_eq!(
        counts.template_body_clone_entries,
        0,
        "no declared body is copied at all — {} instantiations each of a {FIELDS}-field \
         struct template and a one-payload enum template",
        arguments.len(),
    );
}

/// A failed extension returns the admitted row directory to the registry.
///
/// The directory is taken out of its cell before the fallible build and extension run, so
/// an invariant leaving that scope used to drop it — and the pass paid for a cold rebuild
/// of a classification it had already completed. The existing red covers the other arm
/// (`None -> Some -> None`, a batch that opened the first directory leaving none behind);
/// this is the `Some -> fallible extension -> Some` arm it did not reach.
#[test]
fn a_failed_extension_returns_the_admitted_row_directory() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("first Box row mints ready");
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(3))
        .expect("second Box row mints ready");
    assert!(
        registry.row_directory.borrow().is_some(),
        "the mints probed, so the pass holds a classified directory to lose",
    );

    // A row appended *after* that probe carries row 0's identity, so the next extension
    // collides while classifying it — the fallible path, reached with a live cache.
    let duplicate = {
        let mut generics = registry.generics.borrow_mut();
        let clash = generics.type_insts[0].clone();
        generics.type_insts.push(clash);
        generics.type_insts[0].id
    };
    let expected = GenericInvariant::TypeIdentityCollision(duplicate);

    // The probe is a mint, which is what routes through the cached directory. A checker
    // that reaches the collision by its own scan never enters the extension at all, so it
    // cannot observe whether the cache survived it.
    assert!(matches!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Text)], site(4)),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert!(
        registry.row_directory.borrow().is_some(),
        "a failed extension returns the admitted directory; dropping it costs the pass the \
         classification it already paid for and leaves the registry holding none",
    );

    // And the returned directory is the one it received, not a half-extended one: the
    // watermark still describes exactly the rows the scratch classifies, so a second probe
    // reaches the same verdict rather than a different one.
    assert!(matches!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Text)], site(5)),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
}

#[test]
fn metadata_rejects_distinct_ids_with_the_same_semantic_cache_key() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let first = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(3))
        .expect("first Box row mints ready");
    let duplicate = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(4))
        .expect("second Box row mints ready");
    {
        let mut generics = registry.generics.borrow_mut();
        let first_args = generics.type_insts[0].args.clone();
        let first_state = generics.type_insts[0].state.clone();
        generics.type_insts[1].args = first_args;
        generics.type_insts[1].state = first_state;
    }
    // The row was corrupted out of the append order, so the classified directory
    // must be discarded before a probe reclassifies the owners.
    registry.invalidate_row_directory();
    let expected = GenericInvariant::TypeInstantiationKeyCollision { first, duplicate };
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert!(matches!(
        registry.mint_type_instance(
            &mut draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(6),
        ),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert_eq!(registry.inst_spelling(duplicate), None);
    let arg = match duplicate {
        TypeInstId::Record(id) => GArg::Struct(id),
        TypeInstId::Enum(id) => GArg::Enum(id),
    };
    assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn metadata_rejects_generic_ids_owned_by_declared_types() {
    let mut record_registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut record_draft = fresh_draft();
    let declared_record =
        add_declared_struct(&mut record_registry, &mut record_draft, "Plain", Vec::new());
    record_registry
        .mint_type_instance(
            &mut record_draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(4),
        )
        .expect("generic record mints ready");
    record_registry.generics.borrow_mut().type_insts[0].id = TypeInstId::Record(declared_record);
    // The row identity was corrupted out of the append order, so the classified
    // directory must be discarded before a probe reclassifies the owners.
    record_registry.invalidate_row_directory();
    let record_expected =
        GenericInvariant::TypeIdentityCollision(TypeInstId::Record(declared_record));
    let record_before = stable_snapshot(&record_registry);
    let record_draft_before = draft_snapshot(&record_draft);

    assert!(matches!(
        validate_ready_metadata(&record_registry),
        Err(found) if found == record_expected
    ));
    let (record_body, builds) = count_metadata_directory_builds(|| {
        record_registry.type_inst_body(TypeInstId::Record(declared_record))
    });
    assert!(matches!(record_body, Err(found) if found == record_expected));
    assert_eq!(builds, 1);
    assert!(matches!(
        record_registry.static_struct_projection("Plain"),
        Err(found) if found == record_expected
    ));
    assert!(matches!(
        record_registry.static_named_type_projection("Plain"),
        Err(found) if found == record_expected
    ));
    assert_eq!(
        record_registry.struct_field_projection(declared_record, "missing"),
        Err(record_expected)
    );
    assert_eq!(
        garg_anchor_spelling(&record_registry, GArg::Struct(declared_record)),
        Err(record_expected)
    );
    assert_eq!(stable_snapshot(&record_registry), record_before);
    assert_eq!(draft_snapshot(&record_draft), record_draft_before);

    let mut enum_registry = registry(vec![enum_template("Choice", name("T"))]);
    let mut enum_draft = fresh_draft();
    let declared_name = enum_draft
        .intern_string("PlainChoice")
        .expect("a within-domain mint");
    let declared_enum = enum_draft
        .add_enum_type(EnumTypeDef {
            name: declared_name,
            variants: Vec::new(),
        })
        .expect("a within-domain mint");
    enum_registry.enums.push(EnumInfo {
        enum_id: declared_enum,
        name: "PlainChoice".to_string(),
        variants: Vec::new(),
        verdict: DeclarationVerdict::Accepted,
    });
    enum_registry
        .mint_type_instance(
            &mut enum_draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(5),
        )
        .expect("generic enum mints ready");
    enum_registry.generics.borrow_mut().type_insts[0].id = TypeInstId::Enum(declared_enum);
    // The row identity was corrupted out of the append order, so the classified
    // directory must be discarded before a probe reclassifies the owners.
    enum_registry.invalidate_row_directory();
    let enum_expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Enum(declared_enum));
    let enum_before = stable_snapshot(&enum_registry);
    let enum_draft_before = draft_snapshot(&enum_draft);

    assert!(matches!(
        validate_ready_metadata(&enum_registry),
        Err(found) if found == enum_expected
    ));
    let (variants, builds) =
        count_metadata_directory_builds(|| enum_registry.enum_variants(declared_enum));
    assert_eq!(variants, Err(enum_expected));
    assert_eq!(builds, 1);
    assert!(matches!(
        enum_registry.static_enum_projection("PlainChoice"),
        Err(found) if found == enum_expected
    ));
    assert!(matches!(
        enum_registry.static_named_type_projection("PlainChoice"),
        Err(found) if found == enum_expected
    ));
    let (anchor, builds) =
        count_metadata_directory_builds(|| enum_registry.enum_anchor_spelling(declared_enum));
    assert_eq!(anchor, Err(enum_expected));
    assert_eq!(builds, 1);
    assert_eq!(
        garg_anchor_spelling(&enum_registry, GArg::Enum(declared_enum)),
        Err(enum_expected)
    );
    assert_eq!(stable_snapshot(&enum_registry), enum_before);
    assert_eq!(draft_snapshot(&enum_draft), enum_draft_before);
}

#[test]
fn metadata_rejects_generic_ids_owned_by_resource_records() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let resource = add_resource_record(&mut registry, &mut draft, "Account");
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(5))
        .expect("generic record mints ready");
    registry.generics.borrow_mut().type_insts[0].id = TypeInstId::Record(resource);
    // The row identity was corrupted out of the append order, so the classified
    // directory must be discarded before a probe reclassifies the owners.
    registry.invalidate_row_directory();
    let expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Record(resource));
    let owner_before = metadata_owner_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert!(matches!(
        registry.mint_type_instance(
            &mut draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(6),
        ),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(
        registry.instantiation_of(TypeInstId::Record(resource)),
        Err(expected)
    );
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert!(matches!(
        registry.type_inst_body(TypeInstId::Record(resource)),
        Err(found) if found == expected
    ));
    assert!(matches!(
        registry.static_record_projection("Account"),
        Err(found) if found == expected
    ));
    assert_eq!(
        registry.product_field_projection(resource, "missing"),
        Err(expected)
    );
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(registry.inst_spelling(TypeInstId::Record(resource)), None);
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(
        garg_anchor_spelling(&registry, GArg::Struct(resource)),
        Err(expected)
    );
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Scalar(ScalarType::Int)]),
        Err(expected)
    );
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
}

#[test]
fn metadata_rejects_resource_record_collisions_with_static_record_owners() {
    let mut struct_registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut struct_draft = fresh_draft();
    let resource = add_resource_record(&mut struct_registry, &mut struct_draft, "Account");
    add_declared_struct(&mut struct_registry, &mut struct_draft, "Plain", Vec::new());
    struct_registry.structs[0].type_id = resource;
    let expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Record(resource));
    let owner_before = metadata_owner_snapshot(&struct_registry);
    let draft_before = draft_snapshot(&struct_draft);

    assert!(matches!(
        validate_ready_metadata(&struct_registry),
        Err(found) if found == expected
    ));
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );
    assert!(matches!(
        struct_registry.mint_type_instance(
            &mut struct_draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(7),
        ),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );
    assert_eq!(
        struct_registry.validate_type_arguments(&[GArg::Struct(resource)]),
        Err(expected)
    );
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );
    assert_eq!(
        struct_registry.instantiation_of(TypeInstId::Record(resource)),
        Err(expected)
    );
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );
    assert_eq!(
        garg_anchor_spelling(&struct_registry, GArg::Struct(resource)),
        Err(expected)
    );
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );
    assert_eq!(
        struct_registry.validate_durable_value_metadata([GArg::Scalar(ScalarType::Int)]),
        Err(expected)
    );
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );
    assert!(matches!(
        ValueGraph::build(&struct_registry),
        Err(found) if found == expected
    ));
    assert_metadata_unchanged(
        &struct_registry,
        &struct_draft,
        &owner_before,
        &draft_before,
    );

    let mut group_registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut group_draft = fresh_draft();
    let resource = add_resource_record(&mut group_registry, &mut group_draft, "Account");
    add_resource_group(&mut group_registry, &mut group_draft, 0, "profile");
    group_registry.records[0].groups[0].type_id = resource;
    let expected = GenericInvariant::TypeIdentityCollision(TypeInstId::Record(resource));
    let owner_before = metadata_owner_snapshot(&group_registry);
    let draft_before = draft_snapshot(&group_draft);

    assert!(matches!(
        validate_ready_metadata(&group_registry),
        Err(found) if found == expected
    ));
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    assert!(matches!(
        group_registry.mint_type_instance(
            &mut group_draft,
            0,
            &[GArg::Scalar(ScalarType::Int)],
            site(8),
        ),
        Err(ResolveError::Invariant(found)) if found == expected
    ));
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    assert_eq!(
        group_registry.validate_type_arguments(&[GArg::Group(resource)]),
        Err(expected)
    );
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    assert_eq!(
        group_registry.instantiation_of(TypeInstId::Record(resource)),
        Err(expected)
    );
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    assert_eq!(
        garg_anchor_spelling(&group_registry, GArg::Group(resource)),
        Err(expected)
    );
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    assert_eq!(
        group_registry.validate_durable_value_metadata([GArg::Scalar(ScalarType::Int)]),
        Err(expected)
    );
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
    assert!(matches!(
        ValueGraph::build(&group_registry),
        Err(found) if found == expected
    ));
    assert_metadata_unchanged(&group_registry, &group_draft, &owner_before, &draft_before);
}

#[test]
fn metadata_rejects_cyclic_ready_arguments_before_display_or_durable_use() {
    let mut registry = registry(vec![
        template("Inner", vec![("value", name("T"))]),
        template("Outer", vec![("value", name("T"))]),
    ]);
    let mut draft = fresh_draft();
    let inner = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(6))
        .expect("Inner row mints ready");
    let TypeInstId::Record(inner_id) = inner else {
        panic!("Inner is a record template")
    };
    let outer = registry
        .mint_type_instance(&mut draft, 1, &[GArg::Struct(inner_id)], site(7))
        .expect("Outer row may depend on an earlier row");
    let TypeInstId::Record(outer_id) = outer else {
        panic!("Outer is a record template")
    };

    assert_eq!(
        registry.inst_spelling(outer).as_deref(),
        Some("Outer<Inner<int>>")
    );
    assert_eq!(
        garg_anchor_spelling(&registry, GArg::Struct(outer_id)).as_deref(),
        Ok("Outer[Inner[int]]")
    );
    registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Struct(outer_id)];
    let expected = GenericInvariant::TypeArgumentOrderViolation {
        owner: inner,
        target: outer,
    };
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert_eq!(registry.inst_spelling(inner), None);
    let view = registry.metadata_view();
    let mut metadata = MetadataScratch::try_new(&view).expect("identity directory remains valid");
    assert_eq!(
        view.validate_args_with(
            std::slice::from_ref(&GArg::Struct(inner_id)),
            None,
            &mut metadata,
        ),
        Err(expected),
    );
    drop(view);
    assert_eq!(
        garg_anchor_spelling(&registry, GArg::Struct(inner_id)),
        Err(expected)
    );
    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Struct(inner_id)]),
        Err(expected)
    );
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn generic_predecessor_order_survives_collection_expansion() {
    let mut root_template = template("Root", Vec::new());
    root_template.type_params = vec![("T".to_string(), None), ("U".to_string(), None)];
    let mut registry = registry(vec![
        template("Inner", vec![("value", name("T"))]),
        template("Outer", vec![("value", name("T"))]),
        root_template,
    ]);
    let mut draft = fresh_draft();
    let inner = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(8))
        .expect("Inner row mints ready");
    let TypeInstId::Record(inner_id) = inner else {
        panic!("Inner is a record template")
    };
    let outer = registry
        .mint_type_instance(&mut draft, 1, &[GArg::Struct(inner_id)], site(9))
        .expect("Outer row may depend on an earlier row");
    let TypeInstId::Record(outer_id) = outer else {
        panic!("Outer is a record template")
    };
    let list = registry
        .instantiate_list(&mut draft, GArg::Struct(outer_id))
        .expect("collection over a valid predecessor graph mints");
    let nested = registry
        .instantiate_list(&mut draft, GArg::Collection(list))
        .expect("a predecessor collection may be nested");
    let root = registry
        .mint_type_instance(
            &mut draft,
            2,
            &[GArg::Collection(nested), GArg::Struct(inner_id)],
            site(10),
        )
        .expect("Root arguments point only to predecessors");
    let TypeInstId::Record(root_id) = root else {
        panic!("Root is a record template")
    };
    let expected_label = "Root<List<List<Outer<Inner<int>>>>, Inner<int>>";
    assert_eq!(
        registry.inst_spelling(root).as_deref(),
        Some(expected_label)
    );
    let graph = ValueGraph::build(&registry).expect("the valid predecessor graph builds");
    let label_for = |id| {
        let node = graph
            .nodes
            .iter()
            .position(|node| *node == ValueNode::Record(id))
            .expect("the generic record has one graph node");
        graph.labels[node].as_str()
    };
    assert_eq!(label_for(inner_id), "Inner<int>");
    assert_eq!(label_for(outer_id), "Outer<Inner<int>>");
    assert_eq!(label_for(root_id), expected_label);
    assert!(graph.labels.iter().all(|label| label != "instantiation"));

    let carrier = add_declared_struct(
        &mut registry,
        &mut draft,
        "Carrier",
        vec![
            ("collection", GArg::Collection(nested)),
            ("inner", GArg::Struct(inner_id)),
        ],
    );
    registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Collection(nested)];
    let expected = GenericInvariant::TypeArgumentOrderViolation {
        owner: inner,
        target: outer,
    };
    let owner_before = metadata_owner_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    for roots in [
        vec![GArg::Collection(nested), GArg::Struct(inner_id)],
        vec![GArg::Struct(inner_id), GArg::Collection(nested)],
    ] {
        let view = registry.metadata_view();
        let mut metadata =
            MetadataScratch::try_new(&view).expect("identity directory remains valid");
        assert_eq!(
            view.validate_args_with(&roots, None, &mut metadata),
            Err(expected),
            "root order cannot change predecessor validation",
        );
        drop(view);
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            registry.validate_durable_value_metadata(roots),
            Err(expected),
            "durable root order cannot change predecessor validation",
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    }
    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(registry.inst_spelling(inner), None);
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(registry.inst_spelling(root), None);
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    for arg in [
        GArg::Struct(inner_id),
        GArg::Collection(nested),
        GArg::Struct(root_id),
    ] {
        assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    }
    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Struct(root_id)]),
        Err(expected)
    );
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Struct(carrier)]),
        Err(expected)
    );
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
}

#[test]
fn collection_predecessor_validation_preserves_missing_target_precedence() {
    for (corrupt, missing) in [(0, coll(0)), (1, coll(1)), (2, coll(2))] {
        let mut registry = registry(vec![template("A", vec![("value", name("T"))])]);
        let mut draft = fresh_draft();
        let a = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(11))
            .expect("A<int> mints ready");
        let first = registry
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("first collection mints aligned");
        assert_eq!(first, coll(0));
        if corrupt == 1 {
            let second = registry
                .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Bool))
                .expect("forward collection target exists");
            assert_eq!(second, coll(1));
        }
        registry.collections.borrow_mut()[0] = CollSpec::List {
            elem: GArg::Collection(missing),
        };
        registry.generics.borrow_mut().type_insts[0].args = vec![GArg::Collection(coll(0))];
        let owner_before = metadata_owner_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);
        assert_eq!(
            registry.validate_type_arguments(&[match a {
                TypeInstId::Record(id) => GArg::Struct(id),
                TypeInstId::Enum(id) => GArg::Enum(id),
            }]),
            Err(GenericInvariant::TypeArgumentTargetMissing(
                GArg::Collection(missing),
            )),
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert!(matches!(
            validate_ready_metadata(&registry),
            Err(GenericInvariant::TypeArgumentTargetMissing(GArg::Collection(found)))
                if found == missing
        ));
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Collection(coll(0))),
            Err(GenericInvariant::TypeArgumentTargetMissing(
                GArg::Collection(missing),
            )),
        );
        assert_metadata_unchanged(&registry, &draft, &owner_before, &draft_before);
    }

    let registry = registry(Vec::new());
    *registry.collections.borrow_mut() = vec![
        CollSpec::List {
            elem: GArg::Scalar(ScalarType::Int),
        },
        CollSpec::List {
            elem: GArg::Collection(coll(0)),
        },
    ];
    let before = stable_snapshot(&registry);
    assert_eq!(
        registry.validate_type_arguments(&[GArg::Collection(coll(1))]),
        Ok(())
    );
    assert_eq!(stable_snapshot(&registry), before);
}

#[test]
fn collection_predecessor_validation_preserves_source_order_on_first_visit_and_revisit() {
    struct Rows {
        registry: TypeRegistry,
        draft: DraftTxn<'static>,
        owner: TypeInstId,
        forward: TypeInstId,
        later: TypeInstId,
        orphan: TypeId,
    }

    fn rows() -> Rows {
        let mut registry = registry(vec![
            template("Earlier", vec![("value", name("T"))]),
            template("Owner", vec![("value", name("T"))]),
            template("Forward", vec![("value", name("T"))]),
            template("Later", vec![("value", name("T"))]),
        ]);
        let draft_owner: &'static mut ImageDraft = Box::leak(Box::new(ImageDraft::new()));
        let mut draft = admitted(draft_owner);
        let mut ids = Vec::new();
        for template in 0..4 {
            ids.push(
                registry
                    .mint_type_instance(
                        &mut draft,
                        template,
                        &[GArg::Scalar(ScalarType::Int)],
                        site(template as u32 + 20),
                    )
                    .expect("ordered seed row mints Ready"),
            );
        }
        let orphan_name = draft.intern_string("Orphan").expect("a within-domain mint");
        let orphan = draft
            .add_record_type(RecordTypeDef {
                name: orphan_name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        Rows {
            registry,
            draft,
            owner: ids[1],
            forward: ids[2],
            later: ids[3],
            orphan,
        }
    }

    let missing = rows();
    let TypeInstId::Record(forward) = missing.forward else {
        panic!("Forward is record-shaped")
    };
    missing
        .registry
        .collections
        .borrow_mut()
        .push(CollSpec::Map {
            key: GArg::Struct(missing.orphan),
            value: GArg::Struct(forward),
        });
    missing.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(coll(0))];
    let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(missing.orphan));
    let owner_before = metadata_owner_snapshot(&missing.registry);
    let draft_before = draft_snapshot(&missing.draft);
    let (observed, builds) = count_metadata_directory_builds(|| {
        missing
            .registry
            .validate_type_arguments(&[match missing.owner {
                TypeInstId::Record(id) => GArg::Struct(id),
                TypeInstId::Enum(id) => GArg::Enum(id),
            }])
    });
    assert_eq!(observed, Err(expected));
    assert_eq!(builds, 1);
    assert_metadata_unchanged(
        &missing.registry,
        &missing.draft,
        &owner_before,
        &draft_before,
    );

    let ordered = rows();
    let TypeInstId::Record(forward) = ordered.forward else {
        panic!("Forward is record-shaped")
    };
    let TypeInstId::Record(later) = ordered.later else {
        panic!("Later is record-shaped")
    };
    ordered
        .registry
        .collections
        .borrow_mut()
        .push(CollSpec::Map {
            key: GArg::Struct(forward),
            value: GArg::Struct(later),
        });
    ordered.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(coll(0))];
    let expected = GenericInvariant::TypeArgumentOrderViolation {
        owner: ordered.owner,
        target: ordered.forward,
    };
    let owner_before = metadata_owner_snapshot(&ordered.registry);
    let draft_before = draft_snapshot(&ordered.draft);
    assert_eq!(
        ordered
            .registry
            .validate_type_arguments(&[match ordered.owner {
                TypeInstId::Record(id) => GArg::Struct(id),
                TypeInstId::Enum(id) => GArg::Enum(id),
            }]),
        Err(expected),
        "Map key order wins over a later value violation"
    );
    assert_metadata_unchanged(
        &ordered.registry,
        &ordered.draft,
        &owner_before,
        &draft_before,
    );

    let nested = rows();
    let TypeInstId::Record(forward) = nested.forward else {
        panic!("Forward is record-shaped")
    };
    *nested.registry.collections.borrow_mut() = vec![
        CollSpec::List {
            elem: GArg::Struct(nested.orphan),
        },
        CollSpec::Map {
            key: GArg::Collection(coll(0)),
            value: GArg::Struct(forward),
        },
    ];
    nested.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(coll(1))];
    let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(nested.orphan));
    assert_eq!(
        nested
            .registry
            .validate_type_arguments(&[match nested.owner {
                TypeInstId::Record(id) => GArg::Struct(id),
                TypeInstId::Enum(id) => GArg::Enum(id),
            }]),
        Err(expected),
        "nested key traversal precedes the Map value"
    );

    let revisit = rows();
    let TypeInstId::Record(forward) = revisit.forward else {
        panic!("Forward is record-shaped")
    };
    revisit
        .registry
        .collections
        .borrow_mut()
        .push(CollSpec::List {
            elem: GArg::Struct(forward),
        });
    revisit.registry.generics.borrow_mut().type_insts[1].args = vec![GArg::Collection(coll(0))];
    let owner_arg = match revisit.owner {
        TypeInstId::Record(id) => GArg::Struct(id),
        TypeInstId::Enum(id) => GArg::Enum(id),
    };
    let view = revisit.registry.metadata_view();
    let mut metadata = MetadataScratch::try_new(&view).expect("directory builds");
    assert_eq!(
        view.validate_args_with(&[GArg::Collection(coll(0)), owner_arg], None, &mut metadata,),
        Err(GenericInvariant::TypeArgumentOrderViolation {
            owner: revisit.owner,
            target: revisit.forward,
        }),
        "a root previsit cannot hide the later parent-context violation"
    );
}

#[test]
fn template_proof_refuses_every_unstable_fill_or_diagnostic_owner_state() {
    let mut registry = registry(vec![template("Good", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("stable seed mints");

    let id = registry.generics.borrow().type_insts[0].id;
    registry.generics.get_mut().build_invariant = Some(GenericInvariant::ReadyBodyMissing(id));
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
    registry.generics.get_mut().build_invariant = None;

    registry.generics.borrow_mut().fill_batch_start = Some(0);
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
    registry.generics.borrow_mut().fill_batch_start = None;

    let key = TypeInstKey::from(registry.generics.borrow().type_insts[0].id);
    registry.generics.borrow_mut().fill_rows.insert(key, 0);
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
    registry.generics.borrow_mut().fill_rows.clear();

    registry.generics.borrow_mut().fill_stack.push(0);
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
    registry.generics.borrow_mut().fill_stack.clear();

    registry
        .generics
        .borrow_mut()
        .fill_failures
        .push((0, ResolveRefusal::Unsupported));
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
    registry.generics.borrow_mut().fill_failures.clear();

    registry.generics.borrow_mut().type_insts[0]
        .dependents
        .push(0);
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
    registry.generics.borrow_mut().type_insts[0]
        .dependents
        .clear();

    registry.record_limit(site(9), "the real owner is no longer open");
    let pending = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::LimitOwnerNotOpen
        ))
    ));
    assert_eq!(stable_snapshot(&registry), pending);
    let _ = registry.take_generic_diagnostics();
    let reported = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::LimitOwnerNotOpen
        ))
    ));
    assert!(matches!(pending.limit, StableLimit::PendingRow(_)));
    assert!(matches!(reported.limit, StableLimit::Reported));
    assert_eq!(stable_snapshot(&registry), reported);
    registry.generics.borrow_mut().limit = LimitState::Open;

    let body = {
        let mut generics = registry.generics.borrow_mut();
        let prior = std::mem::replace(
            &mut generics.type_insts[0].state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported),
        );
        let TypeInstState::Ready(body) = prior else {
            panic!("seed row is ready")
        };
        body
    };
    registry.generics.borrow_mut().type_insts[0].state =
        TypeInstState::Filling { staged: Some(body) };
    let before = stable_snapshot(&registry);
    assert!(matches!(
        registry.enter_template_proof(0, 0),
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
}

/// An active mutable borrow of the generic owner is a private
/// coherence failure, not a RefCell unwind.
#[test]
fn template_proof_generics_borrow_conflict_fails_without_unwinding() {
    let registry = registry(Vec::new());
    let before = stable_snapshot(&registry);
    let guard = registry.generics.borrow_mut();
    let result = registry.enter_template_proof(0, 0);
    drop(guard);

    assert!(matches!(
        result,
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
}

/// Collection-owner contention is classified independently from
/// the generic owner and cannot unwind through RefCell.
#[test]
fn template_proof_collections_borrow_conflict_fails_without_unwinding() {
    let registry = registry(Vec::new());
    let before = stable_snapshot(&registry);
    let guard = registry.collections.borrow_mut();
    let result = registry.enter_template_proof(0, 0);
    drop(guard);

    assert!(matches!(
        result,
        Err(GenericInvariant::TemplateProof(
            TemplateProofError::UnstableFillState
        ))
    ));
    assert_eq!(stable_snapshot(&registry), before);
}

/// Committed reserved rows expose their exact arguments through
/// the dedicated Option/Result readers.
#[test]
fn ready_reserved_option_and_result_readers_preserve_arguments() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
        .expect("ready Option mints");
    let result_template = registry.reserved_template(Reserved::Result);
    let result = registry
        .mint_type_instance(
            &mut draft,
            result_template,
            &[
                GArg::Scalar(ScalarType::Text),
                GArg::Scalar(ScalarType::Bool),
            ],
            site(3),
        )
        .expect("ready Result mints");
    let TypeInstId::Enum(result) = result else {
        panic!("the reserved Result template is enum-shaped")
    };

    assert_eq!(
        registry.as_option(option),
        Ok(Some(GArg::Scalar(ScalarType::Int)))
    );
    assert_eq!(
        registry.as_result(result),
        Ok(Some((
            GArg::Scalar(ScalarType::Text),
            GArg::Scalar(ScalarType::Bool)
        )))
    );
    assert!(matches!(
        registry.type_inst_body(TypeInstId::Enum(option)),
        Ok(Some(InstBody::Enum(ref variants)))
            if variants.len() == 2
                && variants[0].name == "none"
                && variants[0].payload.is_empty()
                && variants[1].name == "some"
                && variants[1].payload
                    == vec![("value".to_string(), GArg::Scalar(ScalarType::Int))]
    ));
    assert!(matches!(
        registry.type_inst_body(TypeInstId::Enum(result)),
        Ok(Some(InstBody::Enum(ref variants)))
            if variants.len() == 2
                && variants[0].name == "ok"
                && variants[0].payload
                    == vec![("value".to_string(), GArg::Scalar(ScalarType::Text))]
                && variants[1].name == "err"
                && variants[1].payload
                    == vec![("value".to_string(), GArg::Scalar(ScalarType::Bool))]
    ));
    assert_eq!(
        registry.enum_anchor_spelling(option).unwrap().as_deref(),
        Some("Option[int]")
    );
    assert_eq!(
        registry.enum_anchor_spelling(result).unwrap().as_deref(),
        Some("Result[string,bool]")
    );
}

#[test]
fn reserved_readers_require_the_fixed_member_contract_not_only_template_agreement() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(4))
        .expect("ready Option mints");
    let option_template = registry.reserved_template(Reserved::Option);
    let option_row = registry
        .generics
        .borrow()
        .type_insts
        .iter()
        .position(|inst| inst.id == TypeInstId::Enum(option))
        .expect("Option row exists");
    let mut template_variants = {
        let TemplateBody::Enum(variants) = &registry.type_templates[option_template].body else {
            panic!("Option template is enum-shaped")
        };
        variants.to_vec()
    };
    template_variants[OPTION_NONE as usize].name = "nil".to_string();
    registry.type_templates[option_template].body = TemplateBody::Enum(template_variants.into());
    let mut generics = registry.generics.borrow_mut();
    let TypeInstState::Ready(InstBody::Enum(variants)) = &mut generics.type_insts[option_row].state
    else {
        panic!("Option row is Ready and enum-shaped")
    };
    variants[OPTION_NONE as usize].name = "nil".to_string();
    drop(generics);
    let expected = GenericInvariant::ReadyBodyShapeMismatch(TypeInstId::Enum(option));
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert_eq!(registry.as_option(option), Err(expected));
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn metadata_directory_builds_follow_immutable_operation_boundaries() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let (fresh, fresh_builds) = count_metadata_directory_builds(|| {
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
    });
    let id = fresh.expect("the cold Box row mints");
    assert_eq!(
        fresh_builds, 1,
        "the cold preflight builds the directory once; the post-settlement proof \
             extends that classification rather than rebuilding it"
    );

    let (replayed, replay_builds) = count_metadata_directory_builds(|| {
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(3))
    });
    assert_eq!(replayed, Ok(id));
    assert_eq!(
        replay_builds, 0,
        "a Ready cache hit reuses the classified directory with no rebuild"
    );

    let list = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("the List metadata mints");
    let (spelling, spelling_builds) =
        count_metadata_directory_builds(|| registry.collection_spelling(list));
    assert_eq!(spelling, "List<int>");
    assert_eq!(
        spelling_builds, 0,
        "best-effort presentation does not build the semantic directory"
    );

    let (blocked, session_builds) = count_metadata_directory_builds(|| {
        registry
            .with_metadata_session(|_| {
                Ok::<_, GenericInvariant>((
                    registry.generics.try_borrow_mut().is_err(),
                    registry.collections.try_borrow_mut().is_err(),
                ))
            })
            .expect("the immutable metadata session opens")
    });
    assert_eq!(blocked, (true, true));
    assert_eq!(
        session_builds, 0,
        "an out-of-line metadata session reuses the pass directory the append-only mint \
             path already built, extending it in place rather than rebuilding"
    );
    assert!(registry.generics.try_borrow_mut().is_ok());
    assert!(registry.collections.try_borrow_mut().is_ok());

    let map = registry
        .instantiate_map(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            GArg::Scalar(ScalarType::Text),
        )
        .expect("dropping the session permits a later metadata append");
    let (observed, post_append_builds) = count_metadata_directory_builds(|| {
        registry.with_metadata_session(|metadata| metadata.collection_spec(map))
    });
    assert_eq!(
        observed,
        Ok(CollSpec::Map {
            key: GArg::Scalar(ScalarType::Int),
            value: GArg::Scalar(ScalarType::Text),
        })
    );
    assert_eq!(
        post_append_builds, 0,
        "appending a collection extends the reused directory; a later session read \
             classifies only the appended row and never rebuilds"
    );
}

#[test]
fn validated_nested_collection_spelling_reuses_one_metadata_session() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let inner = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("the inner List metadata mints");
    let outer = registry
        .instantiate_map(
            &mut draft,
            GArg::Scalar(ScalarType::Int),
            GArg::Collection(inner),
        )
        .expect("the outer Map metadata mints");
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    let (spellings, builds) = count_metadata_directory_builds(|| {
        registry.with_metadata_session(|metadata| {
            Ok::<_, GenericInvariant>((
                metadata.garg_spelling(GArg::Collection(outer))?,
                metadata.garg_spelling(GArg::Collection(outer))?,
            ))
        })
    });

    assert_eq!(
        spellings,
        Ok((
            "Map<int, List<int>>".to_string(),
            "Map<int, List<int>>".to_string(),
        ))
    );
    assert_eq!(
        builds, 1,
        "repeated nested collection spelling reuses one validated directory"
    );
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn deep_collection_spelling_and_anchor_use_iterative_activity_owners() {
    let make_registry = registry;
    let registry = make_registry(Vec::new());
    let depth = MAX_INSTANTIATIONS;
    {
        let mut collections = registry.collections.borrow_mut();
        for index in 0..depth {
            let elem = if index == 0 {
                GArg::Scalar(ScalarType::Int)
            } else {
                GArg::Collection(coll((index - 1) as u16))
            };
            collections.push(CollSpec::List { elem });
        }
    }
    let root = coll((depth - 1) as u16);
    let owner_before = stable_snapshot(&registry);
    let expected_display = format!("{}int{}", "List<".repeat(depth), ">".repeat(depth));
    let expected_anchor = format!("{}int{}", "List[".repeat(depth), "]".repeat(depth));

    let (display, display_builds) =
        count_metadata_directory_builds(|| registry.collection_spelling(root));
    assert_eq!(display, expected_display);
    assert_eq!(display_builds, 0);
    let (anchor, anchor_builds) =
        count_metadata_directory_builds(|| garg_anchor_spelling(&registry, GArg::Collection(root)));
    assert_eq!(anchor, Ok(expected_anchor));
    assert_eq!(anchor_builds, 1);
    assert_eq!(stable_snapshot(&registry), owner_before);

    let cyclic = make_registry(Vec::new());
    cyclic.collections.borrow_mut().push(CollSpec::List {
        elem: GArg::Collection(coll(0)),
    });
    let cyclic_before = stable_snapshot(&cyclic);
    assert_eq!(cyclic.collection_spelling(coll(0)), "collection");
    assert_eq!(
        garg_anchor_spelling(&cyclic, GArg::Collection(coll(0))),
        Err(GenericInvariant::TypeArgumentTargetMissing(
            GArg::Collection(coll(0))
        ))
    );
    assert_eq!(stable_snapshot(&cyclic), cyclic_before);
}

#[test]
fn metadata_directory_construction_failure_never_enters_a_session() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let first = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("first Box row mints ready");
    let duplicate = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Bool)], site(3))
        .expect("second Box row mints ready");
    {
        let mut generics = registry.generics.borrow_mut();
        let first_args = generics.type_insts[0].args.clone();
        let first_state = generics.type_insts[0].state.clone();
        generics.type_insts[1].args = first_args;
        generics.type_insts[1].state = first_state;
    }
    // The duplicate key was written onto an already-classified settled row, out of the
    // append order the reused directory projects; the contract requires discarding that
    // classification so the next probe rebuilds and the cold semantic-key scan runs.
    registry.invalidate_row_directory();
    let expected = GenericInvariant::TypeInstantiationKeyCollision { first, duplicate };
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);
    let entered = Cell::new(false);

    let (observed, builds) = count_metadata_directory_builds(|| {
        registry.with_metadata_session(|_| {
            entered.set(true);
            Ok::<(), GenericInvariant>(())
        })
    });

    assert_eq!(observed, Err(expected));
    assert_eq!(builds, 1, "directory construction fails on its first pass");
    assert!(!entered.get(), "a failed directory never yields a session");
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn one_metadata_session_classifies_both_reserved_families() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
        .expect("Ready Option mints");
    let result_template = registry.reserved_template(Reserved::Result);
    let TypeInstId::Enum(result) = registry
        .mint_type_instance(
            &mut draft,
            result_template,
            &[
                GArg::Scalar(ScalarType::Text),
                GArg::Scalar(ScalarType::Bool),
            ],
            site(3),
        )
        .expect("Ready Result mints")
    else {
        panic!("Result is an enum template")
    };

    let (classified, builds) = count_metadata_directory_builds(|| {
        registry.with_metadata_session(|metadata| {
            Ok::<_, GenericInvariant>((
                metadata.reserved_instantiation(option)?,
                metadata.reserved_instantiation(result)?,
            ))
        })
    });
    assert_eq!(
        classified,
        Ok((
            Some(ReservedEnumArgs::Option(GArg::Scalar(ScalarType::Int))),
            Some(ReservedEnumArgs::Result(
                GArg::Scalar(ScalarType::Text),
                GArg::Scalar(ScalarType::Bool),
            )),
        ))
    );
    assert_eq!(
        builds, 0,
        "one session classifies both reserved families by reusing the directory the \
             mint path already built for their rows"
    );
}

#[test]
fn metadata_session_replays_its_first_failure_without_reusing_scratch() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let list = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("aligned owners publish List<int>");
    let orphan_name = draft.intern_string("Orphan").expect("a within-domain mint");
    let orphan = draft
        .add_record_type(RecordTypeDef {
            name: orphan_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    registry.collections.borrow_mut()[list.index() as usize] = CollSpec::List {
        elem: GArg::Struct(orphan),
    };
    let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan));

    let (observed, builds) = count_metadata_directory_builds(|| {
        registry.with_metadata_session(|metadata| {
            let first = metadata.validate_type_arguments(&[GArg::Collection(list)]);
            let collection_replay = metadata.collection_spec(list);
            let unrelated_replay =
                metadata.validate_type_arguments(&[GArg::Param(TypeParamIndex::from_position(7))]);
            Ok::<_, GenericInvariant>((first, collection_replay, unrelated_replay))
        })
    });

    assert_eq!(observed, Ok((Err(expected), Err(expected), Err(expected))));
    assert_eq!(builds, 1, "a poisoned session never rebuilds or resumes");
}

/// Provisional and rejected reserved rows expose neither arguments nor body
/// shape through any semantic reader, for both Option and Result.
#[test]
fn filling_and_rejected_reserved_option_and_result_rows_are_hidden() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let option = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
        .expect("ready Option mints");
    let result_template = registry.reserved_template(Reserved::Result);
    let result = registry
        .mint_type_instance(
            &mut draft,
            result_template,
            &[
                GArg::Scalar(ScalarType::Text),
                GArg::Scalar(ScalarType::Bool),
            ],
            site(3),
        )
        .expect("ready Result mints");
    let TypeInstId::Enum(result) = result else {
        panic!("the reserved Result template is enum-shaped")
    };

    {
        let mut generics = registry.generics.borrow_mut();
        for id in [option, result] {
            let inst = generics
                .type_insts
                .iter_mut()
                .find(|inst| inst.id == TypeInstId::Enum(id))
                .expect("minted reserved row exists");
            let prior = std::mem::replace(
                &mut inst.state,
                TypeInstState::Rejected(ResolveRefusal::Unsupported),
            );
            let TypeInstState::Ready(body) = prior else {
                panic!("minted reserved row is ready")
            };
            inst.state = TypeInstState::Filling { staged: Some(body) };
        }
    }
    assert_reserved_rows_hidden(&registry, option, result);

    {
        let mut generics = registry.generics.borrow_mut();
        for id in [option, result] {
            let inst = generics
                .type_insts
                .iter_mut()
                .find(|inst| inst.id == TypeInstId::Enum(id))
                .expect("minted reserved row exists");
            inst.state = TypeInstState::Rejected(ResolveRefusal::Unsupported);
        }
    }
    assert_reserved_rows_hidden(&registry, option, result);
}

fn assert_reserved_rows_hidden(registry: &TypeRegistry, option: EnumId, result: EnumId) {
    assert!(registry.as_option(option).unwrap().is_none());
    assert!(registry.as_result(result).unwrap().is_none());
    for id in [option, result] {
        let id = TypeInstId::Enum(id);
        assert!(registry.instantiation_of(id).unwrap().is_none());
        assert!(registry.type_inst_body(id).unwrap().is_none());
    }
    assert!(registry.enum_variants(option).unwrap().is_none());
    assert!(registry.enum_variants(result).unwrap().is_none());
    assert!(registry.enum_anchor_spelling(option).unwrap().is_none());
    assert!(registry.enum_anchor_spelling(result).unwrap().is_none());
}

/// Absence of the reserved template is a typed owner failure, not
/// an expectation unwind.
#[test]
fn missing_reserved_template_fails_without_unwinding() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let registry_before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("empty draft encodes");
    let invariant = take_generic_invariant(registry.instantiate_reserved_option(
        &mut draft,
        GArg::Scalar(ScalarType::Int),
        site(2),
    ));
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        invariant,
        GenericInvariant::ReservedTemplateMissing(Reserved::Option)
    );
    assert_eq!(stable_snapshot(&registry), registry_before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// A corrupted reserved-template kind fails before it can
/// unwind or expose the record id as an Option enum id.
#[test]
fn reserved_option_wrong_kind_fails_without_unwinding() {
    let mut registry = registry(reserved_templates());
    registry.type_templates[0].body =
        TemplateBody::Struct(vec![("value".to_string(), name("T"))].into());
    let mut draft = fresh_draft();
    let registry_before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("empty draft encodes");
    let invariant = take_generic_invariant(registry.instantiate_reserved_option(
        &mut draft,
        GArg::Scalar(ScalarType::Int),
        site(2),
    ));
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        invariant,
        GenericInvariant::TemplateKindMismatch {
            template: 0,
            expected: TypeInstKind::Enum,
            actual: TypeInstKind::Struct,
        }
    );
    assert_eq!(stable_snapshot(&registry), registry_before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn resolve_garg_reports_exact_missing_option_and_result_templates() {
    for (reserved, head) in [(Reserved::Option, "Option"), (Reserved::Result, "Result")] {
        let templates = reserved_templates()
            .into_iter()
            .filter(|template| template.reserved != Some(reserved))
            .collect();
        let mut registry = registry(templates);
        let args = match reserved {
            Reserved::Option => vec![name("int")],
            Reserved::Result => vec![name("int"), name("string")],
        };
        let annotation = apply(head, args);
        let mut draft = fresh_draft();
        let before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("empty draft encodes");
        assert_eq!(
            registry.resolve_garg(&mut draft, &annotation, site(2)),
            Err(ResolveError::Invariant(
                GenericInvariant::ReservedTemplateMissing(reserved)
            ))
        );
        assert_eq!(stable_snapshot(&registry), before);
        let draft_after = draft.encode().expect("failed draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }
}

#[test]
fn resolve_garg_reports_exact_wrong_option_and_result_template_kinds() {
    for (reserved, head) in [(Reserved::Option, "Option"), (Reserved::Result, "Result")] {
        let mut templates = reserved_templates();
        let template = templates
            .iter()
            .position(|candidate| candidate.reserved == Some(reserved))
            .expect("reserved template exists");
        templates[template].body = TemplateBody::Struct(Vec::new().into());
        let mut registry = registry(templates);
        let args = match reserved {
            Reserved::Option => vec![name("int")],
            Reserved::Result => vec![name("int"), name("string")],
        };
        let annotation = apply(head, args);
        let expected = ResolveError::Invariant(GenericInvariant::TemplateKindMismatch {
            template,
            expected: TypeInstKind::Enum,
            actual: TypeInstKind::Struct,
        });

        let mut draft = fresh_draft();
        let before = stable_snapshot(&registry);
        let draft_before = draft.encode().expect("empty draft encodes");
        assert_eq!(
            registry.resolve_garg(&mut draft, &annotation, site(2)),
            Err(expected)
        );
        assert_eq!(stable_snapshot(&registry), before);
        let draft_after = draft.encode().expect("failed draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }
}

#[test]
fn map_key_resolution_validates_metadata_before_semantic_refusal() {
    let annotation = apply("Map", vec![name("K"), name("int")]);
    for family in ["struct", "enum", "collection"] {
        let mut registry = registry(Vec::new());
        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let arg = match family {
            "struct" => {
                let name = draft
                    .intern_string("OrphanStruct")
                    .expect("a within-domain mint");
                GArg::Struct(
                    draft
                        .add_record_type(RecordTypeDef {
                            name,
                            fields: Vec::new(),
                        })
                        .expect("a within-domain mint"),
                )
            }
            "enum" => {
                let name = draft
                    .intern_string("OrphanEnum")
                    .expect("a within-domain mint");
                GArg::Enum(
                    draft
                        .add_enum_type(EnumTypeDef {
                            name,
                            variants: Vec::new(),
                        })
                        .expect("a within-domain mint"),
                )
            }
            "collection" => GArg::Collection(coll(0)),
            _ => unreachable!("the hostile family table is closed"),
        };
        let expected = GenericInvariant::TypeArgumentTargetMissing(arg);
        let subst = vec![("K".to_string(), arg)];
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        let (resolved, builds) = count_metadata_directory_builds(|| {
            registry.resolve_garg_env(&mut draft, &annotation, &subst, site(2))
        });
        assert!(matches!(
            resolved,
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_eq!(builds, 1, "{family} key uses one metadata proof");
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);

        let (direct, builds) = count_metadata_directory_builds(|| {
            registry.instantiate_map(&mut draft, arg, GArg::Scalar(ScalarType::Int))
        });
        assert!(matches!(
            direct,
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_eq!(
            builds, 1,
            "the direct {family} key path cannot bypass the owner"
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }

    let mut declared_registry = registry(Vec::new());
    let mut declared_draft = fresh_draft();
    let declared = add_declared_struct(
        &mut declared_registry,
        &mut declared_draft,
        "Plain",
        Vec::new(),
    );
    let subst = vec![("K".to_string(), GArg::Struct(declared))];
    let owner_before = stable_snapshot(&declared_registry);
    let draft_before = draft_snapshot(&declared_draft);
    let (refused, builds) = count_metadata_directory_builds(|| {
        declared_registry.resolve_garg_env(&mut declared_draft, &annotation, &subst, site(3))
    });
    assert_eq!(
        refused,
        Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
    );
    assert_eq!(builds, 1, "a coherent non-key is refused after one proof");
    assert_eq!(stable_snapshot(&declared_registry), owner_before);
    assert_eq!(draft_snapshot(&declared_draft), draft_before);

    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let valid = apply("Map", vec![name("int"), name("string")]);
    let (resolved, builds) =
        count_metadata_directory_builds(|| registry.resolve_garg(&mut draft, &valid, site(4)));
    assert_eq!(resolved, Ok(GArg::Collection(coll(0))));
    assert_eq!(builds, 1, "an admitted Map retains one metadata proof");
}

#[test]
fn missing_nominal_map_key_stops_before_resolving_a_fresh_value() {
    let make_registry = registry;
    let annotation = apply("Map", vec![name("K"), apply("List", vec![name("int")])]);
    let missing = GArg::Nominal(NominalId(0));
    let subst = vec![("K".to_string(), missing)];
    let mut registry = make_registry(Vec::new());
    let mut draft = fresh_draft();
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    let (resolved, builds) = count_metadata_directory_builds(|| {
        registry.resolve_garg_env(&mut draft, &annotation, &subst, site(5))
    });
    assert_eq!(
        resolved,
        Err(ResolveError::Invariant(
            GenericInvariant::TypeArgumentTargetMissing(missing)
        ))
    );
    assert_eq!(
        builds, 0,
        "a missing nominal is rejected by its direct owner"
    );
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);

    let (direct, builds) = count_metadata_directory_builds(|| {
        registry.instantiate_map(&mut draft, missing, GArg::Collection(coll(0)))
    });
    assert_eq!(
        direct,
        Err(ResolveError::Invariant(
            GenericInvariant::TypeArgumentTargetMissing(missing)
        ))
    );
    assert_eq!(builds, 0);
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);

    let mut declared = make_registry(Vec::new());
    declared.nominals.push(NominalInfo {
        name: "Key".to_string(),
        lo: 0,
        hi: 10,
        supports: SupportSet::default(),
    });
    let mut declared_draft = fresh_draft();
    assert_eq!(
        declared.resolve_garg_env(
            &mut declared_draft,
            &annotation,
            &[("K".to_string(), GArg::Nominal(NominalId(0)))],
            site(6),
        ),
        Ok(GArg::Collection(coll(1))),
        "a declared nominal remains an admissible Map key"
    );
}

/// A struct template paired with an enum image id fails
/// before interning field names or mutating either draft table.
#[test]
fn struct_body_with_enum_id_fails_before_draft_mutation() {
    let mut registry = registry(vec![template("Good", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let wrong_name = draft.intern_string("Wrong").expect("a within-domain mint");
    let wrong_id = draft
        .add_enum_type(EnumTypeDef {
            name: wrong_name,
            variants: Vec::new(),
        })
        .expect("a within-domain mint");
    let registry_before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("seeded draft encodes");
    let result = registry.fill_type_body(
        &mut draft,
        0,
        TypeInstId::Enum(wrong_id),
        &[GArg::Scalar(ScalarType::Int)],
        site(2),
    );
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        take_generic_invariant(result),
        GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(wrong_id),
            body: TypeInstKind::Struct,
        }
    );
    assert_eq!(stable_snapshot(&registry), registry_before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// An enum template paired with a record image id fails
/// before interning member names or mutating either draft table.
#[test]
fn enum_body_with_record_id_fails_before_draft_mutation() {
    let mut registry = registry(vec![enum_template("Good", name("T"))]);
    let mut draft = fresh_draft();
    let wrong_name = draft.intern_string("Wrong").expect("a within-domain mint");
    let wrong_id = draft
        .add_record_type(RecordTypeDef {
            name: wrong_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    let registry_before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("seeded draft encodes");
    let result = registry.fill_type_body(
        &mut draft,
        0,
        TypeInstId::Record(wrong_id),
        &[GArg::Scalar(ScalarType::Int)],
        site(2),
    );
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        take_generic_invariant(result),
        GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Record(wrong_id),
            body: TypeInstKind::Enum,
        }
    );
    assert_eq!(stable_snapshot(&registry), registry_before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// A List cache/draft index mismatch fails at the resolving
/// owner, before a usable collection index can escape.
#[test]
fn list_draft_ahead_misalignment_fails_without_exposing_an_index() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .expect("a within-domain mint");
    let before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("seeded draft encodes");
    let result = registry.instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int));
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        take_generic_invariant(result),
        GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::List,
            cache_index: 0,
            draft_index: 1,
        }
    );
    assert_eq!(stable_snapshot(&registry), before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// The Map owner has the same no-index-on-misalignment boundary.
#[test]
fn map_cache_ahead_misalignment_fails_without_exposing_an_index() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    registry.collections.borrow_mut().push(CollSpec::Map {
        key: GArg::Scalar(ScalarType::Int),
        value: GArg::Scalar(ScalarType::Text),
    });
    let before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("seeded draft encodes");
    let result = registry.instantiate_map(
        &mut draft,
        GArg::Scalar(ScalarType::Int),
        GArg::Scalar(ScalarType::Text),
    );
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        take_generic_invariant(result),
        GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::Map,
            cache_index: 1,
            draft_index: 0,
        }
    );
    assert_eq!(stable_snapshot(&registry), before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

/// Owner alignment is checked even on a cache hit, before the prior index can
/// escape or reach collection_spec.
#[test]
fn collection_drift_blocks_a_cache_hit_without_exposing_the_prior_index() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let list = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("aligned owners mint the first List");
    assert_eq!(list, coll(0));
    draft
        .add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Text),
            value: ImageType::scalar(Scalar::Bool),
        })
        .expect("a within-domain mint");
    let before = stable_snapshot(&registry);
    let draft_before = draft.encode().expect("drifted draft encodes");

    let result = registry.instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int));
    let draft_after = draft.encode().expect("failed draft still encodes");

    assert_eq!(
        take_generic_invariant(result),
        GenericInvariant::CollectionIndexMismatch {
            kind: CollectionKind::List,
            cache_index: 1,
            draft_index: 2,
        }
    );
    assert_eq!(stable_snapshot(&registry), before);
    assert_eq!(draft_after.bytes, draft_before.bytes);
    assert_eq!(draft_after.image_id, draft_before.image_id);
}

#[test]
fn published_collection_metadata_is_revalidated_without_a_watermark() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let list = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("aligned owners publish List<int>");
    let orphan_name = draft.intern_string("Orphan").expect("a within-domain mint");
    let orphan = draft
        .add_record_type(RecordTypeDef {
            name: orphan_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    registry.collections.borrow_mut()[list.index() as usize] = CollSpec::List {
        elem: GArg::Struct(orphan),
    };
    let expected = GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(orphan));
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert_eq!(
        registry.validate_type_arguments(&[GArg::Collection(list)]),
        Err(expected)
    );
    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Collection(list)], site(2)),
        Err(ResolveError::Invariant(expected))
    );
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn aligned_collection_wrappers_publish_consecutive_indices() {
    let mut registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let list = registry
        .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
        .expect("aligned List owners mint");
    let map = registry
        .instantiate_map(
            &mut draft,
            GArg::Scalar(ScalarType::Text),
            GArg::Scalar(ScalarType::Bool),
        )
        .expect("aligned Map owners mint");

    assert_eq!((list, map), (coll(0), coll(1)));
    assert_eq!(
        stable_snapshot(&registry).collections,
        vec![
            CollSpec::List {
                elem: GArg::Scalar(ScalarType::Int),
            },
            CollSpec::Map {
                key: GArg::Scalar(ScalarType::Text),
                value: GArg::Scalar(ScalarType::Bool),
            },
        ]
    );
}

fn take_generic_invariant<T>(result: Result<T, ResolveError>) -> GenericInvariant {
    match result {
        Err(ResolveError::Invariant(invariant)) => invariant,
        Err(ResolveError::Refusal(_)) => {
            panic!("compiler bookkeeping must not become a semantic refusal")
        }
        Ok(_) => panic!("the incoherent generic state must fail closed"),
    }
}

#[test]
fn record_id_with_enum_body_fails_every_ready_boundary_exactly() {
    let mut registry = registry(vec![template("Box", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let id = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("Box row mints ready");
    let TypeInstId::Record(record_id) = id else {
        panic!("Box is record-shaped")
    };
    registry.generics.borrow_mut().type_insts[0].state =
        TypeInstState::Ready(InstBody::Enum(Vec::new()));
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id,
        body: TypeInstKind::Enum,
    };
    let before = stable_snapshot(&registry);

    assert_eq!(
        registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(3),),
        Err(ResolveError::Invariant(expected)),
        "a cache hit validates Ready before reusing its ID"
    );
    assert_eq!(
        registry.settled_type_result(0, id, AnyReadyInstance),
        Err(ResolveError::Invariant(expected))
    );
    assert_eq!(registry.instantiation_of(id), Err(expected));
    assert!(matches!(registry.type_inst_body(id), Err(found) if found == expected));
    assert_eq!(registry.inst_anchor_spelling(id), Err(expected));
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Struct(record_id)]),
        Err(expected)
    );
    assert_eq!(stable_snapshot(&registry), before);
}

#[test]
fn enum_id_with_struct_body_fails_every_ready_boundary_exactly() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let enum_id = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
        .expect("Option row mints ready");
    let id = TypeInstId::Enum(enum_id);
    registry.generics.borrow_mut().type_insts[0].state =
        TypeInstState::Ready(InstBody::Struct(Vec::new()));
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id,
        body: TypeInstKind::Struct,
    };
    let before = stable_snapshot(&registry);

    assert_eq!(
        registry.instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(3),),
        Err(ResolveError::Invariant(expected))
    );
    assert_eq!(
        registry.settled_type_result(0, id, AnyReadyInstance),
        Err(ResolveError::Invariant(expected))
    );
    assert_eq!(registry.instantiation_of(id), Err(expected));
    assert!(matches!(registry.type_inst_body(id), Err(found) if found == expected));
    assert_eq!(registry.as_option(enum_id), Err(expected));
    assert_eq!(registry.as_result(enum_id), Err(expected));
    assert_eq!(registry.enum_variants(enum_id), Err(expected));
    assert_eq!(registry.enum_anchor_spelling(enum_id), Err(expected));
    assert_eq!(registry.inst_anchor_spelling(id), Err(expected));
    assert_eq!(
        garg_anchor_spelling(&registry, GArg::Enum(enum_id)),
        Err(expected)
    );
    assert!(matches!(ValueGraph::build(&registry), Err(found) if found == expected));
    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Enum(enum_id)]),
        Err(expected)
    );
    assert_eq!(stable_snapshot(&registry), before);
}

#[path = "ready_body_proof_tests.rs"]
mod ready_body_proof_tests;

#[test]
fn commit_ready_state_hostile_branches_are_exact_and_read_only() {
    let stable = active_registry();
    {
        let mut generics = stable.generics.borrow_mut();
        let TypeInstState::Filling { staged } = &mut generics.type_insts[0].state else {
            panic!("active helper produces a Filling row")
        };
        let body = staged.take().expect("active helper stages a body");
        generics.type_insts[0].state = TypeInstState::Ready(body);
    }
    let before = stable_snapshot(&stable);
    let result = {
        let mut generics = stable.generics.borrow_mut();
        stable.commit_ready_state(&mut generics.type_insts[0])
    };
    assert_eq!(
        result,
        Err(ResolveError::Invariant(GenericInvariant::CacheState(
            GenericCacheInvariant::StableRowInActiveBatch,
        )))
    );
    assert_eq!(stable_snapshot(&stable), before);

    let missing = active_registry();
    missing.generics.borrow_mut().type_insts[0].state = TypeInstState::Filling { staged: None };
    let before = stable_snapshot(&missing);
    let result = {
        let mut generics = missing.generics.borrow_mut();
        missing.commit_ready_state(&mut generics.type_insts[0])
    };
    assert_eq!(
        result,
        Err(ResolveError::Invariant(GenericInvariant::CacheState(
            GenericCacheInvariant::IncompleteRowWithoutRefusal,
        )))
    );
    assert_eq!(stable_snapshot(&missing), before);
}

#[test]
fn durable_anchor_reports_every_missing_target_without_fallback_tokens() {
    let registry = registry(Vec::new());
    let mut draft = fresh_draft();
    let record_name = draft
        .intern_string("OrphanRecord")
        .expect("a within-domain mint");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    let enum_name = draft
        .intern_string("OrphanEnum")
        .expect("a within-domain mint");
    let enum_id = draft
        .add_enum_type(EnumTypeDef {
            name: enum_name,
            variants: Vec::new(),
        })
        .expect("a within-domain mint");
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    for (arg, expected) in [
        (
            GArg::Nominal(NominalId(0)),
            GenericInvariant::TypeArgumentTargetMissing(GArg::Nominal(NominalId(0))),
        ),
        (
            GArg::Struct(record),
            GenericInvariant::TypeArgumentTargetMissing(GArg::Struct(record)),
        ),
        (
            GArg::Group(record),
            GenericInvariant::TypeArgumentTargetMissing(GArg::Group(record)),
        ),
        (
            GArg::Enum(enum_id),
            GenericInvariant::TypeArgumentTargetMissing(GArg::Enum(enum_id)),
        ),
        (
            GArg::Collection(coll(0)),
            GenericInvariant::TypeArgumentTargetMissing(GArg::Collection(coll(0))),
        ),
        (
            GArg::Param(TypeParamIndex::from_position(7)),
            GenericInvariant::TypeArgumentParameter(TypeParamIndex::from_position(7)),
        ),
    ] {
        assert_eq!(garg_anchor_spelling(&registry, arg), Err(expected));
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }
}

#[test]
fn durable_metadata_expands_a_shared_value_at_its_shortest_depth() {
    let mut registry = registry(vec![enum_template("Bad", name("T"))]);
    let mut draft = fresh_draft();
    let bad = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("Bad<int> mints ready");
    let TypeInstId::Enum(bad) = bad else {
        panic!("Bad is enum-shaped")
    };
    registry.generics.borrow_mut().type_insts[0].state =
        TypeInstState::Ready(InstBody::Struct(Vec::new()));

    let shared = add_declared_struct(
        &mut registry,
        &mut draft,
        "Shared",
        vec![("bad", GArg::Enum(bad))],
    );
    let mut deep = shared;
    for index in 0..31 {
        deep = add_declared_struct(
            &mut registry,
            &mut draft,
            &format!("Wrap{index}"),
            vec![("value", GArg::Struct(deep))],
        );
    }
    let root = add_declared_struct(
        &mut registry,
        &mut draft,
        "Root",
        vec![
            ("deep", GArg::Struct(deep)),
            ("direct", GArg::Struct(shared)),
        ],
    );
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id: TypeInstId::Enum(bad),
        body: TypeInstKind::Struct,
    };
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert_eq!(
        registry.validate_durable_value_metadata([GArg::Struct(root)]),
        Err(expected)
    );
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn durable_prevalidation_reaches_nested_and_phantom_generic_arguments() {
    for fields in [vec![("value", name("T"))], Vec::new()] {
        let mut templates = reserved_templates();
        templates.push(template("Outer", fields));
        let mut registry = registry(templates);
        let mut draft = fresh_draft();
        let inner = registry
            .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
            .expect("inner Option mints ready");
        let outer = registry
            .mint_type_instance(&mut draft, 2, &[GArg::Enum(inner)], site(3))
            .expect("outer generic mints ready");
        let TypeInstId::Record(outer) = outer else {
            panic!("Outer is record-shaped")
        };
        registry.generics.borrow_mut().type_insts[0].state =
            TypeInstState::Ready(InstBody::Struct(Vec::new()));
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(inner),
            body: TypeInstKind::Struct,
        };

        assert_eq!(
            registry.validate_durable_value_metadata([GArg::Struct(outer)]),
            Err(expected),
            "phantom arguments are prevalidated as well as body-reachable ones"
        );
        assert_eq!(
            garg_anchor_spelling(&registry, GArg::Struct(outer)),
            Err(expected),
            "durable anchor recursion cannot launder the nested Ready invariant"
        );
    }
}

#[test]
fn durable_build_rejects_nested_and_phantom_ready_corruption_before_effects() {
    for source in [
        r#"struct Outer<T> {
    value: T
}

resource Holder {
    required value: Outer<Option<int>>
}

store ^holders[id: int]: Holder
"#,
        r#"struct Outer<T> {}

resource Holder {
    required value: Outer<Option<int>>
}

store ^holders[id: int]: Holder
"#,
    ] {
        let parsed = parse_source(source);
        assert!(!parsed.has_errors());
        let generic_struct = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Struct(declaration) => Some(declaration),
                _ => None,
            })
            .expect("generic struct parses");
        let resource = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(declaration) => Some(declaration),
                _ => None,
            })
            .expect("resource parses");
        let store = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Store(declaration) => Some(declaration),
                _ => None,
            })
            .expect("store parses");
        let structs = vec![(
            FileRef::admitted(0),
            crate::test_file_identity("src/main.mw"),
            generic_struct,
        )];
        let resources = vec![(
            FileRef::admitted(0),
            crate::test_file_identity("src/main.mw"),
            resource,
        )];
        let stores = vec![(
            crate::analysis::FileRef::admitted(0),
            crate::test_file_identity("src/main.mw"),
            store,
        )];
        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let mut diagnostics = DiagnosticCollector::new();
        let registry = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &structs,
            &[],
            &resources,
            &mut diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(diagnostics.is_empty());
        let (option_index, option_id) = {
            let generics = registry.generics.borrow();
            generics
                .type_insts
                .iter()
                .enumerate()
                .find_map(|(index, inst)| {
                    (registry.type_templates[inst.template].reserved == Some(Reserved::Option))
                        .then_some((index, inst.id))
                })
                .expect("resource field mints Option")
        };
        let TypeInstId::Enum(option_id) = option_id else {
            panic!("Option is enum-shaped")
        };
        registry.generics.borrow_mut().type_insts[option_index].state =
            TypeInstState::Ready(InstBody::Struct(Vec::new()));
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option_id),
            body: TypeInstKind::Struct,
        };
        let before = draft.encode().expect("seeded draft encodes");
        draft.commit();

        assert!(matches!(
            crate::durable::DurableRegistry::build(
                &mut draft_owner,
                &registry,
                &resources,
                &stores,
                None,
                &mut diagnostics, DeclarationBudget::default()),
            Err(crate::types::BuildError::Invariant(found)) if found == expected
        ));
        assert!(diagnostics.is_empty());
        let after = draft_owner.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }
}

#[test]
fn value_cycle_invariant_precedes_and_preserves_source_diagnostics() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let enum_id = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
        .expect("Option row mints ready");
    registry.generics.borrow_mut().type_insts[0].state =
        TypeInstState::Ready(InstBody::Struct(Vec::new()));
    let expected = GenericInvariant::TypeBodyKindMismatch {
        id: TypeInstId::Enum(enum_id),
        body: TypeInstKind::Struct,
    };
    let mut diagnostics = DiagnosticCollector::new();
    diagnostics.push(SourceDiagnostic::at(
        Code::CheckType.as_str(),
        crate::test_main_file_identity(),
        SourceSpan::default(),
        "earlier source failure".to_string(),
    ));
    let before = diagnostics.probe();

    assert_eq!(
        reject_value_cycles(&registry, &mut diagnostics),
        Err(expected)
    );
    assert_eq!(diagnostics.probe(), before);
}

#[path = "owner_txn_tests.rs"]
mod owner_txn_tests;

/// Whole-build custody: one store's ordinary refusal settles, a later store returns a
/// semantic invariant, and none of the first store's payload reaches the caller.
///
/// The build publishes per store, so an earlier store's rows were already in the
/// caller's collector by the time a later store aborted — leaving a compilation with no
/// registry but with one store's diagnostics published. The abort is a whole-build
/// event, so the whole build's custody is what it drops.
///
/// Both halves are asserted against artifacts: the first store's row is shown to exist
/// on the path that succeeds, so the empty-collector assertion cannot pass by the row
/// never having been produced.
#[test]
fn a_store_invariant_publishes_no_earlier_store_payload_and_restores_the_draft() {
    const SOURCE: &str = r#"struct Bad {
    c: List<int>
}

struct Outer<T> {
    value: T
}

resource Alpha {
    required f: Bad
}

resource Beta {
    required value: Outer<Option<int>>
}

store ^alpha[id: int]: Alpha
store ^beta[id: int]: Beta
"#;

    /// Build the two-store corpus, optionally planting the body-kind corruption that
    /// makes the *second* store's Product return an invariant.
    fn run(corrupt_second_store: bool) -> (Result<(), GenericInvariant>, Vec<String>, Vec<u8>) {
        let parsed = parse_source(SOURCE);
        assert!(!parsed.has_errors());
        let file = crate::test_file_identity("src/main.mw");
        let at = FileRef::admitted(0);
        let mut structs = Vec::new();
        let mut resources = Vec::new();
        let mut stores = Vec::new();
        for declaration in &parsed.file.declarations {
            match declaration {
                Declaration::Struct(d) => structs.push((at, file.clone(), d)),
                Declaration::Resource(d) => resources.push((at, file.clone(), d)),
                Declaration::Store(d) => stores.push((at, file.clone(), d)),
                other => {
                    panic!("the corpus declares only structs, resources, and stores: {other:?}")
                }
            }
        }
        assert_eq!(structs.len(), 2, "Bad and Outer");
        assert_eq!(stores.len(), 2, "^alpha then ^beta, in that order");

        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let mut diagnostics = DiagnosticCollector::new();
        let registry = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &structs,
            &[],
            &resources,
            &mut diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(diagnostics.is_empty());

        if corrupt_second_store {
            let option_index = {
                let generics = registry.generics.borrow();
                generics
                    .type_insts
                    .iter()
                    .position(|inst| {
                        registry.type_templates[inst.template].reserved == Some(Reserved::Option)
                    })
                    .expect("Beta's field mints Option")
            };
            registry.generics.borrow_mut().type_insts[option_index].state =
                TypeInstState::Ready(InstBody::Struct(Vec::new()));
        }

        let before = draft.encode().expect("seeded draft encodes").bytes;
        draft.commit();

        let outcome = crate::durable::DurableRegistry::build(
            &mut draft_owner,
            &registry,
            &resources,
            &stores,
            None,
            &mut diagnostics,
            DeclarationBudget::default(),
        );
        let outcome = match outcome {
            Ok(_) => Ok(()),
            Err(crate::types::BuildError::Invariant(found)) => Err(found),
            Err(other) => panic!("unexpected build error: {other:?}"),
        };
        let rows = diagnostics
            .finish()
            .expect_complete()
            .iter()
            .map(|row| row.message().to_string())
            .collect();
        let after = draft_owner.encode().expect("draft still encodes").bytes;
        assert_eq!(after, before, "no store's rows survive either build");
        (outcome, rows, before)
    }

    // Body A alone: the first store's ordinary refusal really does produce a row, and
    // the build that completes publishes it. This is the artifact the abort must drop.
    let (clean, clean_rows, _) = run(false);
    assert!(clean.is_ok(), "the uncorrupted two-store build completes");
    let alpha_row = clean_rows
        .iter()
        .find(|row| row.contains("a collection stored directly"))
        .expect("^alpha's ordinary refusal publishes its row when the build completes")
        .clone();

    // Body A then body B: the same first-store refusal happens, then the second store
    // returns its invariant. The first store's row is gone.
    let (aborted, aborted_rows, _) = run(true);
    assert!(
        matches!(aborted, Err(GenericInvariant::TypeBodyKindMismatch { .. })),
        "the second store returns the planted semantic invariant, got {aborted:?}",
    );
    assert!(
        !aborted_rows.contains(&alpha_row),
        "the aborted build published the earlier store's row: {aborted_rows:?}",
    );
    assert!(
        aborted_rows.is_empty(),
        "a whole-build abort publishes no store's payload, got {aborted_rows:?}",
    );
}
