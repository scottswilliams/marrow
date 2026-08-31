//! The durable registry's build-seam tests: the generic enum shape resolution
//! seams and the flat declaration-command emission bound.

/// A fresh armed transaction over its own leaked owner, for fixtures that never
/// touch the owner again.
#[cfg(test)]
fn fresh_draft() -> marrow_image::DraftTxn<'static> {
    let owner: &'static mut marrow_image::ImageDraft =
        Box::leak(Box::new(marrow_image::ImageDraft::new()));
    super::admitted(owner)
}

#[cfg(test)]
mod generic_enum_shape_tests {
    use super::super::*;
    use super::fresh_draft;
    use crate::types::{MintSite, TypeInstId, TypeInstKind};
    use marrow_syntax::{Declaration, parse_source};

    /// The store declaration these resolvers refuse against. The resolver retains its
    /// refusal under the declared placement name, so it needs the declaration's
    /// coordinates even when the test drives only one value-shape walk.
    fn test_declared() -> DeclarationSite<'static> {
        DeclarationSite {
            name: "probe",
            file: crate::test_main_file_identity(),
            at: FileRef::admitted(0),
            span: SourceSpan::default(),
        }
    }

    /// A committed reserved enum reaches the durable-shape owner
    /// with its exact member and payload layout. Missing ledger rows may make the
    /// enclosing graph incomplete, but do not turn a Ready enum into an unavailable
    /// generic row.
    #[test]
    fn ready_option_reaches_the_durable_enum_shape_owner() {
        let mut draft = fresh_draft();
        let mut build_diagnostics = DiagnosticCollector::new();
        let mut records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut build_diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(build_diagnostics.is_empty());
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: SourceSpan {
                        line: 1,
                        column: 1,
                        ..SourceSpan::default()
                    },
                },
            )
            .expect("Ready Option mints");

        let mut diagnostics = DiagnosticCollector::new();
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );
        let mut values = fresh_draft();
        let shape = records
            .with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>(resolver.build_enum_value_shape(
                    &mut values,
                    &records,
                    metadata,
                    option,
                ))
            })
            .expect("the Ready Option metadata session opens")
            .expect("the Ready Option value shape is built");
        let Some(ValueShapeView::Enum { members, .. }) = values.value_shapes().view(shape) else {
            panic!("a Ready Option remains enum-shaped")
        };
        assert_eq!(members.len(), 2);
        assert!(members[0].payload().is_empty());
        assert_eq!(members[1].payload().len(), 1);
        assert_eq!(
            values.value_shapes().view(members[1].payload()[0]),
            Some(ValueShapeView::Scalar(ScalarType::Int.image()))
        );
        assert!(
            resolver.refusal.is_some(),
            "the test intentionally supplies no ledger"
        );
        drop(resolver);
        assert_eq!(
            diagnostics.probe_rows().len(),
            3,
            "sum plus two member identity gaps"
        );
        assert!(
            diagnostics
                .probe_rows()
                .iter()
                .all(|diagnostic| diagnostic.code() == Code::CheckDurableIdentity.as_str())
        );
    }

    /// An image enum with no Ready semantic row is refused before
    /// durable identity spelling or member resolution can observe it.
    #[test]
    fn unavailable_enum_stops_before_durable_identity_resolution() {
        let mut draft = fresh_draft();
        let mut build_diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut build_diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(build_diagnostics.is_empty());
        let name = draft
            .intern_string("Unavailable")
            .expect("a within-domain mint");
        let unavailable = draft
            .add_enum_type(marrow_image::EnumTypeDef {
                name,
                variants: Vec::new(),
            })
            .expect("a within-domain mint");
        let mut diagnostics = DiagnosticCollector::new();
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );

        let mut values = fresh_draft();
        let shape = records
            .with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>(resolver.build_enum_value_shape(
                    &mut values,
                    &records,
                    metadata,
                    unavailable,
                ))
            })
            .expect("the unavailable enum metadata session opens")
            .expect("the unavailable enum value shape is built");
        assert_eq!(
            values.value_shapes().view(shape),
            Some(ValueShapeView::Scalar(ScalarType::Int.image()))
        );
        assert!(resolver.refusal.is_some());
        drop(resolver);
        assert_eq!(diagnostics.probe_rows().len(), 1);
        assert_eq!(
            diagnostics.probe_rows()[0].code(),
            Code::CheckUnsupported.as_str()
        );
        assert!(diagnostics.probe_rows()[0].identity_gap().is_none());
    }

    #[test]
    fn ready_enum_with_struct_body_is_not_contextualized_or_resolved() {
        let mut draft = fresh_draft();
        let mut build_diagnostics = DiagnosticCollector::new();
        let mut records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut build_diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: SourceSpan::default(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let mut diagnostics = DiagnosticCollector::new();
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );

        assert!(
            resolver
                .accept_ready_shape::<()>(Err(expected), "this enum value")
                .is_none()
        );
        assert_eq!(resolver.invariant, Some(expected));
        assert!(resolver.value_memo.is_empty());
        drop(resolver);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn durable_typed_error_stops_before_identity_or_draft_effects() {
        let parsed = parse_source(
            r#"resource Holder {
    required value: Option<int>
}

store ^holders[id: int]: Holder
"#,
        );
        assert!(!parsed.has_errors());
        let resource = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(resource) => Some(resource),
                _ => None,
            })
            .expect("resource parses");
        let resources = vec![(
            FileRef::admitted(0),
            crate::test_file_identity("src/main.mw"),
            resource,
        )];
        let mut draft = fresh_draft();
        let mut diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &resources,
            &mut diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget");
        assert!(diagnostics.is_empty());
        let option = match records.by_name("Holder").expect("record exists").fields[0].ty {
            GArg::Enum(id) => id,
            _ => panic!("resource field resolves to Option"),
        };
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let before = draft.encode().expect("seeded draft encodes");
        let mut reported_identity_gaps = BTreeSet::new();
        let mut resolver = IdentityResolver::new(
            test_declared(),
            SourceSpan::default(),
            None,
            &mut reported_identity_gaps,
            &mut diagnostics,
        );
        assert!(
            resolver
                .accept_ready_shape::<()>(Err(expected), "this durable value")
                .is_none()
        );
        assert_eq!(resolver.invariant, Some(expected));
        assert!(resolver.value_memo.is_empty());
        drop(resolver);
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    /// The projection is appended in the same statement as the ledger entry, so a
    /// resource naming a placement the ledger does not know is the two having drifted.
    /// Answering `Absent` there would put a fabricated absence back at the use site — the
    /// defect this projection exists to remove — reached through the projection instead of
    /// through the executable list.
    #[test]
    fn a_projection_naming_an_unknown_placement_is_drift_not_absence() {
        let mut registry = DurableRegistry::empty(DeclarationBudget::default());
        registry.products.insert(
            "Holder".to_string(),
            ProductStores {
                admitted: vec!["holders".to_string()],
                first_refused: None,
                declared_branches: false,
            },
        );
        assert!(matches!(
            registry.product("Holder"),
            Err(DeclarationIndexDrift)
        ));
        // A resource whose every store was refused steers to the first cause; a
        // projection recording neither an admitted nor a refused store is incoherent.
        registry.products.insert(
            "Neither".to_string(),
            ProductStores {
                admitted: Vec::new(),
                first_refused: None,
                declared_branches: false,
            },
        );
        assert!(matches!(
            registry.product("Neither"),
            Err(DeclarationIndexDrift)
        ));
        // A resource no store binds has no projection entry at all, which is the
        // genuine absence and stays one.
        assert!(matches!(
            registry.product("Unbound"),
            Ok(ProductBinding::Absent)
        ));
    }
}

