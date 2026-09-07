//! Group writes consume presence after all RHS effects and evaluate the RHS once.

use super::{
    IDS, SCHEMA, SOURCE, as_int, as_str, attach, compile_diagnostics, compile_verify, i, run, s,
};

fn assert_requires_presence(body: &str, place: &str) {
    let source = format!("{SCHEMA}\n{body}");
    let start = source.find(place).expect("the fixture contains the write");
    let line = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let column = source[..start]
        .rfind('\n')
        .map_or(start + 1, |newline| start - newline);
    let diagnostics = compile_diagnostics(body);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let diagnostic = &diagnostics[0];
    assert_eq!(
        diagnostic.code(),
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
    let image = compile_verify(&source, IDS);
    for (name, language) in [
        ("replaceWhole", "assigned"),
        ("replaceLeaf", "fresh sibling"),
    ] {
        let mut store = attach(&image);
        run(
            &image,
            &mut store,
            "setBook",
            vec![i(1), i(7), s("original"), i(41), s("old sibling")],
        );
        run(
            &image,
            &mut store,
            "setBook",
            vec![i(9), i(9), s("calls"), i(0), s("unused")],
        );
        run(&image, &mut store, name, vec![i(1), i(7)]);
        assert_eq!(
            as_int(run(&image, &mut store, "readPages", vec![i(9), i(9)])),
            Some(1),
            "{name}: the RHS runs once",
        );
        assert_eq!(
            as_int(run(&image, &mut store, "readPages", vec![i(1), i(7)])),
            Some(77),
            "{name}: the assignment uses the RHS result",
        );
        assert_eq!(
            as_str(run(&image, &mut store, "readTitle", vec![i(1), i(7)])),
            Some("replacement".to_string()),
            "{name}: group assignment preserves the entry replacement",
        );
        assert_eq!(
            as_str(run(&image, &mut store, "readLanguage", vec![i(1), i(7)])),
            Some(language.to_string()),
            "{name}: leaf assignment preserves the sibling after RHS effects",
        );
        assert_eq!(
            as_int(run(&image, &mut store, "readPages", vec![i(7), i(1)])),
            None,
            "{name}: the ordered composite key is preserved",
        );
    }
}
