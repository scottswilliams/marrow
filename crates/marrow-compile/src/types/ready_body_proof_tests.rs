//! The ready-body proof and matcher family, sharing the instantiation-state
//! fixtures through `super::*`.

use super::*;

#[test]
fn ready_body_proof_is_exact_selective_and_allows_ready_back_edges() {
    let make_registry = registry;
    let mut registry = make_registry(vec![
        template(
            "Outer",
            vec![
                ("safe", name("T")),
                ("child", apply("Inner", vec![name("T")])),
            ],
        ),
        template("Inner", vec![("value", name("T"))]),
    ]);
    let mut draft = fresh_draft();
    let outer = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(2))
        .expect("Outer<int> and its later Inner<int> row mint Ready");
    let TypeInstId::Record(outer_id) = outer else {
        panic!("Outer is record-shaped")
    };
    let (outer_row, inner_row, inner) = {
        let generics = registry.generics.borrow();
        let outer_row = generics
            .type_insts
            .iter()
            .position(|inst| inst.id == outer)
            .expect("Outer row exists");
        let inner_row = generics
            .type_insts
            .iter()
            .position(|inst| registry.type_templates[inst.template].name == "Inner")
            .expect("Inner row exists");
        (outer_row, inner_row, generics.type_insts[inner_row].id)
    };
    assert!(
        inner_row > outer_row,
        "body targets may be minted after their owner"
    );
    assert!(registry.type_inst_body(outer).unwrap().is_some());
    assert_eq!(
        registry.struct_field_projection(outer_id, "safe"),
        Ok(StructFieldProjection::Field {
            index: 0,
            ty: GArg::Scalar(ScalarType::Int),
        })
    );

    let inner_ready = {
        let mut generics = registry.generics.borrow_mut();
        std::mem::replace(
            &mut generics.type_insts[inner_row].state,
            TypeInstState::Rejected(ResolveRefusal::Unsupported),
        )
    };
    let expected = GenericInvariant::ReadyBodyMissing(inner);
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);
    assert_eq!(
        registry.struct_field_projection(outer_id, "safe"),
        Ok(StructFieldProjection::Field {
            index: 0,
            ty: GArg::Scalar(ScalarType::Int),
        }),
        "an unrelated unavailable field target does not block a selected safe field"
    );
    assert_eq!(
        registry.struct_field_projection(outer_id, "child"),
        Err(expected)
    );
    assert!(matches!(
        registry.type_inst_body(outer),
        Err(found) if found == expected
    ));
    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert!(matches!(
        ValueGraph::build(&registry),
        Err(found) if found == expected
    ));
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
    registry.generics.borrow_mut().type_insts[inner_row].state = inner_ready;

    let original_outer = registry.generics.borrow().type_insts[outer_row]
        .state
        .clone();
    let TypeInstState::Ready(InstBody::Struct(original_fields)) = &original_outer else {
        panic!("Outer row is Ready and struct-shaped")
    };
    let mut malformed = Vec::new();
    malformed.push(InstBody::Struct(Vec::new()));
    let mut renamed = original_fields.clone();
    renamed[0].0 = "renamed".to_string();
    malformed.push(InstBody::Struct(renamed));
    let mut wrong_type = original_fields.clone();
    wrong_type[0].1 = GArg::Scalar(ScalarType::Bool);
    malformed.push(InstBody::Struct(wrong_type));

    for body in malformed {
        registry.generics.borrow_mut().type_insts[outer_row].state = TypeInstState::Ready(body);
        let expected = GenericInvariant::ReadyBodyShapeMismatch(outer);
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);
        assert!(matches!(
            registry.type_inst_body(outer),
            Err(found) if found == expected
        ));
        assert_eq!(
            registry.struct_field_projection(outer_id, "safe"),
            Err(expected)
        );
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }
    registry.generics.borrow_mut().type_insts[outer_row].state = original_outer;

    for templates in [
        vec![template(
            "Node",
            vec![("next", apply("Node", vec![name("T")]))],
        )],
        vec![
            template("Left", vec![("right", apply("Right", vec![name("T")]))]),
            template("Right", vec![("left", apply("Left", vec![name("T")]))]),
        ],
    ] {
        let mut recursive = make_registry(templates);
        let mut recursive_draft = fresh_draft();
        let root = recursive
            .mint_type_instance(
                &mut recursive_draft,
                0,
                &[GArg::Scalar(ScalarType::Int)],
                site(9),
            )
            .expect("Ready body back edges are admitted after settlement");
        assert!(recursive.type_inst_body(root).unwrap().is_some());
        assert!(validate_ready_metadata(&recursive).is_ok());
    }
}

