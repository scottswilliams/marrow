//! Enum compilation tests: source traversal indices, member lookup, and the one
//! single-declaration rule an enum has (member uniqueness).

use marrow_schema::{
    EnumSchema, SCHEMA_CATEGORY_LEAF, SCHEMA_DUPLICATE_MEMBER, SCHEMA_PARENT_NOT_CATEGORY,
    SchemaErrorKind, compile_enum,
};
use marrow_syntax::{Declaration, EnumDecl, parse_source};

/// Parse `source` and return its single enum declaration.
fn enum_decl(source: &str) -> EnumDecl {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "source should parse cleanly: {:?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Enum(decl) => Some(decl),
            _ => None,
        })
        .expect("an enum declaration")
}

fn compile_ok(source: &str) -> EnumSchema {
    let (schema, errors) = compile_enum(&enum_decl(source));
    assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    schema
}

#[test]
fn members_keep_source_traversal_indices() {
    use marrow_schema::MemberPathResolution::{Found, NotFound};
    let schema = compile_ok("module app\nenum Status\n    active\n    archived\n    banned\n");
    assert_eq!(schema.name, "Status");
    assert_eq!(schema.walk_member_path(&["active"]), Found(0));
    assert_eq!(schema.walk_member_path(&["archived"]), Found(1));
    assert_eq!(schema.walk_member_path(&["banned"]), Found(2));
    assert_eq!(schema.walk_member_path(&["missing"]), NotFound);
}

#[test]
fn member_name_uses_the_traversal_index() {
    let schema = compile_ok("module app\nenum Status\n    active\n    archived\n");
    assert_eq!(schema.member_name(0), Some("active"));
    assert_eq!(schema.member_name(1), Some("archived"));
    assert_eq!(schema.member_name(2), None);
}

#[test]
fn carries_member_docs() {
    let schema = compile_ok("module app\nenum Status\n    ;; Currently live.\n    active\n");
    assert_eq!(schema.members[0].docs, ["Currently live."]);
}

