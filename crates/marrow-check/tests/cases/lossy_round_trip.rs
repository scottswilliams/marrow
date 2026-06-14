use crate::support;
use marrow_check::{
    CHECK_CALL_ARGUMENT, CHECK_KEY_TYPE, CHECK_LOSSY_ROUND_TRIP, CHECK_UNRESOLVED_NAME,
    check_project,
};
use marrow_syntax::Severity;

use support::{config, temp_project, with_code, write};

fn check_source(name: &str, source: &str) -> marrow_check::CheckReport {
    let root = temp_project(name, |root| write(root, "src/m.mw", source));
    let (report, _program) = check_project(&root, &config()).expect("check");
    report
}

#[test]
fn whole_saved_root_assignment_with_keyed_child_layer_warns() {
    let report = check_source(
        "lossy-root-replacement",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn replace(id: int, replacement: Book)\n    ^books(id) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(found[0].severity, Severity::Warning);
}

#[test]
fn nested_keyed_child_layer_under_unkeyed_group_warns() {
    let report = check_source(
        "lossy-nested-root-replacement",
        "module m\n\
         resource Book\n    required title: string\n    audit\n        events(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn replace(id: int, replacement: Book)\n    ^books(id) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(found[0].severity, Severity::Warning);
}

#[test]
fn sequence_sugar_child_layer_warns() {
    let report = check_source(
        "lossy-sequence-root-replacement",
        "module m\n\
         resource Book\n    required title: string\n    tags: sequence[string]\n\
         store ^books(id: int): Book\n\n\
         fn replace(id: int, replacement: Book)\n    ^books(id) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(found[0].severity, Severity::Warning);
}

#[test]
fn keyed_group_child_layer_warns() {
    let report = check_source(
        "lossy-keyed-group-root-replacement",
        "module m\n\
         resource Book\n    required title: string\n    versions(version: int)\n        title: string\n\
         store ^books(id: int): Book\n\n\
         fn replace(id: int, replacement: Book)\n    ^books(id) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(found[0].severity, Severity::Warning);
}

#[test]
fn composite_identity_splice_whole_saved_root_assignment_warns() {
    let report = check_source(
        "lossy-composite-identity-splice",
        "module m\n\
         resource Enrollment\n    required status: string\n    notes(pos: int): string\n\
         store ^enrollments(student: string, course: string): Enrollment\n\n\
         fn replace(id: Id(^enrollments), replacement: Enrollment)\n    ^enrollments(id) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(found[0].severity, Severity::Warning);
}

#[test]
fn composite_scalar_key_whole_saved_root_assignment_warns() {
    let report = check_source(
        "lossy-composite-scalar-keys",
        "module m\n\
         resource Enrollment\n    required status: string\n    notes(pos: int): string\n\
         store ^enrollments(student: string, course: string): Enrollment\n\n\
         fn replace(student: string, course: string, replacement: Enrollment)\n    ^enrollments(student, course) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(found[0].severity, Severity::Warning);
}

#[test]
fn whole_saved_root_assignment_without_keyed_child_layers_does_not_warn() {
    let report = check_source(
        "non-lossy-root-replacement",
        "module m\n\
         resource Book\n    required title: string\n    meta\n        shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn replace(id: int, replacement: Book)\n    ^books(id) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn invalid_saved_root_addressing_does_not_warn() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "wrong-store-identity-splice",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             resource Magazine\n    required title: string\n\
             store ^books(id: int): Book\n\
             store ^magazines(id: int): Magazine\n\n\
             fn replace(id: Id(^magazines), replacement: Book)\n    ^books(id) = replacement\n",
            CHECK_KEY_TYPE,
        ),
        (
            "named-saved-key",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn replace(id: int, replacement: Book)\n    ^books(id: id) = replacement\n",
            CHECK_CALL_ARGUMENT,
        ),
        (
            "wrong-key-type",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn replace(id: string, replacement: Book)\n    ^books(id) = replacement\n",
            CHECK_KEY_TYPE,
        ),
        (
            "identity-range-key",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn replace(lo: Id(^books), hi: Id(^books), replacement: Book)\n    ^books(lo..hi) = replacement\n",
            CHECK_KEY_TYPE,
        ),
        (
            "unresolved-key",
            "module m\n\
             resource Book\n    required title: string\n    tags(pos: int): string\n\
             store ^books(id: int): Book\n\n\
             fn replace(replacement: Book)\n    ^books(missing) = replacement\n",
            CHECK_UNRESOLVED_NAME,
        ),
        (
            "invalid-arity",
            "module m\n\
             resource Enrollment\n    required status: string\n    notes(pos: int): string\n\
             store ^enrollments(student: string, course: string): Enrollment\n\n\
             fn replace(student: string, replacement: Enrollment)\n    ^enrollments(student) = replacement\n",
            CHECK_KEY_TYPE,
        ),
    ];

    for (name, source, expected_error) in cases {
        let report = check_source(name, source);
        assert!(
            !with_code(&report, expected_error).is_empty(),
            "{name}: {:#?}",
            report.diagnostics
        );
        let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
        assert!(found.is_empty(), "{name}: {:#?}", report.diagnostics);
    }
}

#[test]
fn saved_field_assignment_does_not_warn() {
    let report = check_source(
        "field-write-preserves-children",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn retitle(id: int, title: string)\n    ^books(id).title = title\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn keyed_child_entry_assignment_does_not_warn() {
    let report = check_source(
        "keyed-child-entry-preserves-root-siblings",
        "module m\n\
         resource Book\n    required title: string\n    versions(version: int)\n        title: string\n\
         store ^books(id: int): Book\n\n\
         fn replace_version(id: int, version: int, replacement: Book)\n    ^books(id).versions(version) = replacement\n",
    );

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let found = with_code(&report, CHECK_LOSSY_ROUND_TRIP);
    assert!(found.is_empty(), "{:#?}", report.diagnostics);
}