#[test]
fn ready_body_nested_template_contract_fails_every_selected_boundary_exactly() {
    #[derive(Clone, Copy)]
    enum Fault {
        TemplateKind,
        ArgumentCount,
    }

    for fault in [Fault::TemplateKind, Fault::ArgumentCount] {
        let mut registry = registry(vec![
            template(
                "Outer",
                vec![
                    ("safe", name("T")),
                    ("child", apply("Inner", vec![name("T")])),
                ],
            ),
            template("Inner", vec![("value", name("T"))]),
        ]);
        let mut draft = fresh_draft();
        let outer = registry
            .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(20))
            .expect("Outer<int> and Inner<int> mint Ready");
        let TypeInstId::Record(outer_id) = outer else {
            panic!("Outer is record-shaped")
        };
        let inner_row = registry
            .generics
            .borrow()
            .type_insts
            .iter()
            .position(|inst| registry.type_templates[inst.template].name == "Inner")
            .expect("Inner row exists");
        let inner_template = registry.generics.borrow().type_insts[inner_row].template;
        let expected = match fault {
            Fault::TemplateKind => {
                registry.type_templates[inner_template].body =
                    TemplateBody::Enum(Vec::new().into());
                GenericInvariant::TemplateKindMismatch {
                    template: inner_template,
                    expected: TypeInstKind::Enum,
                    actual: TypeInstKind::Struct,
                }
            }
            Fault::ArgumentCount => {
                registry.generics.borrow_mut().type_insts[inner_row]
                    .args
                    .clear();
                GenericInvariant::TypeArgumentCountMismatch {
                    template: inner_template,
                    expected: 1,
                    actual: 0,
                }
            }
        };
        // The template or row was corrupted out of the append order, so the
        // classified directory must be discarded before a probe reclassifies it.
        registry.invalidate_row_directory();
        let owner_before = stable_snapshot(&registry);
        let draft_before = draft_snapshot(&draft);

        let (selected, builds) =
            count_metadata_directory_builds(|| registry.struct_field_projection(outer_id, "safe"));
        assert_eq!(selected, Err(expected));
        assert_eq!(builds, 1);

        let (body, builds) = count_metadata_directory_builds(|| registry.type_inst_body(outer));
        assert!(matches!(body, Err(found) if found == expected));
        assert_eq!(builds, 1);

        // The preceding projection rebuilt and cached a directory over the corrupted
        // owners; discard it so the mint path reclassifies from the owners itself and
        // this boundary's one-build cost stays independent of the earlier probe.
        registry.invalidate_row_directory();
        let (replayed, builds) = count_metadata_directory_builds(|| {
            registry.mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(21))
        });
        assert_eq!(replayed, Err(ResolveError::Invariant(expected)));
        assert_eq!(builds, 1);

        let (cloned, builds) =
            count_metadata_directory_builds(|| validate_ready_metadata(&registry));
        assert!(matches!(cloned, Err(found) if found == expected));
        assert_eq!(builds, 1);

        let (graph, builds) = count_metadata_directory_builds(|| ValueGraph::build(&registry));
        assert!(matches!(graph, Err(found) if found == expected));
        assert_eq!(builds, 1);
        assert_eq!(stable_snapshot(&registry), owner_before);
        assert_eq!(draft_snapshot(&draft), draft_before);
    }
}

#[test]
fn ready_body_matcher_visits_deep_borrowed_template_once_per_node() {
    let mut registry = registry(vec![template("Deep", vec![("value", name("T"))])]);
    let mut draft = fresh_draft();
    let id = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(22))
        .expect("the shallow seed row mints Ready");
    let depth = MAX_INSTANTIATIONS;
    let mut expected = name("T");
    let mut actual = GArg::Scalar(ScalarType::Int);
    {
        let mut collections = registry.collections.borrow_mut();
        collections.reserve(depth);
        for index in 0..depth {
            collections.push(CollSpec::List { elem: actual });
            actual = GArg::Collection(coll(index as u16));
            expected = apply("List", vec![expected]);
        }
    }
    registry.type_templates[0].body =
        TemplateBody::Struct(vec![("value".to_string(), expected)].into());
    registry.generics.borrow_mut().type_insts[0].state =
        TypeInstState::Ready(InstBody::Struct(vec![("value".to_string(), actual)]));
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    let ((body, visits), builds) = count_metadata_directory_builds(|| {
        count_ready_body_match_visits(|| registry.type_inst_body(id).map(|body| body.is_some()))
    });
    assert_eq!(body, Ok(true));
    assert_eq!(builds, 1);
    assert_eq!(
        visits,
        depth + 1,
        "each List node and the terminal parameter is visited exactly once",
    );
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);

    // Remove the hostile deep template iteratively so the test also avoids a
    // recursive destructor after proving the production matcher is iterative.
    let body = std::mem::replace(
        &mut registry.type_templates[0].body,
        TemplateBody::Struct(vec![("value".to_string(), name("T"))].into()),
    );
    let TemplateBody::Struct(mut fields) = body else {
        panic!("Deep remains struct-shaped")
    };
    // Moved out of the shared entries rather than cloned: the replaced body holds the
    // only handle to them, and a deep type expression is as hostile to a recursive
    // `Clone` as it is to a recursive `Drop`.
    let entries = Rc::get_mut(&mut fields).expect("the replaced body is the only handle");
    let mut current = std::mem::replace(&mut entries[0].1, name("T"));
    loop {
        match current {
            TypeExpr::Apply { mut args, .. } => {
                current = args.pop().expect("each List has one argument");
            }
            TypeExpr::Name { .. } => break,
            TypeExpr::Optional { .. } | TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => {
                panic!("the deep matcher fixture contains only List and T")
            }
        }
    }
}

