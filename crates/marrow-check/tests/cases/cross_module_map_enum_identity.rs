use crate::support;
use marrow_check::{DiagnosticPayload, MarrowType, StoredValueMeaning, check_project};

use support::{config, temp_project, with_code, write};

#[test]
fn ambiguous_bare_foreign_enum_keyed_leaf_annotation_reports_unknown_type() {
    let root = temp_project("keyed-leaf-ambiguous-bare-foreign-enum", |root| {
        write(root, "src/a.mw", "module a\npub enum Status\n    active\n");
        write(root, "src/b.mw", "module b\npub enum Status\n    active\n");
        write(
            root,
            "src/m.mw",
            "module m\n\
             resource Book\n\
             \x20   statuses(key: string): Status\n\
             store ^books(id: int): Book\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let errors = with_code(&report, "check.unknown_type");
    assert_eq!(errors.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        errors[0].payload,
        DiagnosticPayload::AmbiguousType {
            ty: marrow_schema::Type::Named("Status".into()),
            name: "Status".into(),
        }
    );
}

/// A keyed leaf whose stored value is a foreign enum must record the *foreign* enum's
/// nominal identity, not a local phantom: the `shades` leaf's stored-value meaning is
/// `Enum { enum_id }` where `enum_id` is exactly module `kinds`'s `Color`, the enum the
/// keyed-leaf value type names across the module boundary.
///
/// This is the typed-fact oracle: the member fact carries the same `EnumId` the project's
/// `kinds::Color` resolves to. A keyed leaf that dropped the foreign owner would either
/// fail to resolve (no enum meaning) or bind a different enum id, both of which this rejects.
#[test]
fn a_keyed_leaf_valued_by_a_foreign_enum_records_the_foreign_enum_identity_in_its_fact() {
    let root = temp_project("keyed-leaf-foreign-enum-fact", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/m.mw",
            "module m\nuse kinds\n\
             resource Book\n\
             \x20   shades(key: string): kinds::Color\n\
             store ^books(id: int): Book\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let facts = &program.facts;
    let kinds = facts.module_id("kinds").expect("kinds module");
    let color = facts.enum_id(kinds, "Color").expect("kinds::Color");
    let m = facts.module_id("m").expect("m module");
    let book = facts.resource_id(m, "Book").expect("m::Book");
    let shades = facts
        .resource_member_id(book, &["shades"])
        .expect("Book.shades member");
    let shades_fact = facts
        .resource_members()
        .iter()
        .find(|member| member.id == shades)
        .expect("Book.shades fact");

    match &shades_fact.value_meaning {
        Some(StoredValueMeaning::Enum { enum_id, .. }) => assert_eq!(
            *enum_id, color,
            "the keyed leaf value must carry the foreign kinds::Color identity"
        ),
        other => panic!("expected a foreign-enum leaf meaning, found {other:?}"),
    }
}

/// The foreign enum identity survives a read of the keyed-leaf value through the type checker,
/// not only in the schema fact. Reading `^books(id).shades("a")` yields a `kinds::Color`
/// value; forcing it into an `int` place is a mismatch reported once as
/// `check.assignment_type` whose `found` is exactly `Enum { module: "kinds", name:
/// "Color" }`. A read that collapsed the foreign enum to `Unknown` would raise no
/// mismatch.
#[test]
fn reading_a_foreign_enum_keyed_leaf_value_into_a_scalar_place_carries_the_foreign_identity() {
    let root = temp_project("keyed-leaf-foreign-enum-read", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/m.mw",
            "module m\nuse kinds\n\
             resource Book\n\
             \x20   shades(key: string): kinds::Color\n\
             store ^books(id: int): Book\n\n\
             fn f(id: Id(^books))\n    \
             const n: int = (^books(id).shades(\"a\") ?? kinds::Color::red)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.assignment_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: MarrowType::Primitive(marrow_schema::ScalarType::Int),
            found: MarrowType::Enum(support::enum_id(&program, "kinds", "Color")),
        },
        "{found:#?}"
    );
}

/// Writing a same-foreign-enum member into the keyed leaf checks clean, while writing a
/// *different* foreign enum is a nominal mismatch. Both prove the keyed leaf's write
/// boundary enforces the foreign `kinds::Color` identity, not merely "some enum": the
/// `kinds::Color::red` write is accepted, and the `other::Shade::dark` write is rejected
/// as `check.assignment_type`.
#[test]
fn writing_the_keyed_leaf_value_enforces_the_foreign_enum_nominal_identity() {
    let root = temp_project("keyed-leaf-foreign-enum-write", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/other.mw",
            "module other\npub enum Shade\n    light\n    dark\n",
        );
        write(
            root,
            "src/m.mw",
            "module m\nuse kinds\nuse other\n\
             resource Book\n\
             \x20   shades(key: string): kinds::Color\n\
             store ^books(id: int): Book\n\n\
             fn ok(id: Id(^books))\n    ^books(id).shades(\"a\") = kinds::Color::red\n\n\
             fn bad(id: Id(^books))\n    ^books(id).shades(\"a\") = other::Shade::dark\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.assignment_type");
    assert_eq!(
        found.len(),
        1,
        "only the cross-enum write is rejected; the same-enum write is clean: {:#?}",
        report.diagnostics
    );
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            // The keyed-leaf write target is clearable, so it presents `kinds::Color?`.
            expected: MarrowType::Optional(Box::new(MarrowType::Enum(support::enum_id(
                &program, "kinds", "Color"
            )))),
            found: MarrowType::Enum(support::enum_id(&program, "other", "Shade")),
        },
        "{found:#?}"
    );
}
