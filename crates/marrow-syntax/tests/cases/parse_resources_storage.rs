//! Resource and store declarations: split forms, saved roots, index members,
//! and the key/member-name grammar rules they share.

use crate::common;
use common::{has_reason, parse_reason};
use marrow_syntax::{ExpectedSyntax, ParseDiagnosticReason, parse_source};

#[test]
fn parses_split_store_declaration() {
    let parsed = parse_source(
        "module app\n\
         resource Book {\n\
         \x20   required title: string\n\
         }\n\
         store ^books[id: int]: Book\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.resource("Book").is_some());
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(store.root.root, "books");
    assert_eq!(store.root.keys.len(), 1);
    assert_eq!(store.root.keys[0].name, "id");
    assert_eq!(store.root.keys[0].ty.to_string(), "int");
    assert_eq!(store.resource, "Book");
}

#[test]
fn malformed_resource_header_reports_the_resource_rule() {
    for source in [
        "module app\nresource Book extra {\n    title: string\n}\n",
        "module app\nresource Book ^ {\n    title: string\n}\n",
        "module app\nresource Book ^books() {\n    title: string\n}\n",
        "module app\nresource Book ^books(id:) {\n    title: string\n}\n",
        "module app\nresource Book ^books extra {\n    title: string\n}\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            has_reason(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::ResourceHeader
                ))
            ),
            "{:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn leading_keyed_layer_clause_keyword_reports_the_member_shape_rule() {
    // A store-body line beginning with a keyed-layer clause keyword such as
    // `unique` is not a member; it gets the same member-shape rule a non-keyword
    // junk word gets, not the bare "expected resource member name".
    let parsed = parse_source(
        "module app\n\
         resource User {\n\
         \x20   required email: string\n\
         }\n\
         store ^users[id: int]: User {\n\
         \x20   unique index byEmail[email]\n\
         }\n",
    );

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(
                ExpectedSyntax::ResourceMemberSyntax
            ))
        ),
        "{:#?}",
        parsed.diagnostics
    );
    assert!(
        !parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("expected resource member name")),
        "a leading clause keyword must not get the bare member-name message: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn trailing_keyed_layer_clause_after_a_type_names_the_stray_token() {
    for clause in ["retain", "counted", "unique"] {
        let source = format!(
            "module app\n\
             resource User {{\n\
             \x20   tags[pos: int]: string {clause}\n\
             }}\n\
             store ^users[id: int]: User\n"
        );
        let parsed = parse_source(&source);

        assert!(parsed.has_errors(), "{clause}: {:#?}", parsed.diagnostics);
        let message = parsed
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            !message.contains("expected field type after `:`"),
            "{clause}: the type is present, so the fault must not claim a missing type: {message}"
        );
        assert!(
            message.contains(clause),
            "{clause}: the fault names the stray clause token: {message}"
        );
    }
}

#[test]
fn a_genuinely_missing_field_type_still_reports_the_missing_type() {
    let parsed = parse_source(
        "module app\n\
         resource User {\n\
         \x20   tags[pos: int]:\n\
         }\n\
         store ^users[id: int]: User\n",
    );

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("expected field type after `:`")),
        "a genuinely missing type keeps the missing-type message: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn split_resource_body_rejects_index_members() {
    let parsed = parse_source(
        "module app\n\
         resource Book {\n\
         \x20   title: string\n\
         \x20   index byTitle[title]\n\
         }\n\
         store ^books[id: int]: Book\n",
    );

    assert!(parsed.has_errors(), "expected parse rejection");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::IndexOutsideStoreBody)
        ),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_tilde_prefixed_saved_roots() {
    for source in [
        "module app\ncache ~books[id: int]: Book\n",
        "module app\nensure ~books[id: int]: Book\n",
        "module app\nresource Book {\n    author: Id(~authors)\n}\n",
        "module app\n~scratch[id: int]: Book\n",
    ] {
        let parsed = parse_source(source);
        assert!(parsed.has_errors(), "expected rejection for:\n{source}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "parse.syntax"),
            "{:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_resource_members_nested_under_fields() {
    // A scalar field cannot introduce nested members: only a group opens a `{ … }`
    // child block. A field followed by a block is the unexpected-block fault.
    let parsed = parse_source(
        r#"module app
resource Book {
    title: string {
        nested: string
    }
}
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::UnexpectedIndentation)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_empty_saved_root_key_lists() {
    let parsed = parse_source(
        r#"module app
resource Book {
    title: string
}
store ^books[]: Book
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "parse.syntax"
                && diagnostic.reason == parse_reason(ParseDiagnosticReason::EmptyKeyParameters)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_empty_index_argument_lists() {
    let parsed = parse_source(
        r#"module app
resource Book {
    title: string
}
store ^books[id: int]: Book {
    index empty[]
}
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::EmptyIndexArguments)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn header_helper_errors_report_specific_expected_parts() {
    for (source, expected) in [
        (
            "module app\nenum 123 {\n    One\n}\n",
            ExpectedSyntax::EnumName,
        ),
        (
            "module app\nenum Status extra {\n    One\n}\n",
            ExpectedSyntax::EnumHeader,
        ),
        (
            "module app\nresource 123 {\n    title: string\n}\n",
            ExpectedSyntax::ResourceName,
        ),
        (
            "module app\nresource Book where ^books {\n    title: string\n}\n",
            ExpectedSyntax::ResourceHeader,
        ),
        (
            "module app\nresource Book extra books {\n    title: string\n}\n",
            ExpectedSyntax::ResourceHeader,
        ),
        (
            "module app\nresource Book ^ {\n    title: string\n}\n",
            ExpectedSyntax::ResourceHeader,
        ),
        ("module app\nstore books: Book\n", ExpectedSyntax::StoreRoot),
        ("module app\nstore ^: Book\n", ExpectedSyntax::SavedRootName),
        (
            "module app\nstore ^books:\n",
            ExpectedSyntax::StoreResourceName,
        ),
        (
            "module app\nstore ^books: Book {\n    index [title]\n}\n",
            ExpectedSyntax::IndexName,
        ),
        (
            "module app\nstore ^books: Book {\n    index byTitle[title] sparse\n}\n",
            ExpectedSyntax::IndexTail,
        ),
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            has_reason(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(expected))
            ),
            "expected {expected:?} for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn future_surface_words_as_resource_enum_or_store_root_names_are_rejected() {
    for word in ["journal", "sensitive", "declassify", "Id"] {
        let resource = parse_source(&format!(
            "module app\nresource {word} {{\n    title: string\n}}\n"
        ));
        assert!(
            has_reason(
                &resource.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::ResourceName
                ))
            ),
            "expected resource-name diagnostic for {word}: {:#?}",
            resource.diagnostics
        );

        let enum_source = parse_source(&format!("module app\nenum {word} {{\n    active\n}}\n"));
        assert!(
            has_reason(
                &enum_source.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::EnumName))
            ),
            "expected enum-name diagnostic for {word}: {:#?}",
            enum_source.diagnostics
        );

        let root = parse_source(&format!(
            "module app\nresource Book {{\n    title: string\n}}\nstore ^{word}: Book\n"
        ));
        assert!(
            has_reason(
                &root.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::SavedRootName
                ))
            ),
            "expected saved-root-name diagnostic for {word}: {:#?}",
            root.diagnostics
        );
    }
}

