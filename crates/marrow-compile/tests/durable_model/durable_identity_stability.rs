//! The durable identity anchors a project mints, frozen.
//!
//! A durable declaration's `(kind, path)` anchors are the keys of the machine-written
//! `.marrow/ids` ledger, and a store keeps its data under the id its anchor resolved to.
//! A changed anchor spelling reads as a missing identity, not a rename: the mint action
//! commits a fresh id beside the one the old spelling still owns, stranding the data under
//! it and breaking rename-preserves-identity
//! (`docs/language/traversal-and-indexes.md`).
//!
//! The anchor set is therefore a durable contract, so this suite compares the WHOLE set for
//! a corpus reaching every `IdentityKind` rather than asserting convenient individual
//! anchors. The frozen list is the observed output of resolving that corpus's gaps to
//! convergence through the production `compile` entry point, so a change in how a path is
//! assembled fails here at the byte.

use std::sync::LazyLock;

use marrow_compile::compile;
use marrow_project::{IdentityAnchor, IdentityKind, ProjectInput};

use super::ids;
use super::project_capture::project_with_ids;

/// A corpus minting an anchor of every `IdentityKind` the durable builder resolves.
///
/// Split across two modules so a coordinate taken from the wrong module cannot pass by
/// there being only one.
fn corpus(ids: Option<&[u8]>) -> ProjectInput {
    project_with_ids(
        &[
            (
                "src/main.mw",
                r#"module main

enum Binding {
    Hard
    Soft
}

resource Book {
    required title: string
    shelf: string
    isbn: string
    binding: Binding

    details {
        pages: int
        language: string
    }

    notes[noteId: string] {
        required text: string
        seq: int

        replies[replyId: int] {
            body: string
        }
    }
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

store ^archive[id: int]: Book

pub fn label(): string {
    return "books"
}
"#,
            ),
            (
                "src/enroll.mw",
                r#"module enroll

resource Enrollment {
    required grade: string
}

store ^enrollments[student: int, course: string]: Enrollment

pub fn subject(): string {
    return "enrollments"
}
"#,
            ),
        ],
        ids,
    )
}

/// Every anchor the corpus mints, in the ledger's canonical order, as `"<kind> <path>"`.
const FROZEN_ANCHORS: &[&str] = &[
    "application .",
    "product Book",
    "product Enrollment",
    "field Book.binding",
    "field Book.details.language",
    "field Book.details.pages",
    "field Book.isbn",
    "field Book.notes.replies.body",
    "field Book.notes.seq",
    "field Book.notes.text",
    "field Book.shelf",
    "field Book.title",
    "field Enrollment.grade",
    "root Book.notes",
    "root Book.notes.replies",
    "root archive",
    "root books",
    "root enrollments",
    "key Book.notes.noteId",
    "key Book.notes.replies.replyId",
    "key archive.id",
    "key books.id",
    "key enrollments.course",
    "key enrollments.student",
    "sum Binding",
    "member Binding.Hard",
    "member Binding.Soft",
    "group Book.details",
    "index books.byIsbn",
    "index books.byShelf",
];

/// One convergence for the whole suite: the gap-resolution loop runs once and every test
/// consumes the settled artifacts.
static CONVERGED: LazyLock<(Vec<IdentityAnchor>, Vec<u8>)> =
    LazyLock::new(|| ids::converged(corpus));

#[test]
fn the_minted_identity_anchor_set_is_frozen() {
    let observed: Vec<String> = CONVERGED
        .0
        .iter()
        .map(|anchor| format!("{} {}", anchor.kind.keyword(), anchor.path))
        .collect();
    let frozen: Vec<String> = FROZEN_ANCHORS.iter().map(|line| line.to_string()).collect();
    assert_eq!(
        observed, frozen,
        "the durable identity anchor set changed; a store keyed by the old \
         spellings would silently lose its data",
    );
}

/// A builder that refused halfway would mint a strict subset, and the comparison above
/// would freeze that subset instead.
#[test]
fn the_corpus_compiles_once_its_anchors_are_minted() {
    let project = corpus(Some(&CONVERGED.1));
    compile(&project)
        .unwrap_or_else(|failure| panic!("the identity corpus must compile: {failure:#?}"));
}

/// Without this, an edit dropping a whole family from the corpus would be absorbed by
/// editing the frozen list to agree.
#[test]
fn the_corpus_reaches_every_identity_kind() {
    for kind in IdentityKind::ALL {
        assert!(
            CONVERGED.0.iter().any(|anchor| anchor.kind == *kind),
            "the corpus mints no `{}` anchor",
            kind.keyword(),
        );
    }
}
