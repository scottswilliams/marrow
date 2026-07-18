//! Composite (multi-column) keys at the root and branch levels, executed end to end.
//!
//! A composite key identifies each entry by its whole ordered tuple: `^r(k1, k2)`
//! addresses one root entry, `^r(k1, k2).b(bk1, bk2)` one branch entry. The physical key
//! is the prefix-free concatenation of the columns, so column *order* is load-bearing —
//! two entries whose columns are the same values in a different order are distinct. These
//! tests drive the whole production path — capture -> compile -> verify -> attach -> VM —
//! over one persistent ephemeral attachment.

use marrow_compile::SourceDiagnostic;
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// A composite-key root `^enrollments(student: string, course: string)` with a required
// `grade`, plus a composite-key branch `sessions(term: int, slot: int)` holding `room`.
const IDS_A: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Enrollment 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Enrollment.grade 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root enrollments 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key enrollments.student 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key enrollments.course 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id root Enrollment.sessions 30303030303030303030303030303030\n\
     id key Enrollment.sessions.term 31313131313131313131313131313131\n\
     id key Enrollment.sessions.slot 32323232323232323232323232323232\n\
     id field Enrollment.sessions.room 33333333333333333333333333333333\n\
     high-water 0\n\
     end\n";

const SOURCE_A: &str = r#"resource Enrollment {
    required grade: int

    sessions[term: int, slot: int] {
        required room: string
    }
}

store ^enrollments[student: string, course: string]: Enrollment

pub fn enroll(student: string, course: string, grade: int) {
    transaction {
        ^enrollments[student, course] = Enrollment(grade: grade)
    }
}

pub fn gradeOf(student: string, course: string): int? {
    return ^enrollments[student, course].grade
}

pub fn enrolled(student: string, course: string): bool {
    return exists(^enrollments[student, course])
}

pub fn unenroll(student: string, course: string) {
    transaction {
        delete ^enrollments[student, course]
    }
}

pub fn setSession(student: string, course: string, term: int, slot: int, room: string) {
    transaction {
        ^enrollments[student, course].sessions[term, slot] = Enrollment.sessions(room: room)
    }
}

pub fn sessionRoom(student: string, course: string, term: int, slot: int): string? {
    return ^enrollments[student, course].sessions[term, slot].room
}
"#;

// Place bindings over the composite-key root and the composite-key branch. A field read or
// write through a `place` must resolve the field against the place's durable node — a root
// by its entry site, a branch by its record — and the node kind is independent of how many
// key slots the place carries. A composite-key root place has several key slots but is still
// a root, so both `e.grade` and `e.grade = g` resolve the root's `grade`, not a (nonexistent)
// branch field.
const PLACE_EXPORTS: &str = r#"pub fn gradeViaPlace(student: string, course: string): int? {
    place e = ^enrollments[student, course]
    return e.grade
}

pub fn setGradeViaPlace(student: string, course: string, grade: int) {
    transaction {
        place e = ^enrollments[student, course]
        e.grade = grade
    }
}

pub fn roomViaPlace(student: string, course: string, term: int, slot: int): string? {
    place x = ^enrollments[student, course].sessions[term, slot]
    return x.room
}
"#;

// The L2 review obligation: a depth-3 branch chain with FOUR key columns, all `int` (the
// same type at every level), so no scalar-kind check can distinguish the columns — only
// their pop order is correct end to end. Root `^grid(a, b)`, branch `cell(c)`, nested
// branch `mark(d)`.
const IDS_B: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Grid 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Grid.label 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root grid 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key grid.a 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key grid.b 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id root Grid.cell 30303030303030303030303030303030\n\
     id key Grid.cell.c 31313131313131313131313131313131\n\
     id field Grid.cell.cval 32323232323232323232323232323232\n\
     id root Grid.cell.mark 40404040404040404040404040404040\n\
     id key Grid.cell.mark.d 41414141414141414141414141414141\n\
     id field Grid.cell.mark.v 42424242424242424242424242424242\n\
     high-water 0\n\
     end\n";

