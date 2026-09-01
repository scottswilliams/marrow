//! The durable identity anchors a project mints, frozen.
//!
//! A durable declaration's `(kind, path)` anchors are the keys of the
//! machine-written `.marrow/ids` ledger: the compiler resolves each one to a
//! stable id, and a store keeps its data under the id that anchor resolved to.
//! A changed anchor spelling therefore does not report anything — it silently
//! re-anchors durable identity, so every existing store's data hangs off an id
//! nothing asks for any more, and the rename-preserves-identity law
//! (`docs/language/traversal-and-indexes.md`) is broken with no diagnostic.
//!
//! That makes the anchor set a durable contract rather than a diagnostic
//! surface, and it is why this suite compares the WHOLE set for a corpus that
//! reaches every anchor-minting site, instead of asserting individual anchors
//! where they are convenient. The corpus below covers each `IdentityKind` a
//! store declaration can mint: the application anchor, root placements
//! (single-column, composite, and a second root over one resource), the product,
//! per-key-column anchors at root and branch layers, stored fields at top level,
//! group-qualified, branch-qualified, and nested-branch-qualified, group
//! namespaces, managed indexes, and an enum's sum and member anchors.
//!
//! The frozen list is not a restatement of the compiler's spelling rule — it is
//! the observed output of resolving the corpus's gaps to convergence through the
//! production `compile` entry point, so a conversion that changes how a path is
//! assembled fails here at the byte.

#[path = "common/ids.rs"]
mod ids;
#[path = "common/project.rs"]
mod project_capture;

use ids::{minted, minted_anchors};
use marrow_compile::compile;
use marrow_project::{IdentityAnchor, IdentityKind, ProjectInput};
use project_capture::project_with_ids;

/// A corpus reaching every anchor-minting site in the durable builder.
///
/// Split across two modules so a coordinate the builder took from the wrong
/// module cannot pass by there being only one.
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

/// Every anchor the corpus mints, in the ledger's canonical order, spelled
/// `"<kind> <path>"`.
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

#[test]
fn the_minted_identity_anchor_set_is_frozen() {
    let observed: Vec<String> = minted_anchors(corpus)
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

/// The frozen set is the set of an ADMITTED program, not of a corpus that
/// stopped early: a builder that refused halfway would mint a strict subset and
/// the comparison above would then freeze that subset.
#[test]
fn the_corpus_compiles_once_its_anchors_are_minted() {
    let ledger = minted(corpus);
    compile(&ledger)
        .unwrap_or_else(|failure| panic!("the identity corpus must compile: {failure:#?}"));
}

/// The corpus reaches every kind a store declaration can mint. Without this a
/// later edit could drop a whole family from the corpus and the frozen list
/// would be edited to agree, with nothing noticing.
#[test]
fn the_corpus_reaches_every_identity_kind() {
    let observed: Vec<IdentityAnchor> = minted_anchors(corpus);
    for kind in IdentityKind::ALL {
        assert!(
            observed.iter().any(|anchor| anchor.kind == *kind),
            "the corpus mints no `{}` anchor",
            kind.keyword(),
        );
    }
}
