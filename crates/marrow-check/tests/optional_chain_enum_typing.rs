mod support;

use marrow_check::{DiagnosticPayload, MarrowType, check_project};

use support::{config, temp_project, with_code, write};

/// A resolved optional-chain read that ends in an enum-typed saved field carries that
/// enum's nominal identity through the checker. `^books(id)?.binding?.state ?? <member>`
/// walks two optional group layers to the `state` field, typed by a *qualified* enum
/// `kinds::Status` declared in another module; the `??` resolves the maybe-present read,
/// so the whole expression is the enum value.
///
/// Forcing that value into an `int` place is the oracle: the mismatch is reported once
/// as `check.assignment_type` whose `found` type is exactly `Enum { module: "kinds",
/// name: "Status" }`. A read that lost the enum identity (typing as `Unknown`) would
/// raise no mismatch, and a read that stamped the wrong owner would carry a different
/// module — both fail this assertion.
#[test]
fn an_optional_chain_to_a_qualified_enum_field_types_as_that_enum() {
    let root = temp_project("optchain-qualified-enum", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/m.mw",
            "module m\nuse kinds\n\
             resource Book at ^books(id: int)\n\
             \x20   binding\n        state: kinds::Status\n\n\
             fn f(id: Id(^books))\n    \
             const n: int = (^books(id)?.binding?.state ?? kinds::Status::active)\n",
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
                name: "Status".into(),
            },
        },
        "{found:#?}"
    );
}

/// The same optional chain, resolved and read into its matching qualified enum, checks
/// clean. The read types as `kinds::Status`, so a `==` against a `kinds::Status` member
/// is a same-enum comparison the nominal-equality rule accepts. This is the positive
/// half: the identity that the mismatch test proves is *present* is also the right one,
/// so a like-for-like use is not over-rejected.
#[test]
fn an_optional_chain_to_a_qualified_enum_field_compares_clean_against_its_own_enum() {
    let root = temp_project("optchain-qualified-enum-clean", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/m.mw",
            "module m\nuse kinds\n\
             resource Book at ^books(id: int)\n\
             \x20   binding\n        state: kinds::Status\n\n\
             fn f(id: Id(^books)): bool\n    \
             return (^books(id)?.binding?.state ?? kinds::Status::active) == kinds::Status::active\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        !report.has_errors(),
        "an optional-chain enum read compared against its own enum must check clean: {:#?}",
        report.diagnostics
    );
}

/// The enum identity an optional chain produces is nominal, so comparing the read against
/// a *different* enum is a cross-enum operator error. The chain reads `kinds::Status`,
/// the comparison's right side is `kinds::Color`; even resolved through `??`, the read
/// keeps `Status` identity, so the `==` is rejected as `check.operator_type`. A read that
/// degraded to `Unknown` would silently accept this mismatch.
#[test]
fn an_optional_chain_enum_read_rejects_a_cross_enum_comparison() {
    let root = temp_project("optchain-qualified-enum-cross", |root| {
        write(
            root,
            "src/kinds.mw",
            "module kinds\npub enum Status\n    active\n    archived\n\npub enum Color\n    red\n    green\n",
        );
        write(
            root,
            "src/m.mw",
            "module m\nuse kinds\n\
             resource Book at ^books(id: int)\n\
             \x20   binding\n        state: kinds::Status\n\n\
             fn f(id: Id(^books)): bool\n    \
             return (^books(id)?.binding?.state ?? kinds::Status::active) == kinds::Color::red\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.operator_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}
