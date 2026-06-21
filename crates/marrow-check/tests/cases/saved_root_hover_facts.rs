use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    SavedPlaceHoverKeyParam, StoreRootHoverFact, StoreRootHoverMember, StoreRootHoverPathSegment,
    store_root_hover_fact_at,
};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    (snapshot, paths[0].clone())
}

fn analyze_files(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    (snapshot, paths)
}

fn fact_at(snapshot: &AnalysisSnapshot, file: &Path, offset: usize) -> Option<StoreRootHoverFact> {
    store_root_hover_fact_at(snapshot, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn last_offset(source: &str, needle: &str) -> usize {
    source.rfind(needle).expect("needle is present")
}

#[test]
fn saved_root_hover_fact_covers_declaration_caret_and_name() {
    let source = "\
module a

resource Book
    ;; Display title.
    required title: string
    ;; Notes by label.
    notes(noteId: string)
        text: string

;; Books saved by id.
store ^books(id: int): Book
    index byTitle(title, id) unique

pub fn title(id: Id(^books)): string
    return ^books(id).title ?? \"\"
";
    let (snapshot, file) = analyze("saved-root-hover-keyed", source);
    let expected = StoreRootHoverFact {
        root: "books".to_string(),
        identity_keys: vec![SavedPlaceHoverKeyParam {
            name: "id".to_string(),
            ty: "int".to_string(),
        }],
        resource: "Book".to_string(),
        store_docs: vec!["Books saved by id.".to_string()],
        members: vec![
            StoreRootHoverMember::Field {
                path: vec![StoreRootHoverPathSegment {
                    name: "title".to_string(),
                    key_params: Vec::new(),
                }],
                required: true,
                ty: "string".to_string(),
            },
            StoreRootHoverMember::Layer {
                path: vec![StoreRootHoverPathSegment {
                    name: "notes".to_string(),
                    key_params: vec![SavedPlaceHoverKeyParam {
                        name: "noteId".to_string(),
                        ty: "string".to_string(),
                    }],
                }],
            },
            StoreRootHoverMember::Field {
                path: vec![
                    StoreRootHoverPathSegment {
                        name: "notes".to_string(),
                        key_params: vec![SavedPlaceHoverKeyParam {
                            name: "noteId".to_string(),
                            ty: "string".to_string(),
                        }],
                    },
                    StoreRootHoverPathSegment {
                        name: "text".to_string(),
                        key_params: Vec::new(),
                    },
                ],
                required: false,
                ty: "string".to_string(),
            },
            StoreRootHoverMember::Index {
                name: "byTitle".to_string(),
                args: vec!["title".to_string(), "id".to_string()],
                unique: true,
            },
        ],
    };
    let declaration_caret = offset(source, "^books");
    let declaration_name = declaration_caret + 1;

    for offset in [declaration_caret, declaration_name, declaration_name + 2] {
        assert_eq!(fact_at(&snapshot, &file, offset), Some(expected.clone()));
    }
}

#[test]
fn saved_root_hover_fact_excludes_keyed_root_token_end() {
    let source = "\
module a

resource Book
    title: string

store ^books(id: int): Book
";
    let (snapshot, file) = analyze("saved-root-hover-keyed-boundary", source);
    let offset = offset(source, "^books") + "^books".len();
    assert_eq!(source.as_bytes()[offset], b'(');

    assert_eq!(fact_at(&snapshot, &file, offset), None);
}

#[test]
fn saved_root_hover_fact_excludes_keyless_root_token_end() {
    let source = "\
module a

resource Settings
    enabled: bool

store ^settings: Settings
";
    let (snapshot, file) = analyze("saved-root-hover-keyless-boundary", source);
    let offset = offset(source, "^settings") + "^settings".len();
    assert_eq!(source.as_bytes()[offset], b':');

    assert_eq!(fact_at(&snapshot, &file, offset), None);
}

#[test]
fn saved_root_hover_fact_covers_keyless_root() {
    let source = "\
module a

resource Settings
    enabled: bool

store ^settings: Settings
";
    let (snapshot, file) = analyze("saved-root-hover-keyless", source);
    let offset = offset(source, "^settings") + 1;

    assert_eq!(
        fact_at(&snapshot, &file, offset),
        Some(StoreRootHoverFact {
            root: "settings".to_string(),
            identity_keys: Vec::new(),
            resource: "Settings".to_string(),
            store_docs: Vec::new(),
            members: vec![StoreRootHoverMember::Field {
                path: vec![StoreRootHoverPathSegment {
                    name: "enabled".to_string(),
                    key_params: Vec::new(),
                }],
                required: false,
                ty: "bool".to_string(),
            }],
        })
    );
}

#[test]
fn saved_root_hover_fact_uses_the_declaration_file_when_names_match() {
    let first = "\
module first

resource books
    firstTitle: string

store ^items(id: int): books
";
    let second = "\
module second

resource books
    subtitle: string

;; Second module root.
store ^books: books
";
    let (snapshot, paths) = analyze_files(
        "saved-root-hover-cross-file-same-name",
        &[("src/first.mw", first), ("src/second.mw", second)],
    );
    let second_file = &paths[1];
    let offset = offset(second, "^books") + 1;

    assert_eq!(
        fact_at(&snapshot, second_file, offset),
        Some(StoreRootHoverFact {
            root: "books".to_string(),
            identity_keys: Vec::new(),
            resource: "books".to_string(),
            store_docs: vec!["Second module root.".to_string()],
            members: vec![StoreRootHoverMember::Field {
                path: vec![StoreRootHoverPathSegment {
                    name: "subtitle".to_string(),
                    key_params: Vec::new(),
                }],
                required: false,
                ty: "string".to_string(),
            }],
        })
    );
}

#[test]
fn saved_root_hover_fact_returns_none_for_saved_root_use() {
    let source = "\
module a

resource Book
    title: string

store ^books(id: int): Book

pub fn delete_book(id: Id(^books))
    delete ^books(id)
";
    let (snapshot, file) = analyze("saved-root-hover-use-none", source);
    let use_caret = last_offset(source, "^books");
    let use_name = use_caret + 1;

    assert_eq!(fact_at(&snapshot, &file, use_caret), None);
    assert_eq!(fact_at(&snapshot, &file, use_name), None);
}
