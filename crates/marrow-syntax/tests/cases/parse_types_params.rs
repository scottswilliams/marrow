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
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::ParameterModes,
                ))),
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
fn parses_alias_declarations() {
    let parsed = parse_source("module app\nalias Count = int\nalias MaybeCount = Count?\n");

    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let aliases: Vec<_> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|decl| match decl {
            marrow_syntax::Declaration::Alias(alias) => Some(alias),
            _ => None,
        })
        .collect();
    assert_eq!(aliases.len(), 2);
    assert_eq!(aliases[0].name, "Count");
    assert_eq!(
        aliases[0].ty.as_ref().map(ToString::to_string).as_deref(),
        Some("int")
    );
    assert_eq!(aliases[1].name, "MaybeCount");
    assert_eq!(
        aliases[1].ty.as_ref().map(ToString::to_string).as_deref(),
        Some("Count?")
    );
}

#[test]
fn alias_names_and_targets_are_validated() {
    // A keyword name, a missing `=`, and a missing target each report one typed
    // expectation at the header line, and parsing stays total.
    for (source, expected) in [
        (
            "module app\nalias int = string\n",
            ExpectedSyntax::AliasName,
        ),
        (
            "module app\nalias Title string\n",
            ExpectedSyntax::AliasType,
        ),
        ("module app\nalias Title =\n", ExpectedSyntax::AliasType),
    ] {
        let parsed = parse_source(source);
        assert!(parsed.has_errors(), "expected error for:\n{source}");
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            })
            .unwrap_or_else(|| panic!("expected {expected:?} for:\n{source}"));
        assert_eq!(diagnostic.code, "parse.syntax");
        assert_eq!(diagnostic.span.line, 2);
    }
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
fn rejects_trailing_tokens_after_a_complete_type_annotation() {
    // A complete type ends at its canonical word; a following token (`in`,
    // `where`, or a second bare word) is not part of the type. The parser must
    // point at that token rather than gluing it into a fabricated type spelling.
    for (source, expected, offender) in [
        (
            "module app\nconst ratio: decimal in 0.0..=1.0 = 0.5\n",
            ExpectedSyntax::ConstType,
            "in",
        ),
        (
            "module app\nfn f()\n    var x: int in 0..=5 = 1\n",
            ExpectedSyntax::ParameterType,
            "in",
        ),
        (
            "module app\nfn f(x: int where y): int\n    return 1\n",
            ExpectedSyntax::ParameterType,
            "where",
        ),
        (
            "module app\nfn f(): int where y\n    return 1\n",
            ExpectedSyntax::FunctionReturnType,
            "where",
        ),
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code == "parse.syntax"
                    && diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            })
            .unwrap_or_else(|| panic!("expected a {expected:?} diagnostic for {source}"));
        // The offender stands as its own word; search the spaced form so the
        // probe does not match a substring inside the preceding type (`int`).
        let offender_byte = source
            .find(&format!(" {offender} "))
            .or_else(|| source.find(&format!(" {offender}\n")))
            .map(|byte| byte + 1)
            .expect("offender token in source");
        assert_eq!(
            diagnostic.span.start_byte, offender_byte,
            "diagnostic should point at `{offender}` for {source}: {:#?}",
            diagnostic
        );
        // No fabricated glued spelling: the offending token must never be folded
        // into a type-annotation text.
        assert!(
            parsed.file.function("f").is_none_or(|function| function
                .params
                .iter()
                .all(|param| !param.ty.to_string().contains(offender))),
            "fabricated glued param type for {source}"
        );
    }
}

