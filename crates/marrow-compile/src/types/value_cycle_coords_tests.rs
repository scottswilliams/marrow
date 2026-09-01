//! The value-cycle report reads declaration coordinates the declare pass owns, not
//! the syntax tree.
//!
//! Driven through the production `compile` path. The observable is an exact
//! operation count, not a ratio: the pass examines zero syntax declarations, so a
//! reintroduced name scan fails the gate at one declaration rather than needing a
//! scale at which a linear term becomes visible.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use super::{ScalingCounts, capture_scaling_counts};
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

/// A corpus whose value cycles are reported: `pad` unrelated declarations ahead of
/// a cyclic struct and a cyclic resource, so a name scan over either declaration
/// list has something to walk before it matches.
fn cyclic_corpus(pad: usize) -> String {
    let mut source = String::new();
    for index in 0..pad {
        writeln!(source, "struct Pad{index} {{ value: int }}").expect("write pad struct");
    }
    for index in 0..pad {
        writeln!(source, "resource Spare{index} {{ value: int }}").expect("write pad resource");
    }
    source.push_str("struct Knot { me: Knot }\n");
    source.push_str("resource Coil { me: Coil }\n");
    source.push_str("fn main() {\n}\n");
    source
}

/// The counts of one compile, kept whether or not the program is admitted — this
/// corpus is deliberately refused, and its refusal is the thing being measured.
fn counts_of(source: String) -> (bool, ScalingCounts) {
    let (result, counts) = capture_scaling_counts(|| compile(&project(source)));
    (result.is_err(), counts)
}

/// The corpus must actually reach the report. A corpus that compiled cleanly, or
/// that stopped at an earlier phase, would make the zero below vacuous.
#[test]
fn the_cyclic_corpus_is_still_refused() {
    let (refused, _) = counts_of(cyclic_corpus(16));
    assert!(
        refused,
        "the value-cycle corpus must be refused; a clean compile never reaches the report"
    );
}

/// The gate: reporting a value cycle examines no syntax declaration.
#[test]
fn reporting_a_value_cycle_reads_no_syntax_declaration() {
    for pad in [1, 16, 64] {
        let (refused, counts) = counts_of(cyclic_corpus(pad));
        assert!(refused, "the corpus at pad {pad} must be refused");
        assert_eq!(
            counts.value_cycle_declaration_scan_steps, 0,
            "reporting a value cycle scanned {} syntax declarations at pad {pad}; the declare \
             pass owns the coordinate, so the report must read none",
            counts.value_cycle_declaration_scan_steps
        );
    }
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

/// The complete ordered artifact the projection corpus reported before any
/// declaration became a typed row, captured from the pre-conversion tree. The
/// conversion must not move a byte of it: the cycle report keeps its coordinates,
/// the durable build keeps its anchor demand, and the index and key rows keep
/// their admission rows.
const PROJECTION_ARTIFACT: &str = "src/main.mw:11:7 check.durable_identity durable identity for application `.` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for root `books` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for product `Book` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for key `books.id` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for field `Book.title` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for field `Book.shelf` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for root `Book.notes` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for key `Book.notes.noteId` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for field `Book.notes.text` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:11:7 check.durable_identity durable identity for index `books.byShelf` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
     src/main.mw:14:1 check.type `Missing` is not a resource in this project\n\
     src/main.mw:1:8 check.recursion value type `Knot` contains itself through the cycle Knot -> Knot";

/// The opening production-path check of this row: after the declare pass and the
/// durable row tables are built, the retained raw declaration lists are read by
/// nothing — value-cycle reporting and store binding consume typed rows — and the
/// cycle diagnostics and `.marrow/ids` anchor demand are byte-identical to the
/// pre-conversion artifact.
#[test]
fn durable_projection_survives_syntax_poison() {
    let (result, counts) =
        capture_scaling_counts(|| compile(&project(PROJECTION_CORPUS.to_string())));
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
                row.code(),
                row.message()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        artifact, PROJECTION_ARTIFACT,
        "the projection artifact moved"
    );
    assert_eq!(
        counts.value_cycle_declaration_scan_steps, 0,
        "value-cycle reporting read {} retained syntax declarations after row construction",
        counts.value_cycle_declaration_scan_steps
    );
    assert_eq!(
        counts.durable_declaration_scan_steps, 0,
        "the durable build read {} retained syntax declarations after row construction",
        counts.durable_declaration_scan_steps
    );
}

/// A repeat for one type keeps the first coordinate, exactly as the table's
/// contract states.
///
/// The declare pass never reserves one image type twice, so this arm is
/// unreachable through any compile — which is precisely why it is pinned
/// directly: were the table to start keeping the later coordinate instead, no
/// corpus could notice, and a caller reporting at a declaration could one day be
/// steered to a later homonym with nothing standing in the way.
#[test]
fn a_repeated_declaration_keeps_its_first_coordinate() {
    use marrow_image::TypeId;
    use marrow_syntax::SourceSpan;

    let mut coordinates = super::decl_coords::DeclarationCoordinates::default();
    let ty = TypeId::from_index(7);
    let first_file = crate::test_file_identity("src/first.mw");
    let later_file = crate::test_file_identity("src/later.mw");
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
}
