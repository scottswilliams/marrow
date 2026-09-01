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

use std::sync::LazyLock;

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

/// One convergence for the whole suite: three tests read one corpus, so the
/// gap-resolution loop runs once and each test consumes the settled artifacts.
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

/// The frozen set is the set of an ADMITTED program, not of a corpus that
/// stopped early: a builder that refused halfway would mint a strict subset and
/// the comparison above would then freeze that subset.
#[test]
fn the_corpus_compiles_once_its_anchors_are_minted() {
    let project = corpus(Some(&CONVERGED.1));
    compile(&project)
        .unwrap_or_else(|failure| panic!("the identity corpus must compile: {failure:#?}"));
}

/// The corpus reaches every kind a store declaration can mint. Without this a
/// later edit could drop a whole family from the corpus and the frozen list
/// would be edited to agree, with nothing noticing.
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

use source_projection::production_code_of;

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

/// Every row table holds exactly its typed fields — pinned line by line, because the
/// round-1 review constructed a bridge the lexical needles missed: a type alias for
/// the raw declaration slice, carried as an extra directory field and consumed by a
/// `find_map` name recovery, kept every asserted count intact. A field-exact pin has
/// no such gap: any carrier added to a row table changes the pinned field list,
/// whatever its type is spelled as.
#[test]
fn the_row_tables_hold_exactly_their_typed_fields() {
    let rows = production_code_of("durable/rows.rs");
    let field_lines = |name: &str| -> Vec<String> {
        let header = format!("pub(super) struct {name} {{");
        let body = rows
            .split_once(&header)
            .unwrap_or_else(|| panic!("`{name}` is declared in the row tables"))
            .1;
        let body = body.split_once("\n}").expect("the struct body is closed").0;
        body.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    };
    let pins: [(&str, &[&str]); 5] = [
        (
            "ResourceDirectory<'a>",
            &[
                "rows: Vec<ResourceRow<'a>>,",
                "by_spelling: BTreeMap<&'a str, ResourceDeclId>,",
            ],
        ),
        (
            "ResourceRow<'a>",
            &[
                "pub(super) record: &'a RecordInfo,",
                "pub(super) groups: Vec<GroupRow<'a>>,",
            ],
        ),
        (
            "StoreRow<'a>",
            &[
                "pub(super) resource: &'a str,",
                "pub(super) binding: StoreResourceBinding,",
                "pub(super) indexes: IndexTable<'a>,",
                "pub(super) keys: KeyTable<'a>,",
            ],
        ),
        (
            "GroupRow<'a>",
            &[
                "pub(super) group: &'a GroupDecl,",
                "pub(super) path: String,",
                "pub(super) keys: Option<BranchKeyRows<'a>>,",
                "pub(super) groups: Vec<GroupRow<'a>>,",
            ],
        ),
        (
            "BranchKeyRows<'a>",
            &[
                "pub(super) table: KeyTable<'a>,",
                "pub(super) scalars: Result<Vec<ScalarType>, Box<SourceDiagnostic>>,",
            ],
        ),
    ];
    for (name, fields) in pins {
        assert_eq!(
            field_lines(name),
            fields,
            "`{name}` grew or changed a field; a new carrier is a lease-and-review event",
        );
    }
}

/// Declaration-by-name recovery has no shape left to hide in: the durable module's
/// `.find` population is a closed census of within-declaration member lookups, and
/// the recovery combinators are absent entirely.
///
/// The counts are exact rather than "at most" so a recovery rewritten onto an
/// allowed combinator moves a number instead of slipping past a needle. Every
/// counted site compares members of ONE declaration's own row — never one
/// declaration list against another's name.
#[test]
fn declaration_name_recovery_has_no_shape_to_hide_in() {
    let builder = production_code_of("durable.rs");
    let rows = production_code_of("durable/rows.rs");
    let staging = production_code_of("durable/staging.rs");
    assert_eq!(
        builder.matches(".find(").count(),
        11,
        "the builder's find census moved: nine member helpers and two index-component \
         lookups, each within one declaration's own members",
    );
    assert_eq!(
        rows.matches(".find(").count() + staging.matches(".find(").count(),
        0,
        "the row tables and the staging boundary perform no searches at all",
    );
    for (file, code) in [
        ("durable.rs", &builder),
        ("durable/rows.rs", &rows),
        ("durable/staging.rs", &staging),
    ] {
        for shape in ["find_map", ".position(", "unstable_name_collisions"] {
            assert!(
                !code.contains(shape),
                "`{shape}` in `{file}` is a recovery shape the census does not admit",
            );
        }
    }
    // The raw declaration type itself is a closed census: the build entry and the
    // row tables' take own every mention, so an alias cannot be minted from either
    // file without moving a count.
    let mentions = |code: &String| {
        code.matches("ResourceDecl").count() - code.matches("ResourceDeclId").count()
    };
    assert_eq!(
        mentions(&builder),
        2,
        "durable.rs names the raw declaration type only at its import and build entry",
    );
    assert_eq!(
        mentions(&rows),
        3,
        "rows.rs names the raw declaration type only at its import, take signature, \
         and declaration map",
    );
    assert_eq!(
        mentions(&staging),
        0,
        "the staging boundary never names the raw declaration type",
    );
}
