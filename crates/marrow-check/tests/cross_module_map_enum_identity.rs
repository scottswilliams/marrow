mod support;

use marrow_check::{DiagnosticPayload, MarrowType, StoredValueMeaning, check_project};

use support::{config, temp_project, with_code, write};

/// A `map[string, foreign::Enum]` saved member sugars to a keyed leaf whose stored value
/// is the foreign enum. The checked fact for that member must record the *foreign* enum's
/// nominal identity, not a local phantom: the `shades` leaf's stored-value meaning is
/// `Enum { enum_id }` where `enum_id` is exactly module `kinds`'s `Color`, the enum the
/// keyed-leaf value type names across the module boundary.
///
/// This is the typed-fact oracle: the member fact carries the same `EnumId` the project's
/// `kinds::Color` resolves to. A map leaf that dropped the foreign owner would either fail
/// to resolve (no enum meaning) or bind a different enum id, both of which this rejects.
#[test]
fn a_map_valued_by_a_foreign_enum_records_the_foreign_enum_identity_in_its_fact() {
    let root = temp_project("map-foreign-enum-fact", |root| {
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
             \x20   shades: map[string, kinds::Color]\n\
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
            "the map's leaf value must carry the foreign kinds::Color identity"
        ),
        other => panic!("expected a foreign-enum leaf meaning, found {other:?}"),
    }
}

/// The foreign enum identity survives a read of the map value through the type checker,
/// not only in the schema fact. Reading `^books(id).shades("a")` yields a `kinds::Color`
/// value; forcing it into an `int` place is a mismatch reported once as
/// `check.assignment_type` whose `found` is exactly `Enum { module: "kinds", name:
/// "Color" }`. A read that collapsed the foreign enum to `Unknown` would raise no
/// mismatch.
#[test]
fn reading_a_foreign_enum_map_value_into_a_scalar_place_carries_the_foreign_identity() {
    let root = temp_project("map-foreign-enum-read", |root| {
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
             \x20   shades: map[string, kinds::Color]\n\
             store ^books(id: int): Book\n\n\
             fn f(id: Id(^books))\n    \
             const n: int = (^books(id).shades(\"a\") ?? kinds::Color::red)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.assignment_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: MarrowType::Primitive(marrow_schema::ScalarType::Int),
            found: MarrowType::Enum {
                module: "kinds".into(),
                name: "Color".into(),
            },
        },
        "{found:#?}"
    );
}

/// Writing a same-foreign-enum member into the map value checks clean, while writing a
/// *different* foreign enum is a nominal mismatch. Both prove the map value's write
/// boundary enforces the foreign `kinds::Color` identity, not merely "some enum": the
/// `kinds::Color::red` write is accepted, and the `other::Shade::dark` write is rejected
/// as `check.assignment_type`.
#[test]
fn writing_the_map_value_enforces_the_foreign_enum_nominal_identity() {
    let root = temp_project("map-foreign-enum-write", |root| {
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
             \x20   shades: map[string, kinds::Color]\n\
             store ^books(id: int): Book\n\n\
             fn ok(id: Id(^books))\n    ^books(id).shades(\"a\") = kinds::Color::red\n\n\
             fn bad(id: Id(^books))\n    ^books(id).shades(\"a\") = other::Shade::dark\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
            expected: MarrowType::Enum {
                module: "kinds".into(),
                name: "Color".into(),
            },
            found: MarrowType::Enum {
                module: "other".into(),
                name: "Shade".into(),
            },
        },
        "{found:#?}"
    );
}
