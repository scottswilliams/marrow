use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{
    SourceEnumHoverFact, SourceEnumMemberHoverFact, SourceEnumMemberStatus,
    SourceEnumMemberSummary, SourceResourceHoverFact, SourceResourceHoverMember,
    SourceResourceHoverMemberKind, SourceResourceHoverPathSegment, SourceSchemaHoverFact,
    SourceSchemaHoverKeyParam, source_schema_hover_fact_at,
};
use marrow_check::{AnalysisSnapshot, BindingIndex, build_binding_index};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, BindingIndex, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths[0].clone())
}

fn analyze_files(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn analyze_files_with_report(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SourceSchemaHoverFact> {
    source_schema_hover_fact_at(snapshot, index, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn resource_fact() -> SourceSchemaHoverFact {
    SourceSchemaHoverFact::Resource(SourceResourceHoverFact {
        name: "Book".to_string(),
        docs: vec!["Book records.".to_string()],
        members: vec![
            SourceResourceHoverMember {
                path: vec![SourceResourceHoverPathSegment {
                    name: "title".to_string(),
                    key_params: Vec::new(),
                }],
                kind: SourceResourceHoverMemberKind::Field {
                    required: true,
                    ty: "string".to_string(),
                },
            },
            SourceResourceHoverMember {
                path: vec![SourceResourceHoverPathSegment {
                    name: "notes".to_string(),
                    key_params: vec![SourceSchemaHoverKeyParam {
                        name: "noteId".to_string(),
                        ty: "string".to_string(),
                    }],
                }],
                kind: SourceResourceHoverMemberKind::Layer,
            },
            SourceResourceHoverMember {
                path: vec![
                    SourceResourceHoverPathSegment {
                        name: "notes".to_string(),
                        key_params: vec![SourceSchemaHoverKeyParam {
                            name: "noteId".to_string(),
                            ty: "string".to_string(),
                        }],
                    },
                    SourceResourceHoverPathSegment {
                        name: "text".to_string(),
                        key_params: Vec::new(),
                    },
                ],
                kind: SourceResourceHoverMemberKind::Field {
                    required: false,
                    ty: "string".to_string(),
                },
            },
        ],
    })
}

fn status_enum_fact() -> SourceSchemaHoverFact {
    SourceSchemaHoverFact::Enum(SourceEnumHoverFact {
        name: "Status".to_string(),
        docs: vec!["Lifecycle state.".to_string()],
        members: vec![
            SourceEnumMemberSummary {
                path: vec!["active".to_string()],
                status: SourceEnumMemberStatus::Category,
            },
            SourceEnumMemberSummary {
                path: vec!["active".to_string(), "open".to_string()],
                status: SourceEnumMemberStatus::Selectable,
            },
            SourceEnumMemberSummary {
                path: vec!["closed".to_string()],
                status: SourceEnumMemberStatus::Selectable,
            },
        ],
    })
}

#[test]
fn source_schema_hover_fact_covers_resource_declaration_constructor_and_type_leaf() {
    let source = "\
module a

;; Book records.
resource Book
    required title: string
    notes(noteId: string)
        text: string

pub fn make(book: Book): Book
    return Book(title: \"Dune\")
";
    let (snapshot, index, file) = analyze("source-schema-hover-resource", source);

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "resource Book")),
        None
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "resource Book") + "resource ".len()
        ),
        Some(resource_fact())
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "book: Book") + "book: ".len()
        ),
        Some(resource_fact())
    );
    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "Book(title")),
        Some(resource_fact())
    );
}

