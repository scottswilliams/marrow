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

/// The 1-based column of the declared name on a form's header line.
fn name_column(form: &str) -> u32 {
    form.lines()
        .next()
        .and_then(|header| header.find(" N"))
        .map(|at| at as u32 + 2)
        .expect("every form declares `N` on its header line")
}

/// The diagnostics of two declarations written in the given order, each labelled
/// with the form whose lines carry it, as `(form, code, column, width)`.
fn labelled(
    first: (&'static str, &'static str),
    second: (&'static str, &'static str),
) -> Vec<(&'static str, Code, u32, usize)> {
    let source = format!("module main\n\n{}\n{}\n{TAIL}", first.1, second.1);
    let first_lines = 3..3 + first.1.lines().count() as u32;
    let second_lines = first_lines.end + 1..first_lines.end + 1 + second.1.lines().count() as u32;
    let mut rows: Vec<(&'static str, Code, u32, usize)> = diagnostics_of(&project(&source))
        .iter()
        .map(|row| {
            let span = row.span();
            let owner = if first_lines.contains(&span.line) {
                first.0
            } else if second_lines.contains(&span.line) {
                second.0
            } else {
                "elsewhere"
            };
            (
                owner,
                row.code(),
                span.column,
                span.end_byte - span.start_byte,
            )
        })
        .collect();
    rows.sort_unstable_by_key(|(owner, code, ..)| (*owner, code.as_str()));
    rows
}

#[test]
fn a_name_conflict_verdict_does_not_depend_on_declaration_order() {
    for (index, first) in FORMS.iter().enumerate() {
        for second in FORMS.iter().skip(index + 1) {
            let written = labelled(*first, *second);
            let [(owner, code, column, width)] = written.as_slice() else {
                panic!(
                    "`{}` and `{}` declare one name and exactly one is refused: {written:?}",
                    first.0, second.0
                );
            };
            assert_eq!(*code, Code::CheckNameConflict, "{}/{}", first.0, second.0);
            let form = FORMS
                .iter()
                .find(|(name, _)| name == owner)
                .map(|(_, form)| *form)
                .unwrap_or_else(|| panic!("the refusal is carried by a form, not {owner}"));
            assert_eq!(
                (*column, *width),
                (name_column(form), 1),
                "`{}`'s refusal spans its declared name",
                owner
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

/// A reserved generic name is refused in every declaration form with one
/// `check.name_conflict` spanning the declared name.
#[test]
fn a_reserved_name_is_refused_in_every_declaration_form() {
    for reserved in ["Option", "Result"] {
        for (form, lines) in FORMS {
            let source = format!(
                "module main\n\n{}\n{TAIL}",
                lines.replace(" N", &format!(" {reserved}"))
            );
            let rows = diagnostics_of(&project(&source));
            let [row] = rows.as_slice() else {
                panic!("`{reserved}` as a {form} is refused exactly once: {rows:?}");
            };
            assert_eq!(
                row.code(),
                Code::CheckNameConflict,
                "{reserved} as a {form}"
            );
            let span = row.span();
            assert_eq!(
                (span.line, span.column, span.end_byte - span.start_byte),
                (3, name_column(lines), reserved.len()),
                "`{reserved}` as a {form} is refused at its declared name"
            );
        }
    }
}
