//! The analysis snapshot classifies a completion position purely positionally over the
//! retained parse tree and enumerates the complete in-scope candidate namespace for that
//! class. The query runs on a broken file over recovered incomplete forms, fails soft on
//! an unresolvable base, refuses an over-cap namespace as a query-local resource limit
//! rather than truncating, and distinguishes an invalid coordinate from a legitimate
//! absence.

use std::sync::Arc;

use marrow_compile::{
    AnalysisResourceLimit, AnalysisSnapshot, CandidateKind, CompletionOutcome, Fact, InputRevision,
    MAX_COMPLETION_CANDIDATES, MAX_COMPLETION_RENDER_BYTES, PositionClass, QueryError,
    Unavailability, analyze,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

fn project_bytes(files: &[(&str, Vec<u8>)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.clone()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn snap(source: &str) -> Arc<AnalysisSnapshot> {
    let files = [("src/app.mw", source.as_bytes().to_vec())];
    let Ok(snapshot) = analyze(Arc::new(project_bytes(&files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot");
    };
    snapshot
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

/// The byte offset of `needle` in `source`, advanced by `extra` bytes.
fn at(source: &str, needle: &str, extra: usize) -> usize {
    source.find(needle).expect("needle present") + extra
}

/// The class and labels of a present completion fact, or a panic.
fn labels(snapshot: &AnalysisSnapshot, offset: usize) -> (PositionClass, Vec<String>) {
    match snapshot.completions(&identity("src/app.mw"), offset) {
        Ok(CompletionOutcome::Ready(Fact::Present(completions))) => (
            completions.class(),
            completions
                .candidates()
                .iter()
                .map(|candidate| candidate.label().to_string())
                .collect(),
        ),
        other => panic!("expected Present completions, got {}", describe(&other)),
    }
}

fn describe(outcome: &Result<CompletionOutcome, QueryError>) -> &'static str {
    match outcome {
        Ok(CompletionOutcome::Ready(Fact::Present(_))) => "Present",
        Ok(CompletionOutcome::Ready(Fact::Absent)) => "Absent",
        Ok(CompletionOutcome::Ready(Fact::Unavailable(_))) => "Unavailable",
        Ok(CompletionOutcome::Refused(_)) => "Refused",
        Err(_) => "QueryError",
    }
}

#[test]
fn completions_expression_name() {
    let source = "module app\n\n\
        fn helper(): int {\n    return 1\n}\n\n\
        fn caller(): int {\n    const total = 2\n    return t\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return t\n}", "return ".len());
    let (class, labels) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::ExpressionName);
    assert!(
        labels.iter().any(|l| l == "total"),
        "local in scope: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "helper"),
        "module fn: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "caller"),
        "module fn: {labels:?}"
    );
    assert!(labels.iter().any(|l| l == "some"), "builtin: {labels:?}");
}

#[test]
fn completions_member_fields() {
    let source = "module app\n\n\
        struct Point {\n    x: int\n    y: int\n}\n\n\
        fn f(p: Point): int {\n    return p.\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return p.\n", "return p.".len());
    let (class, labels) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::Member);
    assert_eq!(labels, vec!["x".to_string(), "y".to_string()]);
}

#[test]
fn completions_enum_members() {
    let source = "module app\n\n\
        enum Role {\n    admin\n    guest\n}\n\n\
        fn f(): int {\n    const r = Role::\n    return 1\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "Role::\n", "Role::".len());
    let (class, labels) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::EnumPath);
    assert_eq!(labels, vec!["admin".to_string(), "guest".to_string()]);
}

#[test]
fn completions_enum_members_mark_category_non_selectable() {
    let source = "module app\n\n\
        enum Cat {\n    category tiger {\n        bengal\n    }\n    lion\n}\n\n\
        fn f(): int {\n    const r = Cat::\n    return 1\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "Cat::\n", "Cat::".len());
    match snapshot.completions(&identity("src/app.mw"), offset) {
        Ok(CompletionOutcome::Ready(Fact::Present(completions))) => {
            let tiger = completions
                .candidates()
                .iter()
                .find(|candidate| candidate.label() == "tiger")
                .expect("tiger member");
            assert_eq!(
                tiger.kind(),
                CandidateKind::EnumMember { selectable: false },
                "a category member is non-selectable",
            );
        }
        other => panic!("expected Present, got {}", describe(&other)),
    }
}