const SOURCE_B: &str = r#"resource Grid {
    required label: string

    cell[c: int] {
        required cval: int

        mark[d: int] {
            required v: int
        }
    }
}

store ^grid[a: int, b: int]: Grid

pub fn setMark(a: int, b: int, c: int, d: int, val: int) {
    transaction {
        ^grid[a, b].cell[c].mark[d] = Grid.cell.mark(v: val)
    }
}

pub fn markV(a: int, b: int, c: int, d: int): int? {
    return ^grid[a, b].cell[c].mark[d].v
}

pub fn markPresent(a: int, b: int, c: int, d: int): bool {
    return exists(^grid[a, b].cell[c].mark[d])
}

pub fn setCell(a: int, b: int, c: int, cval: int) {
    transaction {
        ^grid[a, b].cell[c] = Grid.cell(cval: cval)
    }
}

pub fn sumCells(a: int, b: int): int {
    var total = 0
    for c in ^grid[a, b].cell at most 100 {
        total += c
    } on more {
        total = -1
    }
    return total
}
"#;

fn compile_verify(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

/// Capture and compile `source` against `ids` through the production path, returning the
/// rejection diagnostics. Panics if compilation unexpectedly succeeds.
fn compile_errors(source: &str, ids: &str) -> Vec<SourceDiagnostic> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    marrow_compile::compile(&project).expect_err("compilation must be rejected")
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

struct DebugRun<'a>(&'a DurableRun);
impl std::fmt::Debug for DebugRun<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DurableRun::Ran(Ok(_)) => write!(f, "Ran(Ok(value))"),
            DurableRun::Ran(Err(fault)) => write!(f, "Ran(Err({}))", fault.code()),
            DurableRun::Parked => write!(f, "Parked"),
            DurableRun::Failed(code) => write!(f, "Failed({code})"),
        }
    }
}

fn run(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        other => panic!("{name} did not run cleanly: {:?}", DebugRun(&other)),
    }
}

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a composite-key root must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

fn present(b: bool) -> Option<Value> {
    Some(Value::Bool(b))
}

fn s(v: &str) -> Value {
    Value::Text(v.into())
}

/// A composite-key root addresses each entry by the whole ordered tuple: the transposed
/// tuple `(course, student)` is a distinct, absent entry even though both columns are
/// strings. Whole-entry create/read/presence/delete and a field read all key by the pair.
#[test]
fn a_composite_key_root_keys_by_the_ordered_tuple() {
    let image = compile_verify(SOURCE_A, IDS_A);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "enroll",
        vec![s("amy"), s("cs"), Value::Int(90)],
    );
    assert_eq!(
        run(&image, &mut attachment, "gradeOf", vec![s("amy"), s("cs")]),
        some_int(90)
    );
    assert_eq!(
        run(&image, &mut attachment, "enrolled", vec![s("amy"), s("cs")]),
        present(true)
    );
    // The transposed tuple is a different entry — column order is load-bearing.
    assert_eq!(
        run(&image, &mut attachment, "gradeOf", vec![s("cs"), s("amy")]),
        absent(),
        "the transposed (course, student) tuple addresses a distinct, absent entry",
    );
    assert_eq!(
        run(&image, &mut attachment, "enrolled", vec![s("cs"), s("amy")]),
        present(false)
    );

    // A whole-entry delete keys by the same tuple and leaves the transposed entry alone.
    run(
        &image,
        &mut attachment,
        "enroll",
        vec![s("cs"), s("amy"), Value::Int(10)],
    );
    run(&image, &mut attachment, "unenroll", vec![s("amy"), s("cs")]);
    assert_eq!(
        run(&image, &mut attachment, "enrolled", vec![s("amy"), s("cs")]),
        present(false),
        "the addressed entry was deleted"
    );
    assert_eq!(
        run(&image, &mut attachment, "gradeOf", vec![s("cs"), s("amy")]),
        some_int(10),
        "the transposed sibling entry is untouched"
    );
}

