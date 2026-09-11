use super::{assert_steers_to, diagnostics, project, rows, written_span};
use marrow_compile::{DeclarationNamespace, RefusalReport, RefusedDeclaration, compile};

#[test]
fn a_refused_generic_struct_occupies_its_name() {
    compile(&project(
        "module main\n\npub fn independent(): int {\n    return 7\n}\n",
    ))
    .expect("the independent function compiles without either struct");

    const SOURCE: &str = r#"module main

struct Box<T,T> { v: T }
struct Box<U> { v: U }
pub fn independent(): int {
    return 7
}
"#;
    let conflict = marrow_codes::Code::CheckNameConflict.as_str();
    let repeated_parameter = marrow_syntax::SourceSpan {
        start_byte: 26,
        end_byte: 27,
        line: 3,
        column: 14,
    };

    let renamed_source = SOURCE.replacen("struct Box<U>", "struct Other<U>", 1);
    let renamed = diagnostics(&renamed_source);
    assert_eq!(
        rows(&renamed),
        vec![("src/main.mw", conflict, 3, 14)],
        "renaming the second struct leaves only the repeated-parameter diagnostic",
    );
    assert_eq!(
        renamed[0].span(),
        repeated_parameter,
        "the renamed control preserves the repeated parameter's full span",
    );

    let collision = diagnostics(SOURCE);
    let actual: Vec<_> = collision
        .iter()
        .map(|row| (row.file().as_str(), row.code(), row.span()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("src/main.mw", conflict, repeated_parameter),
            (
                "src/main.mw",
                conflict,
                marrow_syntax::SourceSpan {
                    start_byte: 45,
                    end_byte: 48,
                    line: 4,
                    column: 8,
                },
            ),
        ],
        "a refused generic struct retains its name against a later declaration",
    );
}

#[test]
fn a_generic_enum_duplicate_preserves_the_original_refusal() {
    for (declaration, unknown, duplicate, annotation) in [
        (
            "enum Box<T> { item(value: Missing) }",
            (39, 46, 3, 27),
            (55, 58, 4, 6),
            (103, 111, 5, 23),
        ),
        (
            "struct Box<T> { value: Missing }",
            (36, 43, 3, 24),
            (51, 54, 4, 6),
            (99, 107, 5, 23),
        ),
    ] {
        let source = format!(
            "module main\n\n{declaration}\n\
             enum Box<U> {{ item(value: U) }}\n\
             pub fn inspect(value: Box<int>): int {{\n    return 7\n}}\n",
        );
        let diagnostics = diagnostics(&source);
        let actual: Vec<_> = diagnostics
            .iter()
            .map(|row| {
                let span = row.span();
                (
                    row.file().as_str(),
                    row.code(),
                    (span.start_byte, span.end_byte, span.line, span.column),
                )
            })
            .collect();
        let type_error = marrow_codes::Code::CheckType.as_str();
        assert_eq!(
            actual,
            vec![
                ("src/main.mw", type_error, unknown),
                (
                    "src/main.mw",
                    marrow_codes::Code::CheckNameConflict.as_str(),
                    duplicate,
                ),
                ("src/main.mw", type_error, annotation),
            ],
            "{declaration}",
        );
        assert_steers_to(
            &diagnostics,
            DeclarationNamespace::NamedType,
            type_error,
            RefusalReport::AtDeclaration,
        );
    }
}

#[test]
fn an_over_wide_enum_occupies_its_name() {
    const VALID: &str = "module main\n\nenum E { one }\n\
        pub fn inspect(value: E): int {\n    return 7\n}\n";
    compile(&project(VALID)).expect("the standalone enum and its annotation compile");

    let conflict = marrow_codes::Code::CheckNameConflict.as_str();
    let limit = marrow_codes::Code::CheckResourceLimit.as_str();
    let cause = RefusedDeclaration {
        namespace: Some(DeclarationNamespace::NamedType),
        declaring_code: limit,
        report: RefusalReport::AtDeclaration,
    };
    let accepted = VALID.replacen("enum E { one }", "enum E { zero }\nenum E { one }", 1);
    let accepted_duplicate = written_span("module main\n\nenum E { zero }\nenum E", "E");
    let variants: Vec<String> = (0..=marrow_image::bounds::MAX_VARIANTS)
        .map(|index| format!("    V{index}"))
        .collect();
    let wide = format!("module main\n\nenum E {{\n{}\n}}\n", variants.join("\n"));
    let declaration = written_span("module main\n\nenum E", "E");
    let duplicate = written_span(&format!("{wide}enum E"), "E");
    let source = format!(
        "{wide}enum E {{ one }}\n\
         pub fn inspect(value: E): int {{\n    return 7\n}}\n",
    );
    let renamed = source.replacen("enum E { one }", "enum Other { one }", 1);
    let renamed_use = written_span(&renamed, "E");
    let collision_use = written_span(&source, "E");

    for (source, expected, assertion) in [
        (
            accepted,
            vec![("src/main.mw", conflict, accepted_duplicate, None)],
            "an accepted enum reservation still excludes a duplicate before filling",
        ),
        (
            renamed,
            vec![
                ("src/main.mw", limit, declaration, None),
                ("src/main.mw", limit, renamed_use, Some(&cause)),
            ],
            "renaming only the second enum preserves the first refusal and its annotation steer",
        ),
        (
            source,
            vec![
                ("src/main.mw", limit, declaration, None),
                ("src/main.mw", conflict, duplicate, None),
                ("src/main.mw", limit, collision_use, Some(&cause)),
            ],
            "an over-wide enum retains its name and refusal against a later declaration",
        ),
    ] {
        let diagnostics = diagnostics(&source);
        let actual: Vec<_> = diagnostics
            .iter()
            .map(|row| {
                (
                    row.file().as_str(),
                    row.code(),
                    row.span(),
                    row.refused_declaration(),
                )
            })
            .collect();
        assert_eq!(actual, expected, "{assertion}");
    }
}
