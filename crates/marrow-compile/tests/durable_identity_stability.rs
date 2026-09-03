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

use std::path::{Path, PathBuf};

use source_projection::{is_test_only_file, production_code, production_code_of};

/// The production code of every source file under this crate's `src`,
/// concatenated in path order: the search space of a census that must hold
/// crate-wide, because a shape minted in a file the per-file censuses never read
/// is still that shape.
///
/// Every check below reads this text and decides one thing: whether a spelling
/// occurs at a site, and how often, in the source as written. None of them binds a
/// call graph. A renamed function, a call through an alias or a function value, a
/// wrapper that forwards, and a second deciding site sharing one counted
/// constructor all leave these numbers intact. Each census states the spellings it
/// reads; the property itself is carried by a type boundary, a visibility, or a
/// production-path test, and is named where that is so.
fn production_code_of_crate() -> String {
    fn walk(dir: &Path, code: &mut String) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("read src dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, code);
            } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_only_file(&path) {
                code.push_str(&production_code(
                    &std::fs::read_to_string(&path).expect("read source file"),
                ));
                code.push('\n');
            }
        }
    }
    let mut code = String::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut code,
    );
    assert!(!code.is_empty(), "the source tree is scanned");
    code
}

/// The spellings that carry the registry/slice drift occur exactly as counted,
/// crate-wide: one `DurableResourceMissing` construction, one
/// `ResourceDirectory::take(`, one `DurableRegistry::build(`, and an exact census
/// of the raw declaration tuple and the declaration type it carries.
///
/// Round 2 named two evasions of per-file censuses: a factory holding the sole
/// counted construction, and a raw-slice alias minted in a file the durable
/// censuses never read. Counting crate-wide moves a number for either spelling.
///
/// What it does not establish: that the drift has one *deciding* seam. A second site
/// reached through a renamed helper, a function value, or a shared constructor keeps
/// every number here intact. Nor does it say the join is *right*: `ResourceDirectory`
/// pairs an admitted record with a declaration of the same written name, and a
/// same-named declaration supplied in place of the admitted one is accepted with no
/// disagreement to report. Closing that means handing the durable build the pairing
/// the declare pass already made, and until then this census is the tripwire against
/// a post-projection rescan, not a proof that the pairing is sound.
#[test]
fn the_durable_resource_drift_seams_are_spelled_once() {
    let code = production_code_of_crate();
    for deleted in [
        "records.by_name(&store.resource)",
        "decl.name == store.resource",
        "named_type(resource)",
    ] {
        assert!(!code.contains(deleted), "`{deleted}` is displaced by rows");
    }
    assert_eq!(
        code.matches("GenericInvariant::DurableResourceMissing(")
            .count(),
        1,
        "the drift invariant's constructor is spelled once, at the directory join",
    );
    assert_eq!(
        code.matches("ResourceDirectory::take(").count(),
        1,
        "`ResourceDirectory::take(` is spelled once, at the durable build entry",
    );
    assert_eq!(
        code.matches("DurableRegistry::build(").count(),
        1,
        "`DurableRegistry::build(` is spelled once, in the compile driver",
    );
    assert!(
        code.contains("StoreResourceBinding::Accepted"),
        "the typed store binding must be the live subject of this gate",
    );
    assert_eq!(
        code.matches("(FileRef, FileIdentity, &ResourceDecl)")
            .count(),
        11,
        "the raw declaration tuple is spelled by the declaration passes' parameters, the \
         compile driver's collection, and the durable build entry, and by nothing else",
    );
    assert_eq!(
        code.matches("ResourceDecl").count() - code.matches("ResourceDeclId").count(),
        18,
        "the raw declaration type's crate-wide census moved — its imports, the eleven \
         tuple spellings, the declare pass's per-declaration reader, and the row table's \
         ordinal read own every mention; an alias or a new carrier is a lease-and-review \
         event. It fell from nineteen when the row table stopped keeping a map keyed on \
         resource spellings: that map named the raw type once, and the ordinal that \
         replaced it names nothing.",
    );
}

