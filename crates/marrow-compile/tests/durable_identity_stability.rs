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

// ---------------------------------------------------------------------------
// Structural gates over the anchor-minting source itself.
//
// The frozen set above catches a changed anchor at the byte; these gates keep the
// source shape that makes such a change hard to write in the first place. They scan
// through the shared `source_projection` — the one owner of "what counts as code" —
// so a needle in a comment or string can never decide them.
// ---------------------------------------------------------------------------

#[path = "common/source_projection.rs"]
mod source_projection;

/// The production code of one `marrow-compile` source file, comments, strings, and
/// `#[cfg(test)]` items blanked by the shared projection.
fn production_code_of(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(file);
    source_projection::production_code(&std::fs::read_to_string(path).expect("read source file"))
}

/// The durable builder reads its declaration row tables, never index or key syntax,
/// and a key column's ledger anchor is assembled in exactly one place.
///
/// Every index admission rule once read the parsed `IndexDecl` and rendered each
/// argument's path spelling at the moment it needed one, which made "the same
/// component" a per-caller answer; the `IndexTable` renders each path and classifies
/// its reach once, when the row is taken, so a rule can only ask. The root's and a
/// branch's key tuples each used to spell their own `format!("{path}.{name}")` join,
/// and those anchors are the keys of the ledger this suite freezes: a divergence
/// between two spellings reports nothing and silently re-anchors committed durable
/// identity. The join counts are exact rather than "at most", because a reintroduced
/// inline join would leave `identity_path` behind at one site and pass a mere
/// absence check.
#[test]
fn the_durable_builder_reads_rows_and_joins_each_key_anchor_once() {
    let builder = production_code_of("durable.rs");
    for absent in [
        "IndexDecl",
        "IndexArg {",
        "field_path_spelling",
        "component.segments",
        "index.args",
        "KeyParam",
        "key_param",
    ] {
        assert!(
            !builder.contains(absent),
            "`{absent}` names declaration syntax; the durable builder reads row tables",
        );
    }
    assert!(
        builder.contains("IndexArgReach::ThroughMember"),
        "the nested-member rule must read the row's classified reach",
    );
    assert_eq!(
        builder.matches("IdentityKind::Key,").count(),
        2,
        "exactly two sites mint a key anchor: the store root's tuple and a branch's",
    );
    assert_eq!(
        builder.matches("identity_path(").count(),
        2,
        "both key-anchor sites must read the one join, and nothing else may",
    );
    let rows = production_code_of("durable/rows.rs");
    assert!(
        rows.contains("fn take(indexes: &'a [IndexDecl])") && rows.contains("field_path_spelling"),
        "the index row table must be the one reader of index syntax",
    );
    assert!(
        rows.contains("fn identity_path(") && rows.contains("fn over_wide("),
        "the key row table must own the anchor join and the width cap",
    );
}

/// The staged producer boundary itself carries no raw declaration slice.
///
/// `StagedStoreTxn::build_one` and the `durable.rs::build_one` it forwards to once
/// took `&[(FileRef, FileIdentity, &ResourceDecl)]` and recovered the resource
/// declaration by name search after row construction. The crate-wide absence gate
/// forbids the search; this gate forbids the carrier: the raw slice type appears
/// exactly once in the durable builder — the `build` entry, where it is
/// row-construction *input* handed to nothing but the row tables' `take` — and never
/// inside the staging wrapper, while both `build_one`s read the typed projection.
#[test]
fn the_staged_store_producer_accepts_no_raw_declaration_slice() {
    let builder = production_code_of("durable.rs");
    let staging = production_code_of("durable/staging.rs");
    let raw = "&[(FileRef, FileIdentity, &ResourceDecl)]";
    assert!(
        !staging.contains(raw),
        "the staging wrapper runs after row construction and may carry no raw \
         declaration slice",
    );
    assert_eq!(
        builder.matches(raw).count(),
        1,
        "only the durable build entry may carry the raw declaration slice, as \
         row-construction input",
    );
    assert_eq!(
        builder.matches("resources").count(),
        2,
        "the raw slice is read by exactly two production sites: the `build` signature \
         that receives it and the `ResourceDirectory::take` call that turns it into rows",
    );
    assert!(
        builder.contains("ResourceDirectory::take(resources, records)"),
        "the one consumer of the raw slice must be row construction itself",
    );
    for (file, code) in [("durable.rs", &builder), ("durable/staging.rs", &staging)] {
        assert_eq!(
            code.matches("directory: &ResourceDirectory<'_>").count(),
            1,
            "`{file}` must pass its build_one the typed projection",
        );
    }
}
