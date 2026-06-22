use std::path::Path;

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    SourceTypeBuiltin, SourceTypeCompletionCandidate, SourceTypeCompletionFact,
    source_type_completion_fact,
};
use marrow_syntax::SourceFile;

fn analyze_project() -> (AnalysisSnapshot, Vec<std::path::PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(
        "source-type-completion-facts",
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

;; Book resource docs.
resource Book
    required title: string

;; Books saved by id.
store ^books(id: int): Book

;; Public lifecycle state.
pub enum Status
    active

;; Private shelf state.
enum Secret
    hidden

pub enum Shared
    shelf
",
            ),
            (
                "src/archive/books.mw",
                "\
module archive::books

resource ArchiveBook
    required title: string

pub enum Shared
    archive
",
            ),
            (
                "src/shelf/app.mw",
                "\
module shelf::app

use shelf::books

;; Draft resource docs.
resource Draft
    required title: string

store ^drafts(id: int): Draft

store ^settings: Draft

;; Local private state.
enum LocalSecret
    draft
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);
    (snapshot, paths)
}

fn source_file<'a>(snapshot: &'a AnalysisSnapshot, file: &Path) -> &'a SourceFile {
    &snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)
        .expect("analyzed file")
        .parsed
        .file
}

fn app_fact() -> SourceTypeCompletionFact {
    let (snapshot, paths) = analyze_project();
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");
    source_type_completion_fact(&snapshot.program, app, source_file(&snapshot, app))
}

fn shadowed_alias_fact() -> SourceTypeCompletionFact {
    let (snapshot, paths) = support::analyze_overlay(
        "source-type-completion-shadowed-import",
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

resource Book
    required title: string

pub enum Status
    active
",
            ),
            (
                "src/shelf/app.mw",
                "\
module shelf::app

use shelf::books

resource books
    required value: string
",
            ),
        ],
    );
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");
    source_type_completion_fact(&snapshot.program, app, source_file(&snapshot, app))
}

fn builtin_order(fact: &SourceTypeCompletionFact) -> Vec<SourceTypeBuiltin> {
    fact.candidates
        .iter()
        .filter_map(|candidate| match candidate {
            SourceTypeCompletionCandidate::Builtin { spelling } => Some(*spelling),
            _ => None,
        })
        .collect()
}

fn has_resource_path(fact: &SourceTypeCompletionFact, path: &[&str]) -> bool {
    fact.candidates.iter().any(|candidate| {
        matches!(
            candidate,
            SourceTypeCompletionCandidate::Resource { path: candidate_path, .. }
                if path_matches(candidate_path, path)
        )
    })
}

fn has_enum_path(fact: &SourceTypeCompletionFact, path: &[&str]) -> bool {
    fact.candidates.iter().any(|candidate| {
        matches!(
            candidate,
            SourceTypeCompletionCandidate::Enum { path: candidate_path, .. }
                if path_matches(candidate_path, path)
        )
    })
}

fn path_matches(candidate: &[String], expected: &[&str]) -> bool {
    candidate
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied())
}

fn has_store_identity(fact: &SourceTypeCompletionFact, root: &str) -> bool {
    fact.candidates.iter().any(|candidate| {
        matches!(
            candidate,
            SourceTypeCompletionCandidate::StoreIdentity { root: candidate_root, .. }
                if candidate_root == root
        )
    })
}

#[test]
fn source_type_completion_lists_builtins_first() {
    let fact = app_fact();
    assert_eq!(
        builtin_order(&fact),
        [
            SourceTypeBuiltin::Int,
            SourceTypeBuiltin::Decimal,
            SourceTypeBuiltin::Bool,
            SourceTypeBuiltin::String,
            SourceTypeBuiltin::Bytes,
            SourceTypeBuiltin::Date,
            SourceTypeBuiltin::Instant,
            SourceTypeBuiltin::Duration,
            SourceTypeBuiltin::ErrorCode,
            SourceTypeBuiltin::Sequence,
            SourceTypeBuiltin::Unknown,
            SourceTypeBuiltin::Error,
        ]
    );
}

#[test]
fn source_type_completion_uses_valid_resource_spellings_for_file() {
    let fact = app_fact();

    assert!(has_resource_path(&fact, &["Draft"]));
    assert!(has_resource_path(&fact, &["books", "Book"]));
    assert!(
        !has_resource_path(&fact, &["Book"]),
        "foreign resources must not be offered as bare type annotations"
    );
    assert!(
        !has_resource_path(&fact, &["ArchiveBook"]),
        "unimported foreign resources must not be offered as bare type annotations"
    );
}

#[test]
fn source_type_completion_excludes_shadowed_import_resource_spellings() {
    let fact = shadowed_alias_fact();

    assert!(has_resource_path(&fact, &["books"]));
    assert!(
        !has_resource_path(&fact, &["books", "Book"]),
        "import aliases shadowed by a top-level declaration must not produce resource candidates"
    );
}

#[test]
fn source_type_completion_uses_enum_visibility_for_bare_candidates() {
    let fact = app_fact();

    assert!(has_enum_path(&fact, &["LocalSecret"]));
    assert!(has_enum_path(&fact, &["Status"]));
    assert!(has_enum_path(&fact, &["books", "Status"]));
    assert!(has_enum_path(&fact, &["books", "Shared"]));
    assert!(
        !has_enum_path(&fact, &["Secret"]),
        "private foreign enums must not be offered as bare type annotations"
    );
    assert!(
        !has_enum_path(&fact, &["books", "Secret"]),
        "private foreign enums must not be offered through import aliases"
    );
    assert!(
        !has_enum_path(&fact, &["Shared"]),
        "duplicate public foreign enum names must not collapse to one bare candidate"
    );
}

#[test]
fn source_type_completion_excludes_shadowed_import_enum_spellings() {
    let fact = shadowed_alias_fact();

    assert!(
        !has_enum_path(&fact, &["books", "Status"]),
        "import aliases shadowed by a top-level declaration must not produce enum candidates"
    );
}

#[test]
fn source_type_completion_lists_only_keyed_store_identities() {
    let fact = app_fact();

    assert!(has_store_identity(&fact, "books"));
    assert!(has_store_identity(&fact, "drafts"));
    assert!(
        !has_store_identity(&fact, "settings"),
        "keyless singleton stores must not produce identity type candidates"
    );
}