/// A composite-key *branch* keys by its own tuple under a composite-key root, so the whole
/// key-path is four columns `[student, course, term, slot]`. A transposed branch tuple
/// `(slot, term)` is a distinct, absent branch entry.
#[test]
fn a_composite_key_branch_keys_by_its_tuple_under_a_composite_root() {
    let image = compile_verify(SOURCE_A, IDS_A);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "setSession",
        vec![s("amy"), s("cs"), Value::Int(1), Value::Int(2), s("A100")],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sessionRoom",
            vec![s("amy"), s("cs"), Value::Int(1), Value::Int(2)]
        ),
        some_text("A100"),
    );
    // Transpose the branch tuple: a different branch entry.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sessionRoom",
            vec![s("amy"), s("cs"), Value::Int(2), Value::Int(1)]
        ),
        absent(),
        "the transposed (slot, term) branch tuple is a distinct, absent entry",
    );
    // Transpose a root column: the branch layer under the transposed root is empty.
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sessionRoom",
            vec![s("cs"), s("amy"), Value::Int(1), Value::Int(2)]
        ),
        absent(),
        "a transposed root tuple locates a different (empty) branch layer",
    );
}

/// A `place` over a composite-key root resolves its fields against the root node for both
/// reads and writes, exactly like an inline `^enrollments[student, course].grade` address:
/// the two key operands do not reclassify it as a branch place. Seeding then reading proves
/// the read side; writing a new `grade` back through the place and reading it again proves
/// the symmetric write side resolves the same root field. A composite-key branch place is
/// the control — both node kinds run through the same place field-resolution family — and
/// resolves `room` against its branch record.
#[test]
fn a_composite_root_place_reads_and_writes_its_fields_by_the_root_node() {
    let source = format!("{SOURCE_A}\n{PLACE_EXPORTS}");
    let image = compile_verify(&source, IDS_A);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "enroll",
        vec![s("amy"), s("cs"), Value::Int(90)],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "gradeViaPlace",
            vec![s("amy"), s("cs")]
        ),
        some_int(90),
        "a composite-root place reads `grade` off the root node",
    );

    // A field write through the composite-root place resolves the same root `grade`; the
    // read-back through the place observes the newly written value.
    run(
        &image,
        &mut attachment,
        "setGradeViaPlace",
        vec![s("amy"), s("cs"), Value::Int(75)],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "gradeViaPlace",
            vec![s("amy"), s("cs")]
        ),
        some_int(75),
        "a composite-root place writes `grade` on the root node, not a misrouted branch field",
    );

    run(
        &image,
        &mut attachment,
        "setSession",
        vec![s("amy"), s("cs"), Value::Int(1), Value::Int(2), s("A100")],
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "roomViaPlace",
            vec![s("amy"), s("cs"), Value::Int(1), Value::Int(2)]
        ),
        some_text("A100"),
        "a composite-branch place reads `room` off its branch record",
    );
}

/// The L2 pop-order pin: a depth-3 chain of four `int` key columns
/// `^grid(a, b).cell(c).mark(d)`. Because every column is the same type, only the exact
/// column order produces the right physical address — a same-type reversal at any level
/// would pass every scalar-kind check yet address a different node. Writing at
/// `(1, 2, 3, 4)` and reading back every transposition proves the whole key-path pops in
/// order end to end.
#[test]
fn a_deep_same_typed_key_path_pins_column_order_end_to_end() {
    let image = compile_verify(SOURCE_B, IDS_B);
    let mut attachment = attach(&image);

    let write = vec![
        Value::Int(1),
        Value::Int(2),
        Value::Int(3),
        Value::Int(4),
        Value::Int(99),
    ];
    run(&image, &mut attachment, "setMark", write);

    let at = |a, b, c, d| vec![Value::Int(a), Value::Int(b), Value::Int(c), Value::Int(d)];
    // The exact tuple reads back; the mark entry is present.
    assert_eq!(
        run(&image, &mut attachment, "markV", at(1, 2, 3, 4)),
        some_int(99)
    );
    assert_eq!(
        run(&image, &mut attachment, "markPresent", at(1, 2, 3, 4)),
        present(true)
    );
    // Every transposition addresses a different, absent node — proving the pop order.
    for (a, b, c, d) in [
        (2, 1, 3, 4), // swap the two root columns
        (1, 2, 4, 3), // swap the two branch keys (cell vs mark)
        (1, 3, 2, 4), // swap a root column with the cell key
        (3, 2, 1, 4), // swap the first root column with the cell key
        (4, 2, 3, 1), // swap the first root column with the mark key
    ] {
        assert_eq!(
            run(&image, &mut attachment, "markV", at(a, b, c, d)),
            absent(),
            "transposition ({a},{b},{c},{d}) must address a distinct, absent node",
        );
        assert_eq!(
            run(&image, &mut attachment, "markPresent", at(a, b, c, d)),
            present(false),
        );
    }
}

