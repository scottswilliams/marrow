//! `exists(value)` over a `T?` through the production path: it answers presence for
//! any optional expression and narrows nothing.

use crate::common::Project;
use marrow_vm::Value;

const MAYBE: &str = "fn maybe(present: bool): int? {\n\
    \x20   if present {\n\
    \x20       return 7\n\
    \x20   }\n\
    \x20   return absent\n\
    }\n";

#[test]
fn exists_answers_presence_for_an_optional_value() {
    let mut session = Project::single(&format!(
        "{MAYBE}\n\
         pub fn present(): bool {{\n\
         \x20   return exists(maybe(true))\n\
         }}\n\n\
         pub fn missing(): bool {{\n\
         \x20   const v = maybe(false)\n\
         \x20   return exists(v)\n\
         }}\n"
    ))
    .session();
    assert_eq!(session.call("present", vec![]), Some(Value::Bool(true)));
    assert_eq!(session.call("missing", vec![]), Some(Value::Bool(false)));
}

/// The probe establishes no narrowing: the value keeps its optional type inside the
/// guarded block, so an arithmetic use there is a `check.type`.
#[test]
fn exists_establishes_no_narrowing() {
    let diagnostics = Project::single(&format!(
        "{MAYBE}\n\
         pub fn f(): int {{\n\
         \x20   const v = maybe(true)\n\
         \x20   if exists(v) {{\n\
         \x20       return v + 1\n\
         \x20   }}\n\
         \x20   return 0\n\
         }}\n"
    ))
    .try_image()
    .expect_err("the optional is not narrowed");
    let row = diagnostics.only("check.type");
    assert_eq!(row.line(), 11, "{:?}", diagnostics.all());
}

/// A value that is not optional has no presence to probe.
#[test]
fn exists_over_a_present_value_is_refused() {
    let diagnostics = Project::single("pub fn f(): bool {\n    return exists(3)\n}\n")
        .try_image()
        .expect_err("a present value is refused");
    let row = diagnostics.only("check.type");
    assert_eq!(
        (row.line(), row.column()),
        (2, 19),
        "{:?}",
        diagnostics.all()
    );
}

/// A sparse field of a local resource value is a `T?` like any other: absent until
/// assigned, present after.
#[test]
fn exists_reads_a_sparse_field_of_a_local_value() {
    let mut session = Project::single(
        r#"resource Book {
    required title: string
    subtitle: string
}

pub fn bare(): bool {
    const b = Book(title: "x")
    return exists(b.subtitle)
}

pub fn subtitled(): bool {
    var b = Book(title: "x")
    b.subtitle = "y"
    return exists(b.subtitle)
}
"#,
    )
    .session();
    assert_eq!(session.call("bare", vec![]), Some(Value::Bool(false)));
    assert_eq!(session.call("subtitled", vec![]), Some(Value::Bool(true)));
}

/// The subject is evaluated exactly once: an export that bumps a durable counter and
/// returns the new count leaves the counter at one.
#[test]
fn exists_evaluates_its_subject_once() {
    const IDS: &str = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Counter.n 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         high-water 0\n\
         end\n";
    let workspace = Project::single(
        r#"resource Counter {
    required n: int
}

store ^counters[id: int]: Counter

pub fn bump(): int? {
    var next = 0
    transaction {
        next = (^counters[1].n ?? 0) + 1
        ^counters[1] = Counter(n: next)
    }
    return next
}

pub fn count(): int {
    return ^counters[1].n ?? 0
}

test "the subject is evaluated once" {
    assert exists(bump())
    assert count() == 1
}
"#,
    )
    .ids(IDS)
    .materialize("exists-once");
    let output = workspace.marrow(&["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""failed":0"#), "{stdout}");
}

/// An optional parameter is refused at its annotation, and `exists(p)` over the
/// refused name adds no second row.
#[test]
fn exists_over_a_refused_optional_parameter_adds_no_row() {
    let diagnostics = Project::single("pub fn f(p: int?): bool {\n    return exists(p)\n}\n")
        .try_image()
        .expect_err("an optional parameter is refused");
    assert_eq!(diagnostics.len(), 1, "{:?}", diagnostics.all());
    assert_eq!(diagnostics.only("check.unsupported").line(), 1);
}
