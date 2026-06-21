use std::path::Path;

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    SourceNamespaceCompletionFact, SourceNamespaceEnumMemberStatus,
    source_namespace_completion_fact,
};
use marrow_syntax::SourceFile;

fn analyze_project(name: &str) -> (AnalysisSnapshot, Vec<std::path::PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(
        name,
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

resource Book
    ;; Display title.
    required title: string

;; Public lifecycle state.
pub enum Status
    ;; Ready for use.
    active
    ;; No longer active.
    archived

enum Secret
    hidden

;; Returns a book title.
pub fn titleOf(id: int): string
    return \"title\"

fn privateTitle(): string
    return \"hidden\"

const LIMIT: int = 100
",
            ),
            (
                "src/shelf/app.mw",
                "\
module shelf::app

use shelf::books

resource Draft
    required title: string

enum LocalStatus
    draft

fn privateDraft(): string
    return \"draft\"
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

fn namespace_fact(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    qualifier: &[&str],
) -> Option<SourceNamespaceCompletionFact> {
    let qualifier = qualifier
        .iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    source_namespace_completion_fact(
        &snapshot.program,
        file,
        source_file(snapshot, file),
        &qualifier,
    )
}

#[test]
fn namespace_completion_resolves_used_module_alias_to_visible_members() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-used-module");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Module(fact)) =
        namespace_fact(&snapshot, app, &["books"])
    else {
        panic!("expected module namespace fact");
    };

    assert_eq!(fact.module, "shelf::books");
    assert_eq!(
        fact.resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect::<Vec<_>>(),
        ["Book"]
    );
    assert_eq!(
        fact.enums
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["Status"]
    );
    assert_eq!(
        fact.functions
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["titleOf"]
    );
}

#[test]
fn namespace_completion_keeps_same_module_private_members_visible() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-same-module");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Module(fact)) =
        namespace_fact(&snapshot, app, &["shelf", "app"])
    else {
        panic!("expected same-module namespace fact");
    };

    assert_eq!(fact.module, "shelf::app");
    assert_eq!(
        fact.resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect::<Vec<_>>(),
        ["Draft"]
    );
    assert_eq!(
        fact.enums
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["LocalStatus"]
    );
    assert_eq!(
        fact.functions
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["privateDraft"]
    );
}

#[test]
fn namespace_completion_returns_enum_members_with_docs_and_status() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-enum-members");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Enum(fact)) =
        namespace_fact(&snapshot, app, &["books", "Status"])
    else {
        panic!("expected enum namespace fact");
    };

    assert_eq!(fact.enum_name, "Status");
    assert_eq!(
        fact.members
            .iter()
            .map(|member| (member.name.as_str(), member.docs.as_slice(), member.status,))
            .collect::<Vec<_>>(),
        [
            (
                "active",
                ["Ready for use.".to_string()].as_slice(),
                SourceNamespaceEnumMemberStatus::Selectable,
            ),
            (
                "archived",
                ["No longer active.".to_string()].as_slice(),
                SourceNamespaceEnumMemberStatus::Selectable,
            ),
        ]
    );
}

#[test]
fn namespace_completion_fails_closed_for_resources_and_module_prefixes() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-fail-closed");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    assert_eq!(namespace_fact(&snapshot, app, &["books", "Book"]), None);
    assert_eq!(
        namespace_fact(&snapshot, app, &["shelf", "books", "Book"]),
        None
    );
    assert_eq!(namespace_fact(&snapshot, app, &["shelf"]), None);
}

#[test]
fn namespace_completion_fails_closed_for_ambiguous_single_segment_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-ambiguous-single-alias",
        &[
            (
                "src/shelf/books.mw",
                "module shelf::books\n\npub fn titleOf(): string\n    return \"title\"\n",
            ),
            (
                "src/archive/books.mw",
                "module archive::books\n\npub fn oldTitle(): string\n    return \"old\"\n",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use shelf::books
use archive::books
",
            ),
        ],
    );
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert_eq!(namespace_fact(&snapshot, app, &["books"]), None);
}

#[test]
fn namespace_completion_fails_closed_when_local_declaration_collides_with_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-local-alias-collision",
        &[
            (
                "src/shelf/books.mw",
                "module shelf::books\n\npub fn titleOf(): string\n    return \"title\"\n",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use shelf::books

resource books
    required title: string
",
            ),
        ],
    );
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert_eq!(namespace_fact(&snapshot, app, &["books"]), None);
}