#[test]
fn rejects_a_duplicate_member() {
    let (schema, errors) = compile_enum(&enum_decl(
        "module app\nenum Status\n    active\n    active\n",
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, SCHEMA_DUPLICATE_MEMBER);
    // The duplicate is reported and dropped, so traversal only sees the distinct
    // members.
    assert_eq!(schema.members.len(), 1, "{:?}", schema.members);
    assert_eq!(
        schema.walk_member_path(&["active"]),
        marrow_schema::MemberPathResolution::Found(0)
    );
}

/// A flat enum stays the degenerate one-level tree: every member at the top
/// level (`parent: None`), none a category, with traversal matching source order.
#[test]
fn a_flat_enum_compiles_as_one_traversal_level() {
    let schema = compile_ok("module app\nenum Status\n    active\n    archived\n    banned\n");
    assert_eq!(schema.members.len(), 3);
    for (index, member) in schema.members.iter().enumerate() {
        assert_eq!(member.parent, None, "{member:?}");
        assert!(!member.category, "{member:?}");
        assert_eq!(
            schema.walk_member_path(&[member.name.as_str()]),
            marrow_schema::MemberPathResolution::Found(index)
        );
    }
}

/// Nested members flatten in pre-order DFS — each parent before its children —
/// and `parent` links each child to its parent traversal index.
#[test]
fn nested_members_keep_pre_order_indices_and_parent_links() {
    let schema = compile_ok(
        "module app\nenum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n",
    );
    use marrow_schema::MemberPathResolution::Found;
    // Pre-order traversal: tiger(0), bengal(1), siberian(2), housecat(3).
    assert_eq!(schema.walk_member_path(&["tiger"]), Found(0));
    assert_eq!(schema.walk_member_path(&["bengal"]), Found(1));
    assert_eq!(schema.walk_member_path(&["siberian"]), Found(2));
    assert_eq!(schema.walk_member_path(&["housecat"]), Found(3));
    assert_eq!(schema.members[1].parent, Some(0));
    assert_eq!(schema.members[2].parent, Some(0));
    assert_eq!(schema.members[3].parent, None);
}

/// A `category` member compiles with its flag set; a bare member does not.
#[test]
fn a_category_member_carries_its_flag() {
    let schema =
        compile_ok("module app\nenum Cat\n    category tiger\n        bengal\n    housecat\n");
    assert!(schema.is_category(0), "tiger should be a category");
    assert!(!schema.is_category(1), "bengal should be concrete");
    assert!(!schema.is_category(2), "housecat should be concrete");
}

/// A category with no members is dead — never selectable, never matched — and is
/// rejected.
#[test]
fn rejects_a_category_with_no_members() {
    let (_schema, errors) = compile_enum(&enum_decl(
        "module app\nenum Cat\n    category tiger\n    housecat\n",
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, SCHEMA_CATEGORY_LEAF);
}

/// A non-category member that has nested members is a grouping node, but a value
/// can never select it (its value is one of its descendants) and a `match` can
/// never cover it. The two checker gates — value-position rejection (categories
/// only) and match coverage (childless non-categories only) — would then disagree,
/// admitting a value no arm can handle. Reject the parent at compile time so the
/// invariant category <=> has-children holds and the gates stay complementary.
#[test]
fn rejects_a_non_category_parent_with_children() {
    let (_schema, errors) = compile_enum(&enum_decl(
        "module app\nenum Cat\n    tiger\n        bengal\n        siberian\n    housecat\n",
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, SCHEMA_PARENT_NOT_CATEGORY);
    assert_eq!(
        errors[0].kind,
        SchemaErrorKind::ParentNotCategory {
            member: "tiger".to_string(),
        }
    );
}

/// Member-name uniqueness is per sibling level: two `tiger`s under one parent
/// collide, but the same name under different parents is fine.
#[test]
fn duplicate_member_uniqueness_is_per_sibling_level() {
    let (_, sibling_errors) = compile_enum(&enum_decl(
        "module app\nenum Cat\n    category tiger\n        bengal\n        bengal\n",
    ));
    assert_eq!(sibling_errors.len(), 1, "{sibling_errors:?}");
    assert_eq!(sibling_errors[0].code, SCHEMA_DUPLICATE_MEMBER);

    // `paw` appears under two different parents — distinct levels, no collision.
    let (_, cross_errors) = compile_enum(&enum_decl(
        "module app\nenum Cat\n    category tiger\n        paw\n    category lion\n        paw\n",
    ));
    assert!(cross_errors.is_empty(), "{cross_errors:?}");
}

/// The subtree helpers answer the hierarchy: `subtree_ordinals` lists a node and
/// its descendants inclusively, and `selectable_leaves` is the set of concrete
/// childless members.
#[test]
fn subtree_queries_describe_the_hierarchy() {
    let schema = compile_ok(
        "module app\nenum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n",
    );
    // tiger(0), bengal(1), siberian(2), housecat(3). The subtree of tiger is
    // inclusive of tiger itself and its descendants bengal and siberian, but not
    // the sibling housecat.
    let subtree: Vec<usize> = schema.subtree_ordinals(0).collect();
    assert_eq!(subtree, vec![0, 1, 2]);
    assert!(subtree.contains(&1), "bengal is under tiger");
    assert!(subtree.contains(&0), "a node is its own descendant");
    assert!(!subtree.contains(&3), "housecat is not under tiger");

    let leaves: Vec<usize> = schema.selectable_leaves().collect();
    assert_eq!(leaves, vec![1, 2, 3], "category tiger is not selectable");
}

/// The duplicate-name enum used by the member-path walk tests: `paw` appears under
/// both `tiger` and `lion`, a blessed feature. Pre-order traversal: tiger(0),
/// bengal(1), paw(2), lion(3), paw(4), mane(5).
fn duplicate_paw_enum() -> EnumSchema {
    compile_ok(
        "module app\nenum Cat\n\
         \x20   category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n",
    )
}

/// A qualified path walks parent→child to a single member, so two `paw`s under
/// different parents resolve to their own distinct traversal indices.
#[test]
fn walk_member_path_resolves_a_qualified_path_to_a_distinct_member() {
    use marrow_schema::MemberPathResolution::Found;
    let schema = duplicate_paw_enum();
    assert_eq!(schema.walk_member_path(&["tiger", "paw"]), Found(2));
    assert_eq!(schema.walk_member_path(&["lion", "paw"]), Found(4));
    assert_eq!(schema.walk_member_path(&["tiger", "bengal"]), Found(1));
    assert_eq!(schema.walk_member_path(&["lion", "mane"]), Found(5));
    // A bare top-level name still resolves when unique.
    assert_eq!(schema.walk_member_path(&["tiger"]), Found(0));
    assert_eq!(schema.walk_member_path(&["lion"]), Found(3));
}

/// A bare name shared by members under different parents is ambiguous, and the
/// resolution names the matching members in pre-order so the diagnostic can render
/// their qualifying paths.
#[test]
fn walk_member_path_reports_a_duplicated_bare_name_as_ambiguous() {
    let schema = duplicate_paw_enum();
    match schema.walk_member_path(&["paw"]) {
        marrow_schema::MemberPathResolution::Ambiguous(ordinals) => {
            assert_eq!(ordinals, vec![2, 4]);
            let paths: Vec<String> = ordinals
                .iter()
                .map(|&ordinal| schema.member_path(ordinal).join("::"))
                .collect();
            assert_eq!(
                paths,
                vec!["tiger::paw".to_string(), "lion::paw".to_string()]
            );
        }
        other => panic!("expected ambiguity, got {other:?}"),
    }
}

/// A first segment that is no member, or a later segment that is not a child of
/// the one before it, walks to nothing.
#[test]
fn walk_member_path_returns_not_found_for_an_unknown_step() {
    use marrow_schema::MemberPathResolution::NotFound;
    let schema = duplicate_paw_enum();
    assert_eq!(schema.walk_member_path(&["wolf"]), NotFound);
    assert_eq!(schema.walk_member_path(&["tiger", "mane"]), NotFound);
    assert_eq!(schema.walk_member_path(&["tiger", "paw", "claw"]), NotFound);
    assert_eq!(schema.walk_member_path(&[]), NotFound);
}
