//! Function signatures: parameter lists across comma, newline, and mixed
//! surfaces, parameter docs, type annotations, and the surfaces a v0.1 function
//! signature rejects (defaults, generics, removed parameter modes, type aliases).

use crate::common;
use common::parse_reason;
use marrow_syntax::{
    Diagnose, ExpectedSyntax, ParseDiagnosticReason, ResourceMember, UnsupportedSyntax,
    parse_source,
};

#[test]
fn rejects_parameter_defaults() {
    let parsed = parse_source("module app\nfn f(x: int = 5)\n    return\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::ParameterDefaults,
                ))
        })
        .expect("expected parameter-defaults diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.kind(), "parse");
    assert_eq!(diagnostic.span.line, 2);
    assert!(
        diagnostic.reason
            != parse_reason(ParseDiagnosticReason::Expected(
                ExpectedSyntax::ParameterType
            )),
        "diagnostic should not fall back to a generic message, got {:?}",
        diagnostic.message
    );
}

#[test]
fn parameter_equal_classifies_defaults_separately_from_nested_type_syntax() {
    let default = parse_source("module app\nfn f(x: int = 5)\n    return\n");
    assert!(
        default
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::ParameterDefaults,
                ))),
        "{:#?}",
        default.diagnostics
    );
    assert!(
        !default
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::ParameterType,
                ))),
        "{:#?}",
        default.diagnostics
    );

    let nested = parse_source("module app\nfn f(x: sequence[a = b])\n    return\n");
    assert!(
        nested.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Expected(
                ExpectedSyntax::ParameterType,
            ))),
        "{:#?}",
        nested.diagnostics
    );
    assert!(
        !nested.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Unsupported(
                UnsupportedSyntax::ParameterDefaults,
            ))),
        "{:#?}",
        nested.diagnostics
    );
}

#[test]
fn removed_parameter_modes_are_rejected() {
    for source in [
        "module app\nfn parseInt(text: string, out value: int): bool\n    return true\n",
        "module app\nfn parseInt(text: string, inout value: int): bool\n    return true\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            parsed.has_errors(),
            "expected removed parameter mode rejection"
        );
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("parameter modes were removed")
                && diagnostic.message.contains("return the new value")),
            "{:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn out_and_inout_parse_as_ordinary_parameter_names() {
    assert_eq!(
        param_shape("module app\nfn f(out: int, inout: string)\n    return\n"),
        vec![
            ("out".to_string(), "int".to_string(), Vec::new()),
            ("inout".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn rejects_user_defined_generics_on_functions() {
    let parsed = parse_source("module app\nfn f<T>(x: T)\n    return\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::UserDefinedGenerics,
                ))
        })
        .expect("expected user-defined-generics diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.span.line, 2);
}

#[test]
fn rejects_top_level_type_aliases() {
    let parsed = parse_source("module app\ntype Title = string\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::TypeAliases,
                ))
        })
        .expect("expected type-aliases diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.span.line, 2);
}

#[test]
fn rejects_malformed_type_annotations() {
    // Each malformed-type position carries its own diagnostic, so pairing every
    // source with its specific message selects the malformed-type error rather
    // than any diagnostic whose text happens to contain "type".
    for (source, expected) in [
        ("module app\nconst Max: = 1\n", ExpectedSyntax::ConstType),
        (
            "module app\nfn main(value:)\n    return\n",
            ExpectedSyntax::ParameterType,
        ),
        (
            "module app\nresource Book\n    title: string\nstore ^books(id:): Book\n",
            ExpectedSyntax::KeyType,
        ),
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "parse.syntax"
                    && diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            }),
            "expected a parse.syntax diagnostic with {expected:?} for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_structural_equal_inside_type_annotations() {
    for (source, expected) in [
        (
            "module app\nconst Max: sequence[a = b] = 1\n",
            ExpectedSyntax::ConstType,
        ),
        (
            "module app\nfn f(): int = 1\n    return 1\n",
            ExpectedSyntax::FunctionReturnType,
        ),
        (
            "module app\nresource Book\n    title: string = 1\n",
            ExpectedSyntax::FieldType,
        ),
        (
            "module app\nresource Book\n    scores(k: int = 1): string\n",
            ExpectedSyntax::KeyType,
        ),
        (
            "module app\nresource Book\n    title: string\nstore ^books(id: int = 1): Book\n",
            ExpectedSyntax::KeyType,
        ),
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "parse.syntax"
                    && diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            }),
            "expected a parse.syntax diagnostic with {expected:?} for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn parser_preserves_type_spellings_for_downstream_resolution() {
    let parsed = parse_source(
        "module app\n\
         fn f(rows: FutureBox[string, int]): FutureBox[string, int]\n\
         \x20   return 1\n\
         resource Book\n\
         \x20   scores(k: FutureBox[string, int]): sequence[]\n",
    );
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);

    let function = parsed.file.function("f").expect("function f");
    assert_eq!(function.params[0].ty.text, "FutureBox[string,int]");
    assert_eq!(
        function.return_type.as_ref().map(|ty| ty.text.as_str()),
        Some("FutureBox[string,int]")
    );

    let book = parsed.file.resource("Book").expect("Book resource");
    let ResourceMember::Field(scores) = &book.members[0] else {
        panic!("expected scores field, got {:#?}", book.members[0]);
    };
    assert_eq!(scores.keys[0].ty.text, "FutureBox[string,int]");
    assert_eq!(scores.ty.text, "sequence[]");
}