#[test]
fn signature_parse_errors_point_at_the_offending_token_not_column_one() {
    // A missing or misplaced parameter type and a missing return type each report
    // at the offending signature token, so two signature faults on one line are
    // distinguishable rather than both collapsing to the declaration column.
    for (source, expected, offender) in [
        // Bare word where a `: type` annotation is expected: point at the word.
        (
            "module app\nfn f(a int): int\n    return 1\n",
            ExpectedSyntax::ParameterType,
            "int)",
        ),
        // Parameter with no annotation at all: point at the parameter name.
        (
            "module app\nfn f(a):\n    return\n",
            ExpectedSyntax::ParameterType,
            "a)",
        ),
        // Colon with no return type after it: point at the trailing colon.
        (
            "module app\nfn f(a: int):\n    return\n",
            ExpectedSyntax::FunctionReturnType,
            ":\n",
        ),
        // A stray word trailing a complete return type: point at the misplaced word.
        (
            "module app\nfn f(a: int): int extra\n    return 1\n",
            ExpectedSyntax::FunctionReturnType,
            "extra",
        ),
        // The double-optional return spelling `T??`: point at the `??`.
        (
            "module app\nfn f(a: int): string??\n    return\n",
            ExpectedSyntax::FunctionReturnType,
            "??",
        ),
        // `= default` after a return type: point at the offending `=`.
        (
            "module app\nfn f(a: int): int = 3\n    return 1\n",
            ExpectedSyntax::FunctionReturnType,
            "= 3",
        ),
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code == "parse.syntax"
                    && diagnostic.reason == parse_reason(ParseDiagnosticReason::Expected(expected))
            })
            .unwrap_or_else(|| panic!("expected a {expected:?} diagnostic for {source}"));
        // The offender is located by the two-character context that anchors it,
        // and the diagnostic must point at that first character, never column 1.
        let offender_byte = source.find(offender).expect("offender token in source");
        assert_eq!(
            diagnostic.span.start_byte, offender_byte,
            "diagnostic should point at `{offender}` for {source}: {diagnostic:#?}"
        );
        assert_ne!(
            diagnostic.span.column, 1,
            "diagnostic collapsed to the declaration column for {source}: {diagnostic:#?}"
        );
    }
}

#[test]
fn valid_signature_with_types_and_return_parses() {
    let parsed = parse_source("module app\nfn f(a: int, b: string): bool\n    return true\n");

    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let function = parsed.file.function("f").expect("function f");
    assert_eq!(function.params.len(), 2);
    assert_eq!(function.params[0].ty.to_string(), "int");
    assert_eq!(function.params[1].ty.to_string(), "string");
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
    assert_eq!(function.params[0].ty.to_string(), "FutureBox[string,int]");
    assert_eq!(
        function
            .return_type
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("FutureBox[string,int]")
    );

    let book = parsed.file.resource("Book").expect("Book resource");
    let ResourceMember::Field(scores) = &book.members[0] else {
        panic!("expected scores field, got {:#?}", book.members[0]);
    };
    assert_eq!(scores.keys[0].ty.to_string(), "FutureBox[string,int]");
    assert_eq!(scores.ty.to_string(), "sequence[]");
}

#[test]
fn keyed_collection_parameter_carries_key_and_value_types() {
    let parsed = parse_source(
        "module app\n\
         fn total(scores(player: string): int): int\n\
         \x20   return 0\n",
    );
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);

    let function = parsed.file.function("total").expect("function total");
    let param = &function.params[0];
    assert_eq!(param.name, "scores");
    assert_eq!(param.ty.to_string(), "int");
    assert_eq!(param.keys.len(), 1);
    assert_eq!(param.keys[0].name, "player");
    assert_eq!(param.keys[0].ty.to_string(), "string");
}

#[test]
fn composite_keyed_collection_parameter_carries_each_key() {
    let parsed = parse_source(
        "module app\n\
         fn count(grid(row: int, col: int): bool): int\n\
         \x20   return 0\n",
    );
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);

    let function = parsed.file.function("count").expect("function count");
    let keys = &function.params[0].keys;
    assert_eq!(keys.len(), 2);
    assert_eq!(
        (keys[0].name.as_str(), keys[0].ty.to_string()),
        ("row", "int".to_string())
    );
    assert_eq!(
        (keys[1].name.as_str(), keys[1].ty.to_string()),
        ("col", "int".to_string())
    );
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

#[test]
fn future_surface_words_as_parameter_names_are_rejected() {
    for word in ["journal", "sensitive", "declassify", "Id"] {
        let parsed = parse_source(&format!("fn f({word}: int)\n    return\n"));
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::ParameterName
                ))),
            "expected parameter-name diagnostic for {word}: {:#?}",
            parsed.diagnostics
        );
    }
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
        .map(|param| (param.name.clone(), param.ty.to_string(), param.docs.clone()))
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

