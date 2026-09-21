//! Fixtures shared by the generic-registry custody battery: template spellings, a
//! mint site, a leaked armed draft, and a structural snapshot of every registry owner
//! the generic-owner transaction must restore.
//!
//! One owner, so a registry field added to [`TypeRegistry`] is added here once.

use super::*;

use marrow_image::ImageDraft;

/// The wide collection id at `index`, spelled compactly for the corpus.
pub(super) fn coll(index: u16) -> CollTypeId {
    CollTypeId::from_index(index)
}

/// A bare `Name` type annotation with no spans.
pub(super) fn name(text: &str) -> TypeExpr {
    TypeExpr::Name {
        text: text.to_string(),
        segment_spans: Vec::new(),
        span: SourceSpan::default(),
    }
}

/// An `Apply` type annotation with no spans.
pub(super) fn apply(head: &str, args: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Apply {
        head: head.to_string(),
        head_span: SourceSpan::default(),
        args,
        span: SourceSpan::default(),
    }
}

/// A one-parameter generic struct template.
pub(super) fn template(name: &str, fields: Vec<(&str, TypeExpr)>) -> TypeTemplate {
    TypeTemplate {
        name: name.to_string(),
        file: Some(crate::test_file("src/main.mw").clone()),
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

/// A one-parameter generic enum template of one `value(item: payload)` variant.
pub(super) fn enum_template(name: &str, payload: TypeExpr) -> TypeTemplate {
    TypeTemplate {
        name: name.to_string(),
        file: Some(crate::test_file("src/main.mw").clone()),
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

/// An empty registry carrying `templates` and nothing else.
pub(super) fn test_registry(templates: Vec<TypeTemplate>) -> TypeRegistry {
    TypeRegistry {
        origins: crate::source::CapturedOrigins::of(crate::test_input()),
        named: DeclarationLedger::new(
            DeclarationNamespace::NamedType,
            DeclarationBudget::default(),
        ),
        members: DeclarationLedger::new(
            DeclarationNamespace::ResourceMember,
            DeclarationBudget::default(),
        ),
        aliases: AliasTable::default(),
        nominals: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        records: AdmittedRecords::default(),
        type_templates: templates,
        generics: RefCell::default(),
        collections: RefCell::default(),
        collection_index: RefCell::default(),
        row_directory: RefCell::default(),
        coordinates: DeclarationCoordinates::default(),
    }
}

/// A mint site in the one test file, at `line`.
pub(super) fn site(line: u32) -> MintSite<'static> {
    MintSite {
        file: crate::test_file("src/main.mw"),
        span: SourceSpan {
            line,
            column: 9,
            ..SourceSpan::default()
        },
    }
}

/// The encoded bytes and id of a draft, the shape a "nothing was written" assertion
/// compares.
pub(super) fn draft_fingerprint(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
    let encoded = draft.encode().expect("test draft encodes");
    (encoded.bytes, encoded.image_id)
}

/// Merge a finished generic transfer into a fresh collector and read the complete
/// ordered rows, panicking on a limited terminal (these fixtures stay far below the
/// ceilings).
pub(super) fn ordered(outcome: GenericDiagnostics) -> Vec<SourceDiagnostic> {
    let mut collector = DiagnosticCollector::new();
    outcome.merge_into(&mut collector);
    collector
        .finish()
        .into_complete()
        .expect("a complete terminal")
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StableLimit {
    Open,
    PendingRow(SourceDiagnostic),
    Reported,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StableRowState {
    Filling,
    Staged,
    Ready,
    RejectedLimit,
    RejectedUnsupported,
    RejectedDeclaration,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct StableRow {
    pub(super) template: usize,
    pub(super) args: Vec<GArg>,
    pub(super) id: TypeInstId,
    pub(super) state: StableRowState,
    pub(super) body: Option<StableBody>,
    pub(super) dependents: Vec<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StableBody {
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
pub(super) struct StableSnapshot {
    pub(super) rows: Vec<StableRow>,
    pub(super) collections: Vec<CollSpec>,
    pub(super) functions: Vec<(usize, Vec<GArg>, u16)>,
    pub(super) queue: Vec<(usize, Vec<GArg>, u16)>,
    fill_batch_start: Option<usize>,
    fill_rows: Vec<(TypeInstKey, usize)>,
    filling: Option<PendingFill>,
    pending_fills: Vec<PendingFill>,
    fill_failures: Vec<(usize, ResolveRefusal)>,
    limit: StableLimit,
    payloads: crate::diag::CollectorProbe,
    build_invariant: Option<GenericInvariant>,
    // The lockstep secondary indexes and the swapped argument domain: an isolation probe
    // must observe a missed index purge or a stuck `TemplateProof` domain, not only the
    // primary append-only owners. `HashMap` equality is content-based, so these compare
    // regardless of iteration order.
    type_index: HashMap<usize, HashMap<Vec<GArg>, usize>>,
    pub(super) fn_index: HashMap<usize, HashMap<Vec<GArg>, usize>>,
    collection_index: HashMap<CollSpec, CollTypeId>,
    argument_domain: ArgumentDomain,
}

/// Every registry owner the generic-owner transaction's inverse must restore, in a
/// shape that compares by value.
pub(super) fn stable_snapshot(registry: &TypeRegistry) -> StableSnapshot {
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
        .map(|inst| (inst.template, inst.args.clone(), inst.func.index()))
        .collect();
    let queue = generics
        .fn_queue
        .iter()
        .map(|inst| (inst.template, inst.args.clone(), inst.func.index()))
        .collect();
    let limit = match &generics.limit {
        LimitState::Open => StableLimit::Open,
        LimitState::Pending(diagnostic) => StableLimit::PendingRow(diagnostic.clone()),
        LimitState::Reported => StableLimit::Reported,
    };
    StableSnapshot {
        rows,
        collections: registry.collections.borrow().clone(),
        functions,
        queue,
        fill_batch_start: generics.fill_batch_start,
        fill_rows: generics
            .fill_rows
            .iter()
            .map(|(key, index)| (*key, *index))
            .collect(),
        filling: generics.filling,
        pending_fills: generics.pending_fills.iter().copied().collect(),
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
