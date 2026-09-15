//! Group writes consume presence after all RHS effects and evaluate the RHS once.

use super::{
    IDS, REQUIRED_LEAF_SCHEMA, SCHEMA, SOURCE, as_int, as_str, compile_diagnostics_with_schema, i,
    s,
};
use crate::common::Project;

fn assert_requires_presence(body: &str, place: &str) {
    assert_requires_presence_with_schema(SCHEMA, body, place);
}

fn assert_requires_presence_with_schema(schema: &str, body: &str, place: &str) {
    let source = format!("{schema}\n{body}");
    let start = source.find(place).expect("the fixture contains the write");
    let line = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let column = source[..start]
        .rfind('\n')
        .map_or(start + 1, |newline| start - newline);
    let diagnostics = compile_diagnostics_with_schema(schema, body);
    assert_eq!(diagnostics.len(), 1, "{:?}", diagnostics.all());
    let diagnostic = diagnostics.iter().next().expect("one diagnostic");
    assert_eq!(
        diagnostic.code().as_str(),
        marrow_codes::Code::CheckRequiresPresence.as_str(),
    );
    assert_eq!(diagnostic.file().as_str(), "src/main.mw");
    let span = diagnostic.span();
    assert_eq!(
        (
            span.line as usize,
            span.column as usize,
            span.start_byte,
            span.end_byte,
        ),
        (line, column, start, start + place.len()),
    );
}

#[test]
fn an_invalidated_required_group_leaf_refuses_an_optional_context() {
    let body = r#"pub fn pages(shelf: int, id: int): int? {
    transaction {
        place b = ^books[shelf, id]
        if exists(b) {
            delete b
            return b.details.pages
        }
        return absent
    }
}
"#;
    assert_requires_presence_with_schema(REQUIRED_LEAF_SCHEMA, body, "b.details.pages");
}

#[test]
fn a_whole_group_constructor_argument_erase_requires_fresh_presence() {
    let body = r#"fn eraseAndPages(shelf: int, id: int): int {
    delete ^books[shelf, id]
    return 77
}

pub fn put(shelf: int, id: int) {
    transaction {
        place b = ^books[shelf, id]
        if exists(b) {
            b.details = Book.details(pages: eraseAndPages(shelf, id))
        }
    }
}
"#;
    assert_requires_presence(body, "b.details");
}

#[test]
fn a_group_leaf_rhs_erase_requires_fresh_presence() {
    let body = r#"fn eraseAndPages(shelf: int, id: int): int {
    delete ^books[shelf, id]
    return 77
}

pub fn put(shelf: int, id: int) {
    transaction {
        place b = ^books[shelf, id]
        if exists(b) {
            b.details.pages = eraseAndPages(shelf, id)
        }
    }
}
"#;
    assert_requires_presence(body, "b.details.pages");
}

#[test]
fn replacement_only_group_rhs_runs_once_and_preserves_current_siblings() {
    let source = format!(
        "{SOURCE}\n{}",
        r#"fn replaceAndPages(shelf: int, id: int): int {
    if const count = ^books[9, 9].details.pages {
        ^books[9, 9] = Book(title: "calls", details: Book.details(pages: count + 1))
    }
    ^books[shelf, id] = Book(title: "replacement", details: Book.details(pages: 55, language: "fresh sibling"))
    return 77
}

pub fn replaceWhole(shelf: int, id: int) {
    transaction {
        place b = ^books[shelf, id]
        if exists(b) {
            b.details = Book.details(pages: replaceAndPages(shelf, id), language: "assigned")
        }
    }
}

pub fn replaceLeaf(shelf: int, id: int) {
    transaction {
        place b = ^books[shelf, id]
        if exists(b) {
            b.details.pages = replaceAndPages(shelf, id)
        }
    }
}
"#
    );
    for (name, language) in [
        ("replaceWhole", "assigned"),
        ("replaceLeaf", "fresh sibling"),
    ] {
        let mut session = Project::single(&source).ids(IDS).session();
        session.call(
            "setBook",
            vec![i(1), i(7), s("original"), i(41), s("old sibling")],
        );
        session.call("setBook", vec![i(9), i(9), s("calls"), i(0), s("unused")]);
        session.call(name, vec![i(1), i(7)]);
        assert_eq!(
            as_int(session.call("readPages", vec![i(9), i(9)])),
            Some(1),
            "{name}: the RHS runs once",
        );
        assert_eq!(
            as_int(session.call("readPages", vec![i(1), i(7)])),
            Some(77),
            "{name}: the assignment uses the RHS result",
        );
        assert_eq!(
            as_str(session.call("readTitle", vec![i(1), i(7)])),
            Some("replacement".to_string()),
            "{name}: group assignment preserves the entry replacement",
        );
        assert_eq!(
            as_str(session.call("readLanguage", vec![i(1), i(7)])),
            Some(language.to_string()),
            "{name}: leaf assignment preserves the sibling after RHS effects",
        );
        assert_eq!(
            as_int(session.call("readPages", vec![i(7), i(1)])),
            None,
            "{name}: the ordered composite key is preserved",
        );
    }
}
