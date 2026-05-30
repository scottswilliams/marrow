//! Enum compilation tests: ordinal assignment, member lookup, and the one
//! single-declaration rule an enum has (member uniqueness).

use marrow_schema::{EnumSchema, SCHEMA_DUPLICATE_MEMBER, compile_enum};
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
fn members_take_declaration_order_ordinals() {
    let schema = compile_ok("module app\nenum Status\n    active\n    archived\n    banned\n");
    assert_eq!(schema.name, "Status");
    assert_eq!(schema.ordinal("active"), Some(0));
    assert_eq!(schema.ordinal("archived"), Some(1));
    assert_eq!(schema.ordinal("banned"), Some(2));
    assert_eq!(schema.ordinal("missing"), None);
}

#[test]
fn member_name_inverts_the_ordinal() {
    let schema = compile_ok("module app\nenum Status\n    active\n    archived\n");
    assert_eq!(schema.member_name(0), Some("active"));
    assert_eq!(schema.member_name(1), Some("archived"));
    assert_eq!(schema.member_name(2), None);
}

#[test]
fn carries_member_docs_and_an_empty_stable_id_slot() {
    let schema = compile_ok("module app\nenum Status\n    ;; Currently live.\n    active\n");
    assert_eq!(schema.members[0].docs, ["Currently live."]);
    assert!(schema.members[0].stable_id.is_none());
}

#[test]
fn rejects_a_duplicate_member() {
    let (schema, errors) = compile_enum(&enum_decl(
        "module app\nenum Status\n    active\n    active\n",
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, SCHEMA_DUPLICATE_MEMBER);
    assert!(errors[0].message.contains("enum member"));
    // The duplicate is reported but not stored, so members and ordinals reflect
    // only the distinct members.
    assert_eq!(schema.members.len(), 1, "{:?}", schema.members);
    assert_eq!(schema.ordinal("active"), Some(0));
}