/// A single-column branch layer under a COMPOSITE-keyed root traverses end to end: the
/// `for c in ^grid(a, b).cell` head fixes the two-column ancestor `(a, b)` and iterates the
/// single-column `cell` keys under it, so the ancestor key-path carries multiple columns
/// through the traversal while the traversed layer stays single-column.
#[test]
fn a_single_column_branch_layer_traverses_under_a_composite_ancestor() {
    let image = compile_verify(SOURCE_B, IDS_B);
    let mut attachment = attach(&image);

    for c in [3, 1, 5] {
        run(
            &image,
            &mut attachment,
            "setCell",
            vec![Value::Int(1), Value::Int(2), Value::Int(c), Value::Int(0)],
        );
    }
    // A cell under a different composite root entry, which must not be visited.
    run(
        &image,
        &mut attachment,
        "setCell",
        vec![Value::Int(9), Value::Int(9), Value::Int(100), Value::Int(0)],
    );

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumCells",
            vec![Value::Int(1), Value::Int(2)]
        ),
        Some(Value::Int(9)),
        "the cell layer under (1, 2) iterates 1 + 3 + 5 = 9",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "sumCells",
            vec![Value::Int(3), Value::Int(4)]
        ),
        Some(Value::Int(0)),
        "an empty cell layer under a different composite ancestor sums to zero",
    );
}

/// Bounded traversal over a composite-keyed layer parks: the language spells no
/// composite-key iteration (one loop variable, one `from`), so a `for` head over a
/// composite root is a typed `check.unsupported` with a located span, never a silent
/// miscompile or an invented last-column-under-prefix semantics.
#[test]
fn bounded_traversal_over_a_composite_layer_is_rejected() {
    let body = r#"pub fn scan(): int {
    var total = 0
    for k in ^enrollments at most 10 {
        total += 1
    } on more {
        total = -1
    }
    return total
}
"#;
    let source = format!("{SOURCE_A}\n{body}");
    let diagnostics = compile_errors(&source, IDS_A);
    let hit = diagnostics
        .iter()
        .find(|d| d.code == marrow_codes::Code::CheckUnsupported.as_str())
        .expect("a check.unsupported diagnostic for composite-key traversal");
    assert!(
        hit.line >= 1 && hit.column >= 1,
        "the rejection carries a located span",
    );
}

/// A missing field through a composite-root `place` is a located `check.type` that names
/// the root container, exactly like an inline `^enrollments[student, course].nope` would
/// be. Resolving the field against the root node (not a misrouted, nonexistent branch)
/// means the message names `enrollments`, never an empty container.
#[test]
fn a_missing_field_through_a_composite_root_place_names_the_root_container() {
    let body = r#"pub fn badGrade(student: string, course: string): int? {
    place e = ^enrollments[student, course]
    return e.nope
}
"#;
    let source = format!("{SOURCE_A}\n{body}");
    let diagnostics = compile_errors(&source, IDS_A);
    let hit = diagnostics
        .iter()
        .find(|d| d.code == marrow_codes::Code::CheckType.as_str())
        .expect("a check.type diagnostic for the missing field");
    assert_eq!(
        hit.message, "`enrollments` has no field `nope`",
        "a composite-root place names its root container, not an empty branch",
    );
    assert!(
        hit.line >= 1 && hit.column >= 1,
        "the rejection carries a located span",
    );
}
