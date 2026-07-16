//! Enum declarations: flat and nested members, the category modifier, member
//! grammar rules, and the clean enum round-trip through the formatter.

use crate::common;
use common::parse_reason;
use marrow_syntax::{ParseDiagnosticReason, parse_source};

fn member_names(decl: &marrow_syntax::EnumDecl) -> Vec<&str> {
    decl.members.iter().map(|m| m.name.as_str()).collect()
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn parses_a_flat_enum_declaration() {
    let parsed = parse_source("module app\nenum Status\n    active\n    archived\n    banned\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let status = parsed.file.enum_decl("Status").expect("Status enum");
    assert!(!status.public);
    assert_eq!(member_names(status), ["active", "archived", "banned"]);
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn parses_pub_enum() {
    let parsed = parse_source("module app\npub enum Status\n    active\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let status = parsed.file.enum_decl("Status").expect("Status enum");
    assert!(status.public);
    assert_eq!(member_names(status), ["active"]);
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn attaches_doc_comments_to_enum_members() {
    let parsed = parse_source("module app\nenum Status\n    ;; Currently live.\n    active\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let status = parsed.file.enum_decl("Status").expect("Status enum");
    assert_eq!(status.members[0].docs, ["Currently live."]);
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn rejects_an_enum_with_no_members() {
    let parsed = parse_source("module app\nenum Status\nfn main()\n    return\n");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.reason == parse_reason(ParseDiagnosticReason::EnumNeedsMember)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn rejects_an_enum_member_with_a_type_annotation() {
    let parsed = parse_source("module app\nenum Status\n    active: int\n");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.reason == parse_reason(ParseDiagnosticReason::EnumMemberMustBeBareName)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn parses_a_payload_member() {
    let parsed = parse_source(
        "module app\nenum Shape\n    dot\n    circle(radius: int)\n    rect(width: int, height: int)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let shape = parsed.file.enum_decl("Shape").expect("Shape enum");
    assert_eq!(member_names(shape), ["dot", "circle", "rect"]);
    assert!(shape.members[0].payload.is_empty(), "dot has no payload");
    let circle = &shape.members[1];
    let fields: Vec<&str> = circle.payload.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fields, ["radius"]);
    assert_eq!(shape.members[2].payload.len(), 2);
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn rejects_an_enum_member_with_an_empty_payload() {
    let parsed = parse_source("module app\nenum Status\n    active()\n");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.reason == parse_reason(ParseDiagnosticReason::EnumMemberMustBeBareName)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn round_trips_a_payload_enum_through_the_formatter() {
    let source = "enum Shape\n    dot\n    circle(radius: int)\n    rect(width: int, height: int)";
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(marrow_syntax::format_source(source), format!("{source}\n"));
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn parses_nested_enum_members_into_a_tree() {
    let parsed = parse_source(
        "module app\nenum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let cat = parsed.file.enum_decl("Cat").expect("Cat enum");
    assert_eq!(member_names(cat), ["tiger", "housecat"]);
    let tiger = &cat.members[0];
    assert!(tiger.category, "tiger should be a category");
    let nested: Vec<&str> = tiger.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(nested, ["bengal", "siberian"]);
    assert!(
        cat.members[1].members.is_empty(),
        "housecat has no children"
    );
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn the_category_modifier_sets_the_flag_and_a_bare_member_does_not() {
    let parsed =
        parse_source("module app\nenum Cat\n    category tiger\n        bengal\n    housecat\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let cat = parsed.file.enum_decl("Cat").expect("Cat enum");
    assert!(cat.members[0].category, "category tiger");
    assert!(!cat.members[1].category, "bare housecat");
    // The nested member is a plain member, not a category.
    assert!(!cat.members[0].members[0].category, "bengal");
}

#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn round_trips_an_enum_through_the_formatter() {
    let source = "enum Status\n    active\n    archived\n    banned";
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    // The canonical form of a single declaration is the declaration followed by a
    // trailing newline, so a clean enum round-trips unchanged.
    assert_eq!(marrow_syntax::format_source(source), format!("{source}\n"));
}