#[test]
fn completions_type_annotation() {
    let source = "module app\n\n\
        struct Thing {\n    v: int\n}\n\n\
        fn f(): int {\n    const x: Thing = Thing(v: 1)\n    return x.v\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "const x: Thing", "const x: ".len());
    let (class, labels) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::TypeAnnotation);
    assert!(
        labels.iter().any(|l| l == "Thing"),
        "named type: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "int"),
        "builtin type: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Map"),
        "reserved generic: {labels:?}"
    );
}

#[test]
fn completions_type_annotation_offers_generic_type_parameters() {
    let source = "module app\n\n\
        fn f<T>(value: T): int {\n    const x: T = value\n    return 1\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "const x: T =", "const x: ".len());
    let (class, labels) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::TypeAnnotation);
    assert!(
        labels.iter().any(|l| l == "T"),
        "type parameter in scope: {labels:?}"
    );
}

#[test]
fn completions_broken_file_still_classifies() {
    // The recovery node itself makes the file broken (it carries a parse.syntax error),
    // yet the position over it classifies — the whole point of parser-owned recovery.
    let source = "module app\n\n\
        struct Point {\n    x: int\n    y: int\n}\n\n\
        fn f(p: Point): int {\n    return p.\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return p.\n", "return p.".len());
    let file = identity("src/app.mw");

    // The file is genuinely broken: a hover at the same offset is syntax-unavailable.
    assert!(
        matches!(
            snapshot.hover(&file, offset),
            Ok(Fact::Unavailable(Unavailability::Syntax))
        ),
        "the file must be broken for this to prove the law",
    );
    // Completion nonetheless classifies over the recovered node.
    let (class, _) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::Member);
}

#[test]
fn completions_unresolvable_base_is_absent_fields_not_panic() {
    // A hostile broken base: a name that resolves to nothing. The fail-soft type probe
    // yields an empty field set, never a resolver failure or panic.
    let source = "module app\n\n\
        fn f(): int {\n    return mystery.\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return mystery.\n", "return mystery.".len());
    let (class, labels) = labels(&snapshot, offset);
    assert_eq!(class, PositionClass::Member);
    assert!(
        labels.is_empty(),
        "no fields for an unresolvable base: {labels:?}"
    );
}

#[test]
fn completions_over_cap_refuses() {
    let mut source = String::from("module app\n\n");
    for index in 0..(MAX_COMPLETION_CANDIDATES + 8) {
        source.push_str(&format!("fn f{index}(): int {{\n    return 1\n}}\n\n"));
    }
    source.push_str("fn caller(): int {\n    return f0\n}\n");
    let snapshot = snap(&source);
    let offset = at(&source, "return f0\n}", "return ".len());
    match snapshot.completions(&identity("src/app.mw"), offset) {
        Ok(CompletionOutcome::Refused(AnalysisResourceLimit::CompletionCandidateCount {
            limit,
        })) => {
            assert_eq!(limit, MAX_COMPLETION_CANDIDATES);
        }
        other => panic!(
            "expected a candidate-count refusal, got {}",
            describe(&other)
        ),
    }
}

/// A struct with `field_count` fields (each named `f<index>` zero-padded to
/// `name_width` characters, typed `int`) and a `fn f(p: Big): int { return p. }` whose
/// trailing `p.` member position offers exactly one candidate per field.
fn member_source(field_count: usize, name_width: usize) -> String {
    let mut source = String::from("module app\n\nstruct Big {\n");
    for index in 0..field_count {
        source.push_str(&format!("    f{index:0>name_width$}: int\n"));
    }
    source.push_str("}\n\nfn f(p: Big): int {\n    return p.\n}\n");
    source
}