#[test]
fn rejects_malformed_index_field_paths() {
    for source in [
        "module app\nresource Book {\n    title: string\n}\nstore ^books[id: int]: Book {\n    index bad[title.]\n}\n",
        "module app\nresource Book {\n    title: string\n}\nstore ^books[id: int]: Book {\n    index bad[.title]\n}\n",
        "module app\nresource Book {\n    title: string\n}\nstore ^books[id: int]: Book {\n    index bad[title.*]\n}\n",
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::IndexFieldPath
                ))),
            "diagnostics for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn reserved_word_as_resource_member_name_is_rejected() {
    let parsed = parse_source("resource R {\n    while: int\n}\n");
    assert_eq!(parsed.diagnostics.len(), 1, "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics[0].reason
            == parse_reason(ParseDiagnosticReason::Expected(
                ExpectedSyntax::ResourceMemberName
            )),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

#[test]
fn future_surface_words_as_resource_member_names_are_rejected() {
    for word in ["journal", "sensitive", "declassify", "Id"] {
        let parsed = parse_source(&format!("resource R {{\n    {word}: int\n}}\n"));
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::ResourceMemberName
                ))),
            "expected member-name diagnostic for {word}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn reserved_word_as_key_parameter_name_is_rejected() {
    let parsed = parse_source("resource R {\n    e[while: string]: int\n}\n");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::KeyName))
        ),
        "expected a key-name parse error for the reserved-word key name: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn comment_lines_inside_a_multi_line_store_key_list_are_skipped() {
    let parsed = parse_source(
        "module app\n\
         resource Book {\n\
         \x20   required title: string\n\
         }\n\
         store ^books[\n\
         \x20   id: int, // the identity\n\
         \x20   // the shelf this book lives on\n\
         \x20   shelf: string,\n\
         ]: Book\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(
        store
            .root
            .keys
            .iter()
            .map(|key| (key.name.clone(), key.ty.to_string()))
            .collect::<Vec<_>>(),
        vec![
            ("id".to_string(), "int".to_string()),
            ("shelf".to_string(), "string".to_string()),
        ]
    );
}

#[test]
fn comment_lines_inside_a_multi_line_index_argument_list_are_skipped() {
    let parsed = parse_source(
        "module app\n\
         resource Book {\n\
         \x20   required title: string\n\
         \x20   shelf: int\n\
         }\n\
         store ^books[id: int]: Book {\n\
         \x20   index byShelf[\n\
         \x20       shelf, // primary order\n\
         \x20       // the identity breaks ties\n\
         \x20       id]\n\
         }\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let store = parsed.file.store("books").expect("books store");
    assert!(
        store
            .indexes
            .iter()
            .any(|index| index.name == "byShelf" && index.args == ["shelf", "id"]),
        "{:#?}",
        store.indexes
    );
}

#[test]
fn a_genuinely_missing_key_name_still_reports_a_key_name_error() {
    let parsed = parse_source(
        "module app\n\
         resource Book {\n\
         \x20   required title: string\n\
         }\n\
         store ^books[\n\
         \x20   id: int,\n\
         \x20   : string,\n\
         ]: Book\n",
    );
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::KeyName))
        ),
        "expected a key-name parse error for the nameless key: {:#?}",
        parsed.diagnostics
    );
}
