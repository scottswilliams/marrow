//! End-to-end group materialized-value tests: an unkeyed `group` block is a
//! nested sub-record value inside its resource. Its scalar leaves participate in
//! the materialized value, `entry.group.field` reads and assigns, the whole group
//! reads and copies as a value unit, and a required group descendant is a required
//! member of the containing resource. The `groups` conformance fixture travels the
//! production path (capture -> compile -> encode -> verify -> VM) through the built
//! binary; inline projects assert the completeness rejection.

use crate::common::{Project, conformance_dir, marrow_in};

/// The group conformance fixture passes end to end: a fresh group leaf reads
/// absent, an assignment sets a leaf present, the whole group reads and copies as
/// a value unit with value semantics, and `unset` clears a leaf.
#[test]
fn group_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(&conformance_dir("groups"), &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "group fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":8"#), "{summary}");
}

/// Required-completeness over group descendants: a resource whose group declares a
/// `required` leaf cannot be constructed without supplying it: `Book(title: ...)`
/// with the group omitted is a `check.type` rejection naming the missing group
/// member rather than a silent incomplete value.
#[test]
fn a_required_group_leaf_makes_construction_incomplete() {
    let workspace = Project::single(
        r#"module main

resource Book {
    required title: string
    details {
        required pages: int
    }
}

pub fn make(): string {
    const b = Book(title: "x")
    return b.title
}
"#,
    )
    .materialize("required-group-leaf");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "construction with an unsatisfiable required group leaf must be rejected: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.type""#),
        "expected a check.type rejection: {stdout}"
    );
}

/// The required group leaf is satisfied by the qualified group constructor supplied
/// as a named argument: `Book(title: ..., details: Book.details(pages: 3))`
/// constructs and runs, and the leaf reads present.
#[test]
fn a_required_group_leaf_constructs_when_supplied() {
    let workspace = Project::single(
        r#"module main

resource Book {
    required title: string
    details {
        required pages: int
    }
}

pub fn make(): int {
    const b = Book(title: "x", details: Book.details(pages: 3))
    return b.details.pages
}
"#,
    )
    .materialize("required-group-supplied");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "a supplied required group leaf must construct: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":3"#),
        "leaf must read present: {stdout}"
    );
}

/// The qualified group constructor enforces its own required leaves: omitting a
/// required leaf in `Book.details(...)` is a `check.type` rejection.
#[test]
fn a_group_constructor_missing_a_required_leaf_is_rejected() {
    let workspace = Project::single(
        r#"module main

resource Book {
    required title: string
    details {
        required pages: int
    }
}

pub fn make(): int {
    const b = Book(title: "x", details: Book.details())
    return b.details.pages
}
"#,
    )
    .materialize("group-ctor-missing-required");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a group constructor omitting a required leaf must be rejected: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.type""#),
        "expected a check.type rejection: {stdout}"
    );
}

/// A group with only sparse leaves constructs, and reading an unset leaf yields
/// absent through the production path.
#[test]
fn a_sparse_group_constructs_and_reads_absent() {
    let workspace = Project::single(
        r#"module main

resource Book {
    required title: string
    details {
        pages: int
    }
}

pub fn make(): int {
    const b = Book(title: "x")
    return b.details.pages ?? 5
}
"#,
    )
    .materialize("sparse-group");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "sparse group must construct: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":5"#),
        "unset group leaf must read absent (falls through to 5): {stdout}"
    );
}

/// A nested group (a group inside a group) is deferred: it is a typed
/// `check.unsupported` at the inner group rather than a silent drop, so the trigger
/// is recorded for the lane that lifts it.
#[test]
fn a_nested_group_is_declared_unsupported() {
    let workspace = Project::single(
        r#"module main

resource Book {
    required title: string
    details {
        pages: int
        inner {
            depth: int
        }
    }
}

pub fn make(): int {
    const b = Book(title: "x")
    return 0
}
"#,
    )
    .materialize("nested-group");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a nested group must be rejected: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.unsupported""#),
        "expected a check.unsupported for the nested group: {stdout}"
    );
}

/// Nested field assignment reaches through any present composite, not only a group:
/// a required struct field is read-modified-written the same way.
#[test]
fn nested_assignment_reaches_through_a_required_struct() {
    let workspace = Project::single(
        r#"module main

struct Point {
    x: int
    y: int
}

resource Box {
    required id: int
    required at: Point
}

pub fn make(): int {
    var b = Box(id: 1, at: Point(x: 2, y: 3))
    b.at.x = 9
    return b.at.x
}
"#,
    )
    .materialize("nested-struct-rmw");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "required-struct RMW must run: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":9"#),
        "RMW must observe the new value: {stdout}"
    );
}

/// The read-modify-write guard: assigning through a possibly-absent composite member
/// (a sparse struct field) is a `check.type` rejection, since the container may be
/// absent. Assign the member a present value first.
#[test]
fn assignment_through_a_possibly_absent_member_is_rejected() {
    let workspace = Project::single(
        r#"module main

struct Point {
    x: int
    y: int
}

resource Box {
    required id: int
    maybe: Point
}

pub fn make(): int {
    var b = Box(id: 1)
    b.maybe.x = 9
    return 0
}
"#,
    )
    .materialize("absent-through");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "assigning through a possibly-absent member must be rejected: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.type""#),
        "expected a check.type rejection: {stdout}"
    );
}