/// The durable builder spells no index or key syntax, and mints a key anchor at
/// exactly two sites.
///
/// Every index admission rule once read the parsed `IndexDecl` and rendered each
/// argument's path spelling at the moment it needed one, which made "the same
/// component" a per-caller answer; the `IndexTable` renders each path and classifies
/// its reach once, when the row is taken, so a rule can only ask.
///
/// The anchor join itself is no longer this gate's subject. `KeyTable` retains no
/// declared key column and `identity_path` is private to `durable/rows.rs`, so the
/// builder is handed rendered anchors and is not in a position to spell the join a
/// second time off a row — a visibility fact, which is why the census that stood in
/// for it is gone.
///
/// What remains uncovered, and is not claimed here: the builder still holds each
/// store's `StoreDecl` and the raw `resource` slice, so key and index syntax is
/// reachable there under any spelling these needles do not name.
#[test]
fn the_durable_builder_spells_no_index_or_key_syntax() {
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
            "`{absent}` names declaration syntax and is spelled in the durable builder",
        );
    }
    assert!(
        builder.contains("IndexArgReach::ThroughMember"),
        "the nested-member rule must read the row's classified reach",
    );
    assert_eq!(
        builder.matches("IdentityKind::Key,").count(),
        2,
        "`IdentityKind::Key,` is spelled at two sites: the store root's tuple and a \
         branch's",
    );
    let rows = production_code_of("durable/rows.rs");
    assert!(
        rows.contains("fn take(indexes: &'a [IndexDecl])") && rows.contains("field_path_spelling"),
        "the index row table must be the live reader of index syntax",
    );
    assert!(
        rows.contains("fn identity_path(") && rows.contains("fn over_wide("),
        "the anchor join and the width cap must still live on the key row table",
    );
}

/// The raw declaration slice's type is spelled once in the durable builder and never
/// in the staging wrapper, and `directory: &ResourceDirectory<'_>` is spelled once in
/// each.
///
/// `StagedStoreTxn::build_one` and the `durable.rs::build_one` it forwards to once
/// took `&[(FileRef, FileIdentity, &ResourceDecl)]` and recovered the resource
/// declaration by name search after row construction. This is the lane's named
/// enforcement artifact for the carrier: the slice type occurs only at the `build`
/// entry, where it is row-construction input handed to `ResourceDirectory::take`.
///
/// It reads type spellings in two files. A slice reaching the staging boundary
/// through a type alias, a tuple struct, or a closure it does not name would leave
/// these counts unchanged.
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

/// Each row table's field lines are exactly as written here, visibility included.
///
/// The round-1 review constructed a bridge the lexical needles missed: a type alias
/// for the raw declaration slice, carried as an extra directory field and consumed by
/// a `find_map` name recovery, kept every asserted count intact. A field-exact pin
/// closes that: a carrier added to one of these tables changes its pinned line list
/// whatever its type is spelled as, and a field opened to the durable builder changes
/// the line's `pub(super)`.
///
/// It reads the field lines of five named structs in one file. A carrier reached
/// through a type these rows already hold, or added to a struct not pinned here, is
/// outside its reach.
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
    let pins: [(&str, &[&str]); 6] = [
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
                "pub(super) name: &'a str,",
                "pub(super) path: String,",
                "pub(super) fields: Vec<&'a FieldDecl>,",
                "pub(super) first_member_span: Option<SourceSpan>,",
                "pub(super) keys: Option<KeyTable<'a>>,",
                "pub(super) groups: Vec<GroupRow<'a>>,",
            ],
        ),
        (
            "KeyTable<'a>",
            &[
                "owner: KeyOwner<'a>,",
                "declared_width: usize,",
                "resolution: Result<Vec<KeyColumnRow<'a>>, Box<SourceDiagnostic>>,",
            ],
        ),
        (
            "AdmittedKeyColumn<'a>",
            &[
                "pub(super) spelling: &'a str,",
                "pub(super) anchor: String,",
                "pub(super) scalar: ScalarType,",
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

/// The durable module's `.find(` occurrences are exactly as counted, the three
/// recovery combinators are unspelled there, and the raw declaration type occurs
/// exactly as counted per file.
///
/// The counts are exact rather than "at most" so a recovery rewritten onto an
/// already-counted combinator moves a number instead of slipping past a needle. Each
/// counted site reads members of ONE declaration's own row; that is what was checked
/// when these numbers were set, and what a reviewer re-checks when one moves.
///
/// It reads spellings in three files. A recovery written with a `for` loop, a method
/// on a helper type, or a combinator not named here occurs without moving a number.
#[test]
fn the_declaration_search_census_is_closed_at_its_spellings() {
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
        "the row tables and the staging boundary spell no `.find(` at all",
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
    // The raw declaration type's mentions are counted per file, so an alias minted
    // from either file moves a number.
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
        2,
        "rows.rs names the raw declaration type only at its import and take signature. \
         The third mention was the spelling-keyed declaration map, and it went with the \
         name join: the pairing is read from the ordinal the declare pass recorded, so \
         there is no map to key and no third place to name the raw type",
    );
    assert_eq!(
        mentions(&staging),
        0,
        "the staging boundary never names the raw declaration type",
    );
}
