//! Resource members parse in source-declaration order. Member identity in the
//! catalog is positional in source, so the parser must preserve the exact
//! sequence a resource body declares — fields and nested groups interleaved —
//! rather than only recording which members are present.

use marrow_syntax::{ResourceMember, parse_source};

/// The declared name of a resource member, in source order.
fn member_name(member: &ResourceMember) -> &str {
    match member {
        ResourceMember::Field(field) => &field.name,
        ResourceMember::Group(group) => &group.name,
    }
}

/// A resource body that interleaves required fields, sparse fields, and a nested
/// group parses with its members in exact source-declaration order. Reordering
/// any line would change this sequence, so the assertion pins order, not just
/// presence.
#[test]
fn resource_members_keep_source_declaration_order() {
    let parsed = parse_source(
        "module app\n\
         resource Patient {\n\
         \x20   required mrn: string\n\
         \x20   required lastName: string\n\
         \x20   firstName: string\n\
         \x20   name {\n\
         \x20       required first: string\n\
         \x20       required last: string\n\
         \x20   }\n\
         \x20   note: string\n\
         }\n\
         store ^patients[id: string]: Patient\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let resource = parsed.file.resource("Patient").expect("Patient resource");

    let order: Vec<&str> = resource.members.iter().map(member_name).collect();
    assert_eq!(
        order,
        vec!["mrn", "lastName", "firstName", "name", "note"],
        "resource members must parse in source-declaration order",
    );

    // The nested group's own members are likewise ordered as declared.
    let ResourceMember::Group(name_group) = &resource.members[3] else {
        panic!(
            "expected `name` to parse as a group: {:#?}",
            resource.members
        );
    };
    let nested: Vec<&str> = name_group.members.iter().map(member_name).collect();
    assert_eq!(nested, vec!["first", "last"]);
}

/// Order preservation does not depend on member kind: a field declared after a
/// group still follows it. This guards against a parser that batches fields and
/// groups into separate passes, which would silently reorder the body.
#[test]
fn a_field_after_a_group_keeps_its_trailing_position() {
    let parsed = parse_source(
        "module app\n\
         resource Order {\n\
         \x20   lines[pos: int] {\n\
         \x20       sku: string\n\
         \x20   }\n\
         \x20   total: int\n\
         }\n\
         store ^orders[id: int]: Order\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let resource = parsed.file.resource("Order").expect("Order resource");

    let order: Vec<&str> = resource.members.iter().map(member_name).collect();
    assert_eq!(order, vec!["lines", "total"]);
    assert!(
        matches!(resource.members[0], ResourceMember::Group(_)),
        "the group must stay first",
    );
    assert!(
        matches!(resource.members[1], ResourceMember::Field(_)),
        "the trailing field must stay last",
    );
}