#[test]
fn ready_body_matcher_preserves_alias_precedence_over_template_parameters() {
    let mut alias = BTreeMap::new();
    alias.insert("Alias".to_string(), name("int"));
    let mut alias_template = template("AliasBox", vec![("value", name("Alias"))]);
    alias_template.type_params = vec![("Alias".to_string(), None)];
    let mut registry = registry(vec![alias_template]);
    registry.aliases = alias;
    let mut draft = fresh_draft();
    let id = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Text)], site(23))
        .expect("alias expansion wins before template substitution while minting");
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    let ((body, visits), builds) = count_metadata_directory_builds(|| {
        count_ready_body_match_visits(|| registry.type_inst_body(id))
    });
    assert!(matches!(
        body,
        Ok(Some(InstBody::Struct(ref fields)))
            if fields
                == &vec![("value".to_string(), GArg::Scalar(ScalarType::Int))]
    ));
    assert_eq!(
        visits, 2,
        "the alias name and expanded scalar are visited once"
    );
    assert_eq!(builds, 1);
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn ready_enum_payload_targets_are_checked_before_shape_or_durable_projection() {
    let mut registry = registry(vec![
        enum_template("Outer", apply("Inner", vec![name("T")])),
        template("Inner", vec![("value", name("T"))]),
    ]);
    let mut draft = fresh_draft();
    let outer = registry
        .mint_type_instance(&mut draft, 0, &[GArg::Scalar(ScalarType::Int)], site(12))
        .expect("Outer<int> and its payload target mint Ready");
    let TypeInstId::Enum(outer_id) = outer else {
        panic!("Outer is enum-shaped")
    };
    let (inner_row, inner) = {
        let generics = registry.generics.borrow();
        let row = generics
            .type_insts
            .iter()
            .position(|inst| registry.type_templates[inst.template].name == "Inner")
            .expect("Inner row exists");
        (row, generics.type_insts[row].id)
    };
    registry.generics.borrow_mut().type_insts[inner_row].state =
        TypeInstState::Rejected(ResolveRefusal::Unsupported);
    let expected = GenericInvariant::ReadyBodyMissing(inner);
    let owner_before = stable_snapshot(&registry);
    let draft_before = draft_snapshot(&draft);

    assert_eq!(registry.enum_variants(outer_id), Err(expected));
    assert!(matches!(
        registry.with_metadata_session(|session| {
            session.durable_enum_shape_and_anchor(outer_id)
        }),
        Err(found) if found == expected
    ));
    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert!(matches!(
        ValueGraph::build(&registry),
        Err(found) if found == expected
    ));
    assert_eq!(stable_snapshot(&registry), owner_before);
    assert_eq!(draft_snapshot(&draft), draft_before);
}

#[test]
fn ready_template_id_mismatch_is_typed_after_body_id_validation() {
    let mut registry = registry(reserved_templates());
    let mut draft = fresh_draft();
    let enum_id = registry
        .instantiate_reserved_option(&mut draft, GArg::Scalar(ScalarType::Int), site(2))
        .expect("Option row mints ready");
    registry.type_templates[0].body = TemplateBody::Struct(Vec::new().into());
    let expected = GenericInvariant::TemplateKindMismatch {
        template: 0,
        expected: TypeInstKind::Struct,
        actual: TypeInstKind::Enum,
    };
    let before = stable_snapshot(&registry);

    assert_eq!(registry.as_option(enum_id), Err(expected));
    assert!(matches!(
        validate_ready_metadata(&registry),
        Err(found) if found == expected
    ));
    assert_eq!(stable_snapshot(&registry), before);
}