#[test]
fn reserved_word_as_parameter_name_is_rejected() {
    let parsed = parse_source("fn f(while: int)\n    return\n");
    assert_eq!(parsed.diagnostics.len(), 1, "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics[0].reason
            == parse_reason(ParseDiagnosticReason::Expected(
                ExpectedSyntax::ParameterName
            )),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

/// `(name, ty, docs)` triples for every parameter of a single function, for
/// comparing parameter lists across the comma, newline, and mixed surfaces.
fn param_shape(source: &str) -> Vec<(String, String, Vec<String>)> {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    parsed
        .file
        .function("f")
        .expect("function f")
        .params
        .iter()
        .map(|param| {
            (
                param.name.clone(),
                param.ty.text.clone(),
                param.docs.clone(),
            )
        })
        .collect()
}

#[test]
fn single_line_parameter_list_parses_unchanged() {
    assert_eq!(
        param_shape("module app\nfn f(a: int, b: string)\n    return\n"),
        vec![
            ("a".to_string(), "int".to_string(), Vec::new()),
            ("b".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn multi_line_parameter_list_without_commas_matches_single_line() {
    let newline_separated = "module app\nfn f(\n    a: int\n    b: string\n)\n    return\n";
    assert_eq!(
        param_shape(newline_separated),
        param_shape("module app\nfn f(a: int, b: string)\n    return\n")
    );
}

#[test]
fn multi_line_parameter_list_with_trailing_commas_matches_single_line() {
    let comma_separated = "module app\nfn f(\n    a: int,\n    b: string,\n)\n    return\n";
    assert_eq!(
        param_shape(comma_separated),
        param_shape("module app\nfn f(a: int, b: string)\n    return\n")
    );
}

#[test]
fn mixed_comma_and_newline_separators_parse_identically() {
    let mixed = "module app\nfn f(\n    a: int,\n    b: string\n    c: bool,\n)\n    return\n";
    assert_eq!(
        param_shape(mixed),
        vec![
            ("a".to_string(), "int".to_string(), Vec::new()),
            ("b".to_string(), "string".to_string(), Vec::new()),
            ("c".to_string(), "bool".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn single_doc_line_above_a_parameter_is_captured() {
    let source = "module app\nfn f(\n    ;; the book to file\n    book: int,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![(
            "book".to_string(),
            "int".to_string(),
            vec!["the book to file".to_string()],
        )]
    );
}

#[test]
fn stacked_doc_lines_are_captured_in_order() {
    let source =
        "module app\nfn f(\n    ;; first line\n    ;; second line\n    book: int,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![(
            "book".to_string(),
            "int".to_string(),
            vec!["first line".to_string(), "second line".to_string()],
        )]
    );
}

#[test]
fn a_parameter_without_a_doc_has_empty_docs() {
    let source =
        "module app\nfn f(\n    ;; documented\n    a: int,\n    b: string,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![
            (
                "a".to_string(),
                "int".to_string(),
                vec!["documented".to_string()]
            ),
            ("b".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn multi_line_call_arguments_still_parse() {
    // A multi-line call-argument list is governed by the same delimiter-newline
    // suppression; documenting parameters must not regress it.
    let parsed =
        parse_source("module app\nfn f()\n    print(\n        1,\n        2,\n        3,\n    )\n");
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
}

#[test]
fn parameter_type_wrapped_inside_brackets_stays_one_parameter() {
    // A type may span physical lines inside its brackets; the line break sits at
    // a depth above the parameter list, so it must not split the parameter.
    let source = "module app\nfn f(\n    rows: sequence[\n        Book\n    ]\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![("rows".to_string(), "sequence[Book]".to_string(), Vec::new(),)]
    );
}

#[test]
fn parameter_with_wrapped_bracketed_type_and_a_following_parameter_parses_both() {
    let source = "module app\nfn f(\n    rows: sequence[\n        Book\n    ]\n    shelf: string\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![
            ("rows".to_string(), "sequence[Book]".to_string(), Vec::new(),),
            ("shelf".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn trailing_doc_with_no_following_parameter_is_reported() {
    // A dangling `;;` run after the last parameter documents nothing; it must be
    // reported rather than silently dropped.
    let source = "module app\nfn f(\n    a: int,\n    ;; orphaned doc\n)\n    return\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|d| d.reason == parse_reason(ParseDiagnosticReason::DocCommentBeforeParameter))
        .expect("a diagnostic for the orphaned doc comment");
    assert_eq!(diagnostic.code, "parse.syntax");
}
