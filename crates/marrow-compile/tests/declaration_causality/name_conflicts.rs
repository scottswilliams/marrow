//! One conflict predicate for the whole type namespace.
//!
//! Every declaration pass answers "is this name taken?" from the same predicate,
//! so which of two colliding declarations is refused follows from the declaration
//! forms and never from the order the source happens to write them. The passes do
//! not run in source order — aliases resolve before nominals, templates before the
//! concrete declare-then-fill passes — so a per-pass predicate consulting its own
//! set of tables silently makes the verdict order-dependent.
//!
//! Each fixture writes two declarations of one name, in both orders, and asserts
//! the reports agree on code and on *which declaration form* carries each one.

use super::{diagnostics_of, project};
use marrow_codes::Code;

/// A declaration of the name `N`, one per form the type namespace admits.
const FORMS: [(&str, &str); 6] = [
    ("alias", "alias N = int\n"),
    ("nominal", "type N: int in 0..10\n"),
    ("template", "struct N<T> {\n    v: T\n}\n"),
    ("struct", "struct N {\n    v: int\n}\n"),
    ("enum", "enum N {\n    one\n}\n"),
    ("resource", "resource N {\n    required v: string\n}\n"),
];

const TAIL: &str = "pub fn driver(n: int): int {\n    return n\n}\n";

/// The diagnostics of two declarations written in the given order, each labelled
/// with the form whose lines carry it.
fn labelled(
    first: (&'static str, &'static str),
    second: (&'static str, &'static str),
) -> Vec<(&'static str, Code)> {
    let source = format!("module main\n\n{}\n{}\n{TAIL}", first.1, second.1);
    let first_lines = 3..3 + first.1.lines().count() as u32;
    let second_lines = first_lines.end + 1..first_lines.end + 1 + second.1.lines().count() as u32;
    let mut rows: Vec<(&'static str, Code)> = diagnostics_of(&project(&source))
        .iter()
        .map(|row| {
            let line = row.span().line;
            let owner = if first_lines.contains(&line) {
                first.0
            } else if second_lines.contains(&line) {
                second.0
            } else {
                "elsewhere"
            };
            (owner, row.code())
        })
        .collect();
    rows.sort_unstable_by_key(|(owner, code)| (*owner, code.as_str()));
    rows
}

#[test]
fn a_name_conflict_verdict_does_not_depend_on_declaration_order() {
    for (index, first) in FORMS.iter().enumerate() {
        for second in FORMS.iter().skip(index + 1) {
            let written = labelled(*first, *second);
            assert!(
                written
                    .iter()
                    .any(|(_, code)| code == &Code::CheckNameConflict || code == &Code::CheckType),
                "`{}` and `{}` declare one name and must collide: {written:?}",
                first.0,
                second.0,
            );
            assert_eq!(
                written,
                labelled(*second, *first),
                "`{}` and `{}` declaring one name must reach the same verdict in either order",
                first.0,
                second.0,
            );
        }
    }
}