fn member_outcome(source: &str) -> Result<CompletionOutcome, QueryError> {
    let snapshot = snap(source);
    let offset = at(source, "return p.\n", "return p.".len());
    snapshot.completions(&identity("src/app.mw"), offset)
}

#[test]
fn completions_candidate_count_boundary_admits_max_and_refuses_one_more() {
    // Exactly the cap is admitted; one past it refuses. A struct member position offers
    // exactly one candidate per field with no builtins folded in, so the count is the
    // field count precisely — pinning the `> MAX` boundary so an off-by-one to `>=`
    // cannot pass unnoticed.
    let cap = MAX_COMPLETION_CANDIDATES as usize;

    match member_outcome(&member_source(cap, 3)) {
        Ok(CompletionOutcome::Ready(Fact::Present(completions))) => {
            assert_eq!(
                completions.candidates().len(),
                cap,
                "exactly MAX_COMPLETION_CANDIDATES candidates are present",
            );
        }
        other => panic!("expected exactly-cap Present, got {}", describe(&other)),
    }

    match member_outcome(&member_source(cap + 1, 3)) {
        Ok(CompletionOutcome::Refused(AnalysisResourceLimit::CompletionCandidateCount {
            limit,
        })) => assert_eq!(limit, MAX_COMPLETION_CANDIDATES),
        other => panic!(
            "expected a candidate-count refusal one past the cap, got {}",
            describe(&other)
        ),
    }
}

#[test]
fn completions_render_bytes_refuses_when_labels_exceed_the_budget() {
    // A candidate set within the count cap whose rendered label+detail bytes exceed the
    // render budget refuses on the byte arm, not the count arm. MAX field names of a
    // length that overshoots the budget exercise the otherwise-uncovered
    // `CompletionRenderBytes` refusal.
    let cap = MAX_COMPLETION_CANDIDATES as usize;
    let name_width = (MAX_COMPLETION_RENDER_BYTES as usize / cap) + 4;
    match member_outcome(&member_source(cap, name_width)) {
        Ok(CompletionOutcome::Refused(AnalysisResourceLimit::CompletionRenderBytes { limit })) => {
            assert_eq!(limit, MAX_COMPLETION_RENDER_BYTES);
        }
        other => panic!("expected a render-byte refusal, got {}", describe(&other)),
    }
}

#[test]
fn completions_absent_in_literal() {
    let source = "module app\n\n\
        fn f(): int {\n    return 42\n}\n";
    let snapshot = snap(source);
    let offset = at(source, "return 42", "return 4".len());
    assert!(
        matches!(
            snapshot.completions(&identity("src/app.mw"), offset),
            Ok(CompletionOutcome::Ready(Fact::Absent))
        ),
        "a position inside a literal has no completion class",
    );
}

#[test]
fn completions_unknown_file() {
    let source = "module app\n\nfn f(): int {\n    return 1\n}\n";
    let snapshot = snap(source);
    assert!(matches!(
        snapshot.completions(&identity("src/other.mw"), 0),
        Err(QueryError::UnknownFile)
    ));
}

#[test]
fn completions_offset_out_of_range() {
    let source = "module app\n\nfn f(): int {\n    return 1\n}\n";
    let snapshot = snap(source);
    assert!(matches!(
        snapshot.completions(&identity("src/app.mw"), source.len() + 1),
        Err(QueryError::OffsetOutOfRange)
    ));
}

#[test]
fn completions_non_utf8_file_is_unavailable() {
    // A non-UTF-8 file never produced a parse tree, so it has no retained module and a
    // completion query in it is syntax-unavailable, never a fabricated empty set.
    let files = [("src/app.mw", vec![0x66, 0x6e, 0xff, 0xfe])];
    let Ok(snapshot) = analyze(Arc::new(project_bytes(&files)), InputRevision::new(1)) else {
        panic!("expected a snapshot");
    };
    assert!(matches!(
        snapshot.completions(&identity("src/app.mw"), 0),
        Ok(CompletionOutcome::Ready(Fact::Unavailable(
            Unavailability::Syntax
        )))
    ));
}
