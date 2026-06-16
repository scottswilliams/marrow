//! Resource and store declarations: split forms, saved roots, index members,
//! and the key/member-name grammar rules they share.

use crate::common;
use common::{has_reason, parse_reason};
use marrow_syntax::{ExpectedSyntax, ParseDiagnosticReason, parse_source};

#[test]
fn parses_split_store_declaration() {
    let parsed = parse_source(
        "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.resource("Book").is_some());
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(store.root.root, "books");
    assert_eq!(store.root.keys.len(), 1);
    assert_eq!(store.root.keys[0].name, "id");
    assert_eq!(store.root.keys[0].ty.text, "int");
    assert_eq!(store.resource, "Book");
}

#[test]
fn malformed_resource_header_reports_the_resource_rule() {
    for source in [
        "module app\nresource Book extra\n    title: string\n",
        concat!("module app\nresource Book ", "^\n    title: string\n",),
        concat!(
            "module app\nresource Book ",
            "^books()\n    title: string\n",
        ),
        concat!(
            "module app\nresource Book ",
            "^books(id:)\n    title: string\n",
        ),
        concat!(
            "module app\nresource Book ",
            "^books extra\n    title: string\n",
        ),
    ] {
        let parsed = parse_source(source);
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason
                    == parse_reason(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::ResourceName,
                    ))
            })
            .expect("resource-head diagnostic");

        assert!(
            diagnostic.message.contains("resource header"),
            "{diagnostic:?}"
        );
    }
}

#[test]
fn split_resource_body_rejects_index_members() {
    let parsed = parse_source(
        "module app\n\
         resource Book\n\
         \x20   title: string\n\
         \x20   index byTitle(title)\n\
         store ^books(id: int): Book\n",
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
        "module app\ncache ~books(id: int): Book\n",
        "module app\nensure ~books(id: int): Book\n",
        "module app\nresource Book\n    author: Id(~authors)\n",
        "module app\n~scratch(id: int): Book\n",
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
    let parsed = parse_source(
        r#"module app
resource Book
    title: string
        nested: string
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
resource Book
    title: string
store ^books(): Book
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
resource Book
    title: string
store ^books(id: int): Book
    index empty()
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
        ("module app\nenum 123\n    One\n", ExpectedSyntax::EnumName),
        (
            "module app\nenum Status extra\n    One\n",
            ExpectedSyntax::EnumHeader,
        ),
        (
            "module app\nresource 123\n    title: string\n",
            ExpectedSyntax::ResourceName,
        ),
        (
            "module app\nresource Book where ^books\n    title: string\n",
            ExpectedSyntax::ResourceName,
        ),
        (
            concat!(
                "module app\nresource Book ",
                "extra books\n    title: string\n",
            ),
            ExpectedSyntax::ResourceName,
        ),
        (
            concat!("module app\nresource Book ", "^\n    title: string\n",),
            ExpectedSyntax::ResourceName,
        ),
        ("module app\nstore books: Book\n", ExpectedSyntax::StoreRoot),
        ("module app\nstore ^: Book\n", ExpectedSyntax::SavedRootName),
        (
            "module app\nstore ^books:\n",
            ExpectedSyntax::StoreResourceName,
        ),
        (
            "module app\nstore ^books: Book\n    index (title)\n",
            ExpectedSyntax::IndexName,
        ),
        (
            "module app\nstore ^books: Book\n    index byTitle(title) sparse\n",
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
        let resource = parse_source(&format!("module app\nresource {word}\n    title: string\n"));
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

        let enum_source = parse_source(&format!("module app\nenum {word}\n    active\n"));
        assert!(
            has_reason(
                &enum_source.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::EnumName))
            ),
            "expected enum-name diagnostic for {word}: {:#?}",
            enum_source.diagnostics
        );

        let root = parse_source(&format!(
            "module app\nresource Book\n    title: string\nstore ^{word}: Book\n"
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
        "module app\nresource Book\n    title: string\nstore ^books(id: int): Book\n    index bad(title.)\n",
        "module app\nresource Book\n    title: string\nstore ^books(id: int): Book\n    index bad(.title)\n",
        "module app\nresource Book\n    title: string\nstore ^books(id: int): Book\n    index bad(title.*)\n",
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
    let parsed = parse_source("resource R\n    while: int\n");
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
        let parsed = parse_source(&format!("resource R\n    {word}: int\n"));
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
    let parsed = parse_source("resource R\n    e(while: string): int\n");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::KeyName))
        ),
        "expected a key-name parse error for the reserved-word key name: {:#?}",
        parsed.diagnostics
    );
}
