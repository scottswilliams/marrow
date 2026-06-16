use crate::support;
use crate::support_enum;
use marrow_check::{DiagnosticPayload, EnumDiagnostic, check_project};
use marrow_schema::SchemaErrorKind;

use support::{
    assert_clean, assert_schema_payload, check_module, check_module_report, config, temp_project,
    with_code, write,
};
use support_enum::assert_enum_payload;

/// A nested enum used as a value, a `match` over its leaves, and `is` tests all
/// over one declaration, used by the hierarchy checker tests.
fn cat_enum() -> &'static str {
    "module m\n\
     enum Cat\n\
     \x20   category tiger\n\
     \x20       bengal\n\
     \x20       siberian\n\
     \x20   housecat\n"
}

#[test]
fn value_is_category_types_bool() {
    let report = check_module_report(
        "is-types-bool",
        &format!(
            "{}\
             fn f(pet: Cat): bool\n    \
             return pet is Cat::tiger\n",
            cat_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn is_against_a_concrete_leaf_is_clean() {
    // `is` against a concrete-leaf right operand is the exact case; it types `bool`
    // with no category error.
    let report = check_module_report(
        "is-leaf",
        &format!(
            "{}\
             fn f(pet: Cat): bool\n    \
             return pet is Cat::bengal\n",
            cat_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn is_with_a_non_enum_left_is_rejected() {
    let errors = check_module(
        "is-non-enum",
        &format!(
            "{}\
             fn f(): bool\n    \
             return 1 is Cat::tiger\n",
            cat_enum()
        ),
        "check.is_requires_enum",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn is_against_a_different_enum_is_rejected() {
    let errors = check_module(
        "is-cross-enum",
        &format!(
            "{}\
             enum Dog\n    \
             poodle\n    \
             beagle\n\n\
             fn f(pet: Cat): bool\n    \
             return pet is Dog::poodle\n",
            cat_enum()
        ),
        "check.is_type",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn is_operand_private_enum_diagnostic_carries_payload() {
    let root = temp_project("is-private-enum-payload", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             enum Hidden\n    one\n    two\n\
             pub fn hidden(): Hidden\n    return Hidden::one\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             fn f(): bool\n    return a::hidden() is a::Hidden::one\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.private_enum");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::PrivateEnum("a::Hidden".into())
    );
}

#[test]
fn a_category_is_not_selectable_in_value_position() {
    let errors = check_module(
        "category-not-selectable",
        &format!(
            "{}\
             fn f(): Cat\n    \
             return Cat::tiger\n",
            cat_enum()
        ),
        "check.category_not_selectable",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn a_category_member_error_does_not_emit_an_untyped_return_hint() {
    let report = check_module_report(
        "category-no-untyped-cascade",
        &format!(
            "{}\
             fn f(): Cat\n    \
             return Cat::tiger\n",
            cat_enum()
        ),
    );
    assert_eq!(
        with_code(&report, "check.category_not_selectable").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_match_with_a_category_arm_covers_its_subtree() {
    // A `tiger` arm covers both `bengal` and `siberian`; with `housecat` covered,
    // the match is exhaustive over the selectable leaves.
    let report = check_module_report(
        "match-category-arm",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             housecat\n            return 2\n",
            cat_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn a_match_missing_a_leaf_is_nonexhaustive() {
    // Listing only `bengal` and `housecat` leaves `siberian` uncovered.
    let errors = check_module(
        "match-missing-leaf",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             bengal\n            return 1\n        \
             housecat\n            return 2\n",
            cat_enum()
        ),
        "check.nonexhaustive_match",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_enum_payload(
        &errors[0],
        EnumDiagnostic::NonexhaustiveMatch {
            enum_name: "Cat".into(),
            missing: vec!["tiger::siberian".into()],
        },
    );
}

#[test]
fn a_non_category_parent_with_children_is_rejected() {
    // `tiger` has children but is not marked `category`, so it is a grouping node a
    // value can never hold and a `match` can never cover. The schema rule rejects it
    // at check time; without it the program would check clean here yet fault at run,
    // since `Cat::tiger` types as a value the match's leaf coverage cannot reach.
    // The repro uses `tiger` BOTH as a value (`var t: Cat = Cat::tiger`) and as a
    // match scrutinee, so the rejection lands regardless of how the parent is used.
    let errors = check_module(
        "parent-not-category",
        "module m\n\
         enum Cat\n    \
         tiger\n        \
         bengal\n        \
         siberian\n    \
         housecat\n\
         fn classify(pet: Cat): int\n    \
         match pet\n        \
         bengal\n            return 1\n        \
         siberian\n            return 2\n        \
         housecat\n            return 3\n\
         fn use_value(): int\n    \
         var t: Cat = Cat::tiger\n    \
         return classify(t)\n",
        "schema.parent_not_category",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_schema_payload(
        &errors[0],
        SchemaErrorKind::ParentNotCategory {
            member: "tiger".to_string(),
        },
    );
}

#[test]
fn a_correctly_marked_category_parent_checks_clean() {
    // With `category tiger`, `tiger` is a category rather than a value, and the
    // match's leaf coverage (`bengal`, `siberian`, `housecat`) is exhaustive, so the
    // program checks clean.
    let report = check_module_report(
        "parent-category-clean",
        "module m\n\
         enum Cat\n    \
         category tiger\n        \
         bengal\n        \
         siberian\n    \
         housecat\n\
         fn classify(pet: Cat): int\n    \
         match pet\n        \
         bengal\n            return 1\n        \
         siberian\n            return 2\n        \
         housecat\n            return 3\n",
    );
    assert_clean(&report);
}

#[test]
fn a_leaf_and_its_ancestor_category_overlap_is_a_duplicate_arm() {
    // Covering both `tiger` (the category) and `bengal` (a leaf under it) double-
    // covers `bengal`.
    let errors = check_module(
        "match-overlap",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             bengal\n            return 2\n        \
             housecat\n            return 3\n",
            cat_enum()
        ),
        "check.duplicate_match_arm",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn an_overlapping_arm_yields_only_a_duplicate_not_a_secondary_nonexhaustive() {
    // `bengal` covers itself, then the `tiger` category overlaps it and is rejected
    // as a duplicate. Rejecting `tiger` must not drop its other leaf (`siberian`)
    // from coverage and falsely report the match non-exhaustive: the overlap is one
    // clear diagnostic, never two.
    let report = check_module_report(
        "overlap-no-secondary-nonexhaustive",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             bengal\n            return 1\n        \
             tiger\n            return 2\n        \
             housecat\n            return 3\n",
            cat_enum()
        ),
    );
    assert_eq!(
        with_code(&report, "check.duplicate_match_arm").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.nonexhaustive_match").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_flat_enum_match_and_equality_still_check() {
    // A flat enum's `match` is exhaustive over its members and `==` is exact nominal
    // equality; both check clean.
    let report = check_module_report(
        "flat-enum-match-equality",
        "module m\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         fn label(s: Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n\n\
         fn same(): bool\n    return Status::active == Status::active\n",
    );
    assert_clean(&report);
}

/// An enum where `paw` appears under two categories — the blessed duplicate-name
/// case. Pre-order: tiger(0), bengal(1), paw(2), lion(3), paw(4), mane(5).
fn duplicate_paw_enum() -> &'static str {
    "module m\n\
     enum Cat\n\
     \x20   category tiger\n        bengal\n        paw\n\
     \x20   category lion\n        paw\n        mane\n"
}

#[test]
fn a_full_member_path_to_a_duplicated_leaf_resolves_in_value_position() {
    // `Cat::tiger::paw` and `Cat::lion::paw` are distinct members, both selectable.
    let report = check_module_report(
        "dup-value-full-path",
        &format!(
            "{}\
             fn a(): Cat\n    return Cat::tiger::paw\n\
             fn b(): Cat\n    return Cat::lion::paw\n",
            duplicate_paw_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn a_bare_duplicated_member_in_value_position_is_ambiguous() {
    // Bare `Cat::paw` names `paw` under both `tiger` and `lion`; the value cannot
    // pick one. The diagnostic payload records the qualifying paths.
    let errors = check_module(
        "dup-value-bare",
        &format!(
            "{}\
             fn a(): Cat\n    return Cat::paw\n",
            duplicate_paw_enum()
        ),
        "check.ambiguous_member",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_enum_payload(
        &errors[0],
        EnumDiagnostic::AmbiguousMember {
            enum_name: "Cat".into(),
            label: "paw".into(),
            candidates: vec!["tiger::paw".into(), "lion::paw".into()],
        },
    );
}

#[test]
fn an_ambiguous_enum_member_does_not_emit_an_untyped_return_hint() {
    let report = check_module_report(
        "dup-value-bare-no-untyped-cascade",
        &format!(
            "{}\
             fn a(): Cat\n    return Cat::paw\n",
            duplicate_paw_enum()
        ),
    );
    assert_eq!(
        with_code(&report, "check.ambiguous_member").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_match_with_qualified_arms_over_duplicated_leaves_is_exhaustive() {
    // Arms `tiger::paw`, `lion::paw`, `tiger::bengal`, `lion::mane` cover every
    // selectable leaf exactly once.
    let report = check_module_report(
        "dup-match-qualified",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger::bengal\n            return 1\n        \
             tiger::paw\n            return 2\n        \
             lion::paw\n            return 3\n        \
             lion::mane\n            return 4\n",
            duplicate_paw_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn a_bare_duplicated_match_arm_is_actionably_ambiguous() {
    // A bare `paw` arm cannot pick a subtree; the diagnostic payload records the
    // qualifying paths so the dev can disambiguate.
    let errors = check_module(
        "dup-match-bare-arm",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger::bengal\n            return 1\n        \
             paw\n            return 2\n        \
             lion::mane\n            return 3\n",
            duplicate_paw_enum()
        ),
        "check.ambiguous_match_arm",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_enum_payload(
        &errors[0],
        EnumDiagnostic::AmbiguousMatchArm {
            enum_name: "Cat".into(),
            label: "paw".into(),
            candidates: vec!["tiger::paw".into(), "lion::paw".into()],
        },
    );
}

#[test]
fn a_match_with_category_arms_over_a_duplicated_enum_is_exhaustive() {
    // Two category arms `tiger` and `lion` each cover their whole subtree, so every
    // leaf is covered exactly once.
    let report = check_module_report(
        "dup-match-category",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             lion\n            return 2\n",
            duplicate_paw_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn a_category_arm_overlapping_a_qualified_leaf_arm_is_a_duplicate() {
    // `tiger` covers `tiger::paw`, so a separate `tiger::paw` arm double-covers it.
    let errors = check_module(
        "dup-match-overlap",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger\n            return 1\n        \
             tiger::paw\n            return 2\n        \
             lion\n            return 3\n",
            duplicate_paw_enum()
        ),
        "check.duplicate_match_arm",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}

#[test]
fn a_match_missing_a_duplicated_leaf_reports_its_full_path() {
    // Dropping `lion::mane` leaves it uncovered; the payload records the full path
    // so a bare `mane` is unambiguous to the reader.
    let errors = check_module(
        "dup-match-nonexhaustive",
        &format!(
            "{}\
             fn f(pet: Cat): int\n    \
             match pet\n        \
             tiger::bengal\n            return 1\n        \
             tiger::paw\n            return 2\n        \
             lion::paw\n            return 3\n",
            duplicate_paw_enum()
        ),
        "check.nonexhaustive_match",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_enum_payload(
        &errors[0],
        EnumDiagnostic::NonexhaustiveMatch {
            enum_name: "Cat".into(),
            missing: vec!["lion::mane".into()],
        },
    );
}

#[test]
fn is_with_a_full_member_path_is_exact_and_a_category_is_a_subtree_test() {
    // `pet is Cat::tiger::paw` is the exact-leaf test; `pet is Cat::tiger` is the
    // subtree test. Both type clean — `is` admits a category right operand.
    let report = check_module_report(
        "dup-is-full-path",
        &format!(
            "{}\
             fn exact(pet: Cat): bool\n    return pet is Cat::tiger::paw\n\
             fn subtree(pet: Cat): bool\n    return pet is Cat::tiger\n",
            duplicate_paw_enum()
        ),
    );
    assert_clean(&report);
}

#[test]
fn is_with_a_bare_duplicated_member_is_ambiguous() {
    // A bare `Cat::paw` as an `is` operand is the symmetric footgun; reject it with
    // the same qualifying-path payload as value position.
    let errors = check_module(
        "dup-is-bare",
        &format!(
            "{}\
             fn f(pet: Cat): bool\n    return pet is Cat::paw\n",
            duplicate_paw_enum()
        ),
        "check.ambiguous_member",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_enum_payload(
        &errors[0],
        EnumDiagnostic::AmbiguousMember {
            enum_name: "Cat".into(),
            label: "paw".into(),
            candidates: vec!["tiger::paw".into(), "lion::paw".into()],
        },
    );
}
