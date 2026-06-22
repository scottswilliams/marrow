use crate::support;
use marrow_check::tooling::{
    SourceSavedRootCompletionCandidate, source_saved_root_completion_fact,
};

#[test]
fn source_saved_root_completion_lists_declared_roots_in_checked_order() {
    let (snapshot, _paths) = support::analyze_overlay(
        "source-saved-root-completion-facts",
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

resource Book
    required title: string

;; Books saved by id.
store ^books(id: int): Book
",
            ),
            (
                "src/shelf/app.mw",
                "\
module shelf::app

resource Setting
    required name: string

;; Settings singleton.
store ^settings: Setting

resource Draft
    required title: string

;; Drafts saved by id.
store ^drafts(id: int): Draft
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);

    let fact = source_saved_root_completion_fact(&snapshot.program);

    assert_eq!(
        fact.candidates,
        vec![
            SourceSavedRootCompletionCandidate {
                root: "settings".into(),
                module: "shelf::app".into(),
                resource_name: "Setting".into(),
                docs: vec!["Settings singleton.".into()],
            },
            SourceSavedRootCompletionCandidate {
                root: "drafts".into(),
                module: "shelf::app".into(),
                resource_name: "Draft".into(),
                docs: vec!["Drafts saved by id.".into()],
            },
            SourceSavedRootCompletionCandidate {
                root: "books".into(),
                module: "shelf::books".into(),
                resource_name: "Book".into(),
                docs: vec!["Books saved by id.".into()],
            },
        ]
    );
}