#[cfg(test)]
mod declaration_command_bound_tests {
    use super::super::*;
    use super::fresh_draft;
    use marrow_image::{ExportId, FunctionDef, ImageBuildError, Instr, SpanEntry};

    const APPLICATION_ID: [u8; 16] = [0x0a; 16];
    const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
    const KEY_ID: [u8; 16] = [0x0c; 16];
    const PRODUCT_ID: [u8; 16] = [0x0d; 16];

    /// A distinct member ledger id seeded by `n`, so a width fixture cannot be
    /// answered by an identity-collision refusal instead of the bound under test.
    fn member_id(n: usize) -> LedgerIdBytes {
        let mut bytes = [0x40u8; 16];
        bytes[0] = n as u8;
        bytes[1] = (n >> 8) as u8;
        LedgerIdBytes::from_bytes(bytes)
    }

    /// Encode a minimal image whose one keyed root projects a Product declaring exactly
    /// `commands`, so the declaration width is the only reason an encode can fail.
    fn encode_product(
        commands: impl FnOnce(&mut DraftTxn<'_>) -> Vec<DeclarationMemberDef>,
    ) -> Result<(), ImageBuildError> {
        /// The construction budget these producer-seam fixtures are admitted under.
        ///
        /// The command term is the image's own [`bounds::MAX_ADMITTED_DECLARATION_COMMANDS`],
        /// which sits exactly one past the member bound, so the over-wide declaration below
        /// is admitted into the draft and refused where that refusal lives — at the encoder.
        fn admitted_plan() -> AdmittedGraphInputPlan {
            AdmittedGraphInputPlan::admit(1, 1, bounds::MAX_ADMITTED_DECLARATION_COMMANDS)
                .expect("one Product, one root, and the image's own command ceiling")
        }

        let mut draft = fresh_draft();
        let commands = commands(&mut draft);
        let type_name = draft.intern_string("R").expect("a within-domain mint");
        let record = draft
            .add_record_type(RecordTypeDef {
                name: type_name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint");
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        let root_name = draft.intern_string("r").expect("a within-domain mint");
        draft
            .declare_product(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                record,
                commands,
            )
            .expect("a well-formed flat declaration");
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
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        let src = draft
            .intern_string("src/main.mw")
            .expect("a within-domain mint");
        let main_name = draft.intern_string("main").expect("a within-domain mint");
        let zero = draft.intern_int(0).expect("a within-domain mint");
        let code = vec![Instr::ConstLoad(zero), Instr::Return];
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

    /// The declaration member bound is admitted, emitted, and refused by three owners that
    /// agree by exactly one command.
    /// This module stops emitting at [`bounds::MAX_ADMITTED_DECLARATION_COMMANDS`] — the same count an
    /// [`AdmittedGraphInputPlan`] admits for one declaration; `marrow-image` records
    /// a declaration as over-bound only at *more* than `MAX_DURABLE_MEMBERS` rows and the
    /// encoder then refuses the image with
    /// [`ImageBuildError::TooManyDurableMembers`] — which the compiler classifies as a
    /// producer contradiction. Truncating one command lower would hand the image
    /// owner a full-width declaration it accepts, and the over-wide resource would encode
    /// silently short instead of being refused. This drives the real emitter with an
    /// over-wide node buffer and carries its output to a real encode, so moving either
    /// bound without the other fails here.
    ///
    /// It is a producer-seam fixture rather than a `compile()`-tier one because the width
    /// is not reachable from source today: every member anchors one identity ledger row,
    /// and `marrow-project`'s `MAX_IDS_ROWS` (8192) admits no ledger that also carries the
    /// application, product, placement, and key rows a resource of this width needs.
    #[test]
    fn one_member_past_the_member_bound_encodes_as_too_many_durable_members() {
        assert_eq!(
            bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
            bounds::MAX_DURABLE_MEMBERS + 1,
            "the admitted command count must sit exactly one past the image owner's bound"
        );

        let flat_fields = |draft: &mut DraftTxn<'_>, count: usize| {
            let value = draft
                .value_scalar(Scalar::Int)
                .expect("the test arena mints");
            declaration_commands(
                (0..count)
                    .map(|n| {
                        DeclarationDraftNode::declared(
                            None,
                            DeclarationWireClass::Field,
                            DeclarationMemberShape::Field {
                                id: member_id(n),
                                required: false,
                                value,
                            },
                        )
                    })
                    .collect(),
            )
        };

        assert!(
            matches!(
                encode_product(|draft| {
                    let commands = flat_fields(draft, bounds::MAX_DURABLE_MEMBERS + 64);
                    assert_eq!(
                        commands.len(),
                        bounds::MAX_DURABLE_MEMBERS + 1,
                        "an over-wide resource emits exactly one command past the member bound"
                    );
                    commands
                }),
                Err(ImageBuildError::TooManyDurableMembers)
            ),
            "one command past the bound must reach the encoder as the durable-member limit"
        );

        assert!(
            encode_product(|draft| {
                let at_bound = flat_fields(draft, bounds::MAX_DURABLE_MEMBERS);
                assert_eq!(at_bound.len(), bounds::MAX_DURABLE_MEMBERS);
                at_bound
            })
            .is_ok(),
            "a declaration exactly at the member bound still encodes"
        );
    }
}

/// The post-staging custody seam of [`DurableRegistry::build`], driven from source.
///
/// Each store is built inside an aggregate that owns the armed transaction and its private
/// diagnostics together. Every checked refusal returns `StoreBuild::Refused`, runs the
/// total inverse, and then releases its row; an invariant instead leaves through the `?`
/// on `build_one`, dropping the armed transaction and the staged rows together.
///
/// The trigger is the construction budget, and it is the only one this compiler has:
/// every image bound the durable build can cross — roots, sites, string bytes,
/// declaration members — is *nonblocking* by construction, observed into the policy
/// ledger and refused later by the encoder over a complete graph. What still refuses
/// inside `build_one` is the budget the census froze, and it is reachable from source:
/// the budget saturates root occurrences at [`bounds::MAX_ADMITTED_ROOT_OCCURRENCES`], so
/// a project declaring one more admissible store than that drives the last store's
/// `add_root_occurrence` into a refusal after that store has already staged into the
/// draft.
///
/// Two consequences of that trigger shape the assertions below.
///
/// A graph with that many live occurrences is past [`bounds::MAX_ROOTS`], so the encoder
/// refuses it and encoded bytes are not available as the restoration artifact here.
/// [`marrow_image::DurableContractView::contract_id`] is: the byte-exact 32-byte identity
/// of the canonical durable graph, written from the same rows the encoder would write and
/// defined whatever the policy ledger holds.
///
/// And a last store that occurs a Product the draft already declares stages exactly one
/// row before the refusal — its interned placement spelling — which no public read of a
/// draft the encoder refuses can observe. That arm therefore carries the outcome and the
/// settlement laws; the restoration law is carried by the arms whose last store declares
/// its own Product, one member wide and many, whose staging the draft does publish.
#[cfg(test)]
mod post_staging_custody_tests {
    use super::super::*;
    use marrow_image::DurableContractId;
    use marrow_project::IdentityLedger;
    use marrow_syntax::{Declaration, parse_source};
    use std::fmt::Write as _;

    /// Root occurrences the corpus commits before its last store is built, so that last
    /// store's occurrence is the first one the construction budget refuses.
    const FILLED_OCCURRENCES: usize = bounds::MAX_ADMITTED_ROOT_OCCURRENCES;

    /// A corpus small enough that every arm of it commits, for measuring what the last
    /// store stages where the budget still admits its occurrence.
    const ADMITTED_OCCURRENCES: usize = 2;

    /// The width of the `Many` resource, against the one field `One` declares.
    const MANY_FIELDS: usize = 32;

    /// Which resource the corpus's last store occurs, and therefore how much that store
    /// stages before its root occurrence is refused.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum LastStore {
        /// No last store at all: the control the other arms are compared against.
        Absent,
        /// A further occurrence of the Product every earlier store already declared. It
        /// stages its interned placement spelling and nothing else.
        Held,
        /// The sole occurrence of a Product declaring one member.
        One,
        /// The sole occurrence of a Product declaring [`MANY_FIELDS`] members.
        Many,
    }

    impl LastStore {
        /// The resource the arm's last store occurs, if it declares one.
        fn resource(self) -> Option<&'static str> {
            match self {
                Self::Absent => None,
                Self::Held => Some("Held"),
                Self::One => Some("One"),
                Self::Many => Some("Many"),
            }
        }
    }

    /// The anchor positions [`corpus_anchors`] writes the two fresh Products at, so a test
    /// can ask the draft whether either declaration survived a refused store.
    const ONE_PRODUCT_ANCHOR: usize = 3;
    const MANY_PRODUCT_ANCHOR: usize = 5;

    /// A distinct ledger id per anchor position, so no corpus can be answered by an
    /// identity collision instead of the budget under test.
    fn anchor_id(position: usize) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&(position as u64 + 1).to_be_bytes());
        bytes
    }

    /// The corpus source: one store naming a resource that does not exist (an ordinary
    /// refusal, which settles a row when the build completes), `roots` keyless occurrences
    /// of `Held`, and the last store `last` selects.
    fn corpus_source(roots: usize, last: LastStore) -> String {
        let mut source = String::from(
            "resource Held {\n    required value: int\n}\n\n\
             resource One {\n    required f0: int\n}\n\n\
             resource Many {\n",
        );
        for field in 0..MANY_FIELDS {
            let _ = writeln!(source, "    required f{field}: int");
        }
        source.push_str("}\n\nstore ^nowhere: Missing\n");
        for root in 0..roots {
            let _ = writeln!(source, "store ^r{root}: Held");
        }
        if let Some(resource) = last.resource() {
            let _ = writeln!(source, "store ^last: {resource}");
        }
        source
    }

    /// The corpus anchors, in the order [`anchor_id`] numbers them. `^nowhere` names no
    /// resource so it anchors nothing, and a keyless root anchors no key column.
    fn corpus_anchors(roots: usize) -> Vec<String> {
        let mut anchors = vec![
            "application .".to_string(),
            "product Held".to_string(),
            "field Held.value".to_string(),
        ];
        assert_eq!(anchors.len(), ONE_PRODUCT_ANCHOR);
        anchors.push("product One".to_string());
        anchors.push("field One.f0".to_string());
        assert_eq!(anchors.len(), MANY_PRODUCT_ANCHOR);
        anchors.push("product Many".to_string());
        anchors.extend((0..MANY_FIELDS).map(|field| format!("field Many.f{field}")));
        anchors.extend((0..roots).map(|root| format!("root r{root}")));
        anchors.push("root last".to_string());
        anchors
    }

    /// The committed identity ledger the corpus resolves against.
    fn corpus_ledger(roots: usize) -> IdentityLedger {
        let mut text = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for (position, anchor) in corpus_anchors(roots).iter().enumerate() {
            let _ = write!(text, "id {anchor} ");
            for byte in anchor_id(position) {
                let _ = write!(text, "{byte:02x}");
            }
            text.push('\n');
        }
        text.push_str("high-water 0\nend\n");
        IdentityLedger::parse(text.as_bytes()).expect("the corpus ledger parses")
    }

    /// Everything a store's staging can change that a draft the encoder refuses still
    /// publishes, in one comparable value. `contract` is the canonical durable graph's
    /// byte-exact identity; the declaration flags and arena counts cover what a Product
    /// declaration appends outside the occurrence table, which a graph identity derived
    /// from root occurrences alone would not observe.
    #[derive(PartialEq, Eq, Debug)]
    struct DraftState {
        contract: Option<DurableContractId>,
        semantic_nodes: usize,
        one_declared: bool,
        many_declared: bool,
        value_shapes: usize,
        records: usize,
        enums: usize,
        collections: usize,
    }

    fn draft_state(draft: &ImageDraft) -> DraftState {
        let declared = |anchor: usize| {
            draft
                .product_members(LedgerIdBytes::from_bytes(anchor_id(anchor)))
                .is_some()
        };
        let view = draft.contract_view();
        DraftState {
            contract: view.contract_id().ok(),
            semantic_nodes: view.semantic_nodes().len(),
            one_declared: declared(ONE_PRODUCT_ANCHOR),
            many_declared: declared(MANY_PRODUCT_ANCHOR),
            value_shapes: draft.value_shapes().len(),
            records: draft.record_type_count(),
            enums: draft.enum_type_count(),
            collections: draft.collection_type_count(),
        }
    }

    /// What one production build of the corpus produced.
    struct Built {
        outcome: Result<(), crate::types::BuildError>,
        published: Vec<String>,
        state: DraftState,
        /// The encoded image, where the corpus is inside every image bound.
        image: Result<Vec<u8>, marrow_image::ImageBuildError>,
    }

    /// Drive `DurableRegistry::build` over the corpus through the production entry point,
    /// against the real type registry and the real identity ledger.
    fn build_corpus(roots: usize, last: LastStore) -> Built {
        let source = corpus_source(roots, last);
        let parsed = parse_source(&source);
        assert!(!parsed.has_errors(), "the corpus parses");
        let file = crate::test_file_identity("src/main.mw");
        let at = FileRef::admitted(0);
        let mut resources = Vec::new();
        let mut stores = Vec::new();
        for declaration in &parsed.file.declarations {
            match declaration {
                Declaration::Resource(d) => resources.push((at, file.clone(), d)),
                Declaration::Store(d) => stores.push((at, file.clone(), d)),
                other => panic!("the corpus declares only resources and stores: {other:?}"),
            }
        }

        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let mut diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &resources,
            &mut diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the corpus registry stays within the ledger budget");
        assert!(diagnostics.is_empty(), "the corpus types check clean");
        draft.commit();

        let ledger = corpus_ledger(roots);
        let outcome = DurableRegistry::build(
            &mut draft_owner,
            &records,
            &resources,
            &stores,
            Some(&ledger),
            &mut diagnostics,
            DeclarationBudget::default(),
        );
        Built {
            outcome: outcome.map(|_| ()),
            published: diagnostics
                .finish()
                .expect_complete()
                .iter()
                .map(|row| row.message().to_string())
                .collect(),
            state: draft_state(&draft_owner),
            image: draft_owner.encode().map(|encoded| encoded.bytes),
        }
    }

    /// What each last store stages, measured where the construction budget still admits
    /// its occurrence: what the same store contributes to a corpus that commits it.
    /// Without this, the restoration assertions below would hold just as well over a last
    /// store that staged nothing at all — the zero arm, which every checked refusal
    /// already takes before staging begins.
    #[test]
    fn each_last_store_stages_strictly_more_than_the_one_before_it() {
        let built = |last| {
            let built = build_corpus(ADMITTED_OCCURRENCES, last);
            assert!(
                built.outcome.is_ok(),
                "the admitted corpus builds for {last:?}: {:?}",
                built.outcome
            );
            let bytes = built
                .image
                .expect("the admitted corpus is inside every image bound")
                .len();
            (built.state, bytes)
        };
        let (absent, _) = built(LastStore::Absent);
        let (held, _) = built(LastStore::Held);
        let (one, one_bytes) = built(LastStore::One);
        let (many, many_bytes) = built(LastStore::Many);

        assert!(
            held.semantic_nodes > absent.semantic_nodes,
            "a further occurrence of a held Product stages a graph node of its own: \
             {held:?} against {absent:?}"
        );
        assert!(
            !held.one_declared && one.one_declared,
            "a last store over a Product no earlier store declared stages that \
             declaration, and one over a held Product does not: {one:?}"
        );
        assert!(
            many.many_declared
                && many.semantic_nodes > one.semantic_nodes
                && many_bytes > one_bytes,
            "a {MANY_FIELDS}-member Product declaration stages strictly more than a \
             one-member one: {many:?} at {many_bytes} bytes against {one:?} at \
             {one_bytes} bytes"
        );
    }

    /// The refused occurrence restores the draft it staged into and settles nothing —
    /// neither its own rows nor the row an earlier store had already settled.
    #[test]
    fn a_refused_root_occurrence_restores_the_draft_and_settles_no_store_payload() {
        let control = build_corpus(FILLED_OCCURRENCES, LastStore::Absent);
        assert!(
            control.outcome.is_ok(),
            "the corpus at the admitted occurrence count builds: {:?}",
            control.outcome
        );
        assert!(
            control.state.contract.is_some(),
            "the corpus has a durable graph identity to be restored to"
        );
        assert!(
            !control.state.one_declared && !control.state.many_declared,
            "no corpus declares a fresh Product until a last store occurs it"
        );
        let settled = control
            .published
            .iter()
            .find(|row| row.contains("`Missing` is not a resource in this project"))
            .expect("an earlier store's ordinary refusal publishes when the build completes")
            .clone();

        for last in [LastStore::Held, LastStore::One, LastStore::Many] {
            let refused = build_corpus(FILLED_OCCURRENCES, last);
            assert!(
                matches!(
                    refused.outcome,
                    Err(crate::types::BuildError::Invariant(
                        GenericInvariant::DurableConstructionRefused
                    ))
                ),
                "the occurrence past the admitted count is refused by the durable graph, got \
                 {:?} for {last:?}",
                refused.outcome
            );
            assert_eq!(
                refused.state, control.state,
                "the refused store's armed transaction restores every table it staged into, \
                 for {last:?}"
            );
            assert!(
                !refused.published.contains(&settled),
                "the aborted build settled an earlier store's row for {last:?}: {:?}",
                refused.published
            );
            assert!(
                refused.published.is_empty(),
                "a whole-build abort settles no store's payload for {last:?}, got {:?}",
                refused.published
            );
        }
    }
}
