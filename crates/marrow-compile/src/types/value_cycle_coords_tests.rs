//! The value-cycle report reads declaration coordinates the declare pass owns, not
//! the syntax tree, and the durable declaration projection is the one reader of the
//! declarations it projects.
//!
//! These pin the report's artifact through the production `compile` path. The
//! structural half — the reject pass's declaration-free signature and the
//! projection-owned syntax reads — is enforced by
//! `reporting_a_value_cycle_reads_no_syntax_declaration` in `absence_gates` and the
//! row-table gates in `durable_identity_stability`.

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use crate::CompileFailure;
use crate::compile::compile;

fn project(source: String) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// One program reaching every declaration family this row projects into typed rows:
/// a value cycle, an admitted store binding, an unbound store spelling, a managed
/// index, a root key tuple, and a keyed branch tuple. No identity ledger is
/// captured, so the durable half reports its complete `(kind, path)` anchor demand
/// as `check.durable_identity` rows — the anchors `.marrow/ids` would hold.
const PROJECTION_CORPUS: &str = "\
struct Knot { me: Knot }\n\
struct Settled { value: int }\n\
resource Book {\n\
    required title: string\n\
    shelf: string\n\
\n\
    notes[noteId: string] {\n\
        required text: string\n\
    }\n\
}\n\
store ^books[id: int]: Book {\n\
    index byShelf[shelf, id]\n\
}\n\
store ^nowhere[id: int]: Missing\n\
fn main() {\n\
}\n";

/// The complete ordered artifact the projection corpus must report: the cycle report
/// keeps its coordinates, the durable build keeps its anchor demand, and the index and
/// key rows keep their admission rows. Not a byte of it may move.
const PROJECTION_ARTIFACT: &str = "src/main.mw:11:7 check.durable_identity durable identity for application `.` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for root `books` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for product `Book` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for key `books.id` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for field `Book.title` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for field `Book.shelf` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for root `Book.notes` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for key `Book.notes.noteId` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for field `Book.notes.text` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for index `books.byShelf` is missing from .marrow/ids; `marrow run` or `marrow test` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:14:1 check.type `Missing` is not a resource in this project\n\
     src/main.mw:1:8 check.recursion value type `Knot` contains itself through the cycle Knot -> Knot";

/// The complete value-cycle/store/index/key corpus reports cycle diagnostics and a
/// `.marrow/ids` anchor demand byte-identical to [`PROJECTION_ARTIFACT`]. That the
/// retained declaration lists are unreadable after row construction is structural and
/// belongs to the gates this file's header names: a lexical counter cannot pin an
/// absence.
#[test]
fn durable_projection_survives_syntax_poison() {
    let result = compile(&project(PROJECTION_CORPUS.to_string()));
    let diagnostics = match result {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics,
        other => panic!("the projection corpus must be refused with diagnostics: {other:?}"),
    };
    let artifact = diagnostics
        .iter()
        .map(|row| {
            let span = row.span();
            format!(
                "{}:{}:{} {} {}",
                row.file().as_str(),
                span.line,
                span.column,
                row.code().as_str(),
                row.message()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        artifact, PROJECTION_ARTIFACT,
        "the projection artifact moved"
    );
}

/// A repeat for one declaration keeps the first coordinate, for a record and for an
/// enum.
///
/// The declare pass never reserves one image type twice, so no corpus could notice the
/// table starting to keep the later coordinate instead — and a caller reporting at a
/// declaration would then be steered to a later homonym. Hence the direct pin.
#[test]
fn a_repeated_declaration_keeps_its_first_coordinate() {
    use marrow_image::{EnumId, TypeId};
    use marrow_syntax::SourceSpan;

    let mut coordinates = super::decl_coords::DeclarationCoordinates::default();
    let ty = TypeId::from_index(7);
    let first_file = crate::test_file("src/first.mw").clone();
    let later_file = crate::test_file("src/later.mw").clone();
    let first_span = SourceSpan {
        start_byte: 10,
        end_byte: 14,
        line: 2,
        column: 8,
    };
    let later_span = SourceSpan {
        start_byte: 90,
        end_byte: 94,
        line: 9,
        column: 8,
    };

    coordinates.declare(
        ty,
        crate::analysis::FileRef::admitted(0),
        &first_file,
        first_span,
    );
    coordinates.declare(
        ty,
        crate::analysis::FileRef::admitted(1),
        &later_file,
        later_span,
    );

    let (file, span) = coordinates
        .resolve(ty)
        .expect("a declared type has a coordinate");
    assert_eq!(file, &first_file, "the first module coordinate stands");
    assert_eq!(span, first_span, "the first name span stands");

    let id = EnumId::from_index(7);
    coordinates.declare_enum(
        id,
        crate::analysis::FileRef::admitted(0),
        &first_file,
        first_span,
    );
    coordinates.declare_enum(
        id,
        crate::analysis::FileRef::admitted(1),
        &later_file,
        later_span,
    );

    let (file, span) = coordinates
        .resolve_enum(id)
        .expect("a declared enum has a coordinate");
    assert_eq!(file, &first_file, "the first module coordinate stands");
    assert_eq!(span, first_span, "the first name span stands");
}