#[test]
fn source_schema_hover_fact_covers_qualified_resource_leaf_only() {
    let state = "\
module shelf::state

;; Book records.
resource Book
    required title: string
    notes(noteId: string)
        text: string
";
    let app = "\
module shelf::app

use shelf::state

pub fn make(book: state::Book): state::Book
    return state::Book(title: \"Dune\")
";
    let (snapshot, index, paths) = analyze_files(
        "source-schema-hover-qualified-resource",
        &[("src/shelf/state.mw", state), ("src/shelf/app.mw", app)],
    );
    let app_file = &paths[1];

    for needle in ["book: state::Book", "return state::Book"] {
        let qualifier = offset(app, needle) + needle.find("state").unwrap();
        let leaf = qualifier + "state::".len();

        assert_eq!(fact_at(&snapshot, &index, app_file, qualifier + 1), None);
        assert_eq!(
            fact_at(&snapshot, &index, app_file, leaf + 1),
            Some(resource_fact())
        );
    }
}

#[test]
fn source_schema_hover_fact_covers_enum_declaration_annotation_and_member_leaf() {
    let source = "\
module a

;; Lifecycle state.
enum Status
    category active
        ;; Open for edits.
        open
    closed

pub fn current(status: Status): Status
    return Status::active::open
";
    let (snapshot, index, file) = analyze("source-schema-hover-enum", source);

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "enum Status")),
        None
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "enum Status") + "enum ".len()
        ),
        Some(status_enum_fact())
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "status: Status") + "status: ".len()
        ),
        Some(status_enum_fact())
    );
    assert_eq!(
        fact_at(&snapshot, &index, &file, offset(source, "category active")),
        None
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "category active") + "category ".len()
        ),
        Some(SourceSchemaHoverFact::EnumMember(
            SourceEnumMemberHoverFact {
                enum_name: "Status".to_string(),
                path: vec!["active".to_string()],
                docs: Vec::new(),
                status: SourceEnumMemberStatus::Category,
            }
        ))
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            offset(source, "        open") + "        ".len()
        ),
        Some(SourceSchemaHoverFact::EnumMember(
            SourceEnumMemberHoverFact {
                enum_name: "Status".to_string(),
                path: vec!["active".to_string(), "open".to_string()],
                docs: vec!["Open for edits.".to_string()],
                status: SourceEnumMemberStatus::Selectable,
            }
        ))
    );
    assert_eq!(
        fact_at(
            &snapshot,
            &index,
            &file,
            source.rfind("Status::active::open").unwrap() + "Status::active::".len()
        ),
        Some(SourceSchemaHoverFact::EnumMember(
            SourceEnumMemberHoverFact {
                enum_name: "Status".to_string(),
                path: vec!["active".to_string(), "open".to_string()],
                docs: vec!["Open for edits.".to_string()],
                status: SourceEnumMemberStatus::Selectable,
            }
        ))
    );
}

#[test]
fn source_schema_hover_fact_preserves_enum_annotation_visibility() {
    let first = "\
module shelf::first

;; First lifecycle state.
pub enum Status
    open
";
    let second = "\
module shelf::second

;; Second lifecycle state.
pub enum Status
    closed
";
    let app = "\
module shelf::app

pub fn set(status: Status)
    return
";
    let (snapshot, index, paths) = analyze_files_with_report(
        "source-schema-hover-ambiguous-enum",
        &[
            ("src/shelf/first.mw", first),
            ("src/shelf/second.mw", second),
            ("src/shelf/app.mw", app),
        ],
    );
    let app_file = &paths[2];
    let annotation = offset(app, "status: Status") + "status: ".len();
    assert_eq!(fact_at(&snapshot, &index, app_file, annotation), None);

    let private_state = "\
module shelf::state

;; Private lifecycle state.
enum Status
    open
";
    let private_app = "\
module shelf::app

use shelf::state

pub fn set(status: state::Status)
    return
";
    let (snapshot, index, paths) = analyze_files_with_report(
        "source-schema-hover-private-enum",
        &[
            ("src/shelf/state.mw", private_state),
            ("src/shelf/app.mw", private_app),
        ],
    );
    let app_file = &paths[1];
    let qualifier = offset(private_app, "state::Status");
    let leaf = qualifier + "state::".len();

    assert_eq!(fact_at(&snapshot, &index, app_file, qualifier + 1), None);
    assert_eq!(fact_at(&snapshot, &index, app_file, leaf + 1), None);
}