#[test]
fn own_line_comment_inside_a_multi_line_parameter_list_is_skipped() {
    // A `;` comment inside open delimiters does not close the list; it is skipped
    // like a blank line, so the parameters around it parse normally.
    let source = "module app\nfn f(\n    a: int,\n    ; explaining the next one\n    b: string,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![
            ("a".to_string(), "int".to_string(), Vec::new()),
            ("b".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn trailing_comment_after_a_parameter_is_skipped() {
    let source = "module app\nfn f(\n    a: int, ; first\n    b: string, ; second\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![
            ("a".to_string(), "int".to_string(), Vec::new()),
            ("b".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn comment_lines_do_not_disturb_parameter_docs() {
    // A `;` comment carries no documentation, while a `;;` run above a parameter
    // still attaches; the two must not interfere when interleaved.
    let source = concat!(
        "module app\nfn f(\n",
        "    ; an ordinary note\n",
        "    ;; the book to file\n",
        "    book: int,\n",
        ")\n    return\n",
    );
    assert_eq!(
        param_shape(source),
        vec![(
            "book".to_string(),
            "int".to_string(),
            vec!["the book to file".to_string()],
        )]
    );
}

/// A nominal `type` declaration parses into its named parts: name, base type,
/// `in` range expression, and `supports` capability list, with total recovery
/// for each malformed piece.
#[test]
fn parses_nominal_type_declarations() {
    use marrow_syntax::{Declaration, parse_source, range_expr};

    let parsed = parse_source(
        "module app\ntype Age: int in 0..=150 supports add, subtract\ntype Percent: int in 0..101\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let nominals: Vec<_> = parsed
        .file
        .declarations
        .iter()
        .filter_map(|decl| match decl {
            Declaration::Nominal(nominal) => Some(nominal),
            _ => None,
        })
        .collect();
    assert_eq!(nominals.len(), 2);
    assert_eq!(nominals[0].name, "Age");
    assert_eq!(
        nominals[0]
            .base
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("int")
    );
    let range = range_expr(nominals[0].interval.as_ref().expect("interval"))
        .expect("interval parses as a range");
    assert!(range.inclusive_end);
    let caps: Vec<&str> = nominals[0]
        .supports
        .iter()
        .map(|support| support.name.as_str())
        .collect();
    assert_eq!(caps, ["add", "subtract"]);
    assert_eq!(nominals[1].name, "Percent");
    assert!(nominals[1].supports.is_empty());
    let range = range_expr(nominals[1].interval.as_ref().expect("interval"))
        .expect("interval parses as a range");
    assert!(!range.inclusive_end);
}

/// Each malformed nominal header piece reports one diagnostic and keeps the
/// declaration node (total parsing): a keyword name, a missing `in` interval,
/// and a malformed `supports` tail.
#[test]
fn nominal_type_declaration_recovers_totally() {
    use marrow_syntax::{Declaration, parse_source};

    for (source, expected_message) in [
        (
            "type fn: int in 0..1\n",
            "`fn` is a keyword and cannot be used as a type name",
        ),
        ("type Age: int\n", "requires an `in lo..hi` interval"),
        ("type Age: int in\n", "expected an interval"),
        ("type Age in 0..1\n", "expected `:` and a base type"),
        (
            "type Age: int in 0..1 supports\n",
            "expected a capability list",
        ),
        (
            "type Age: int in 0..1 supports add,\n",
            "expected a capability name after `,`",
        ),
        (
            "type Age: int in 0..1 supports add add\n",
            "capability names separated by commas",
        ),
    ] {
        let parsed = parse_source(source);
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|d| d.message.contains(expected_message)),
            "missing {expected_message:?} for {source:?}: {:#?}",
            parsed.diagnostics
        );
        assert!(
            parsed
                .file
                .declarations
                .iter()
                .any(|decl| matches!(decl, Declaration::Nominal(_))),
            "declaration node must survive recovery for {source:?}"
        );
        // One diagnostic per defect, not a cascade.
        assert!(
            parsed.diagnostics.len() <= 2,
            "no cascade for {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

/// The formatter renders a nominal declaration canonically and idempotently,
/// including the docs, interval spelling, and capability list.
#[test]
fn formats_nominal_type_declarations() {
    use marrow_syntax::format_source;

    let source = "module app\n\ntype Age:   int   in 0..=150   supports add,subtract\n";
    let formatted = format_source(source);
    assert_eq!(
        formatted,
        "module app\n\ntype Age: int in 0..=150 supports add, subtract\n"
    );
    let reparsed = format_source(&formatted);
    assert_eq!(formatted, reparsed, "formatting must be idempotent");
}
