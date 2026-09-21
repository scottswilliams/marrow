use super::{
    assert_steers_to, cause_facts, diagnostics, diagnostics_of, project, rows, with_minted_ids,
    written_span,
};
use marrow_compile::{DeclarationNamespace, RefusalReport, compile};

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
    let conflict = marrow_codes::Code::CheckNameConflict;
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

/// A written span as `(start_byte, end_byte, line, column)`.
type WrittenSpan = (usize, usize, u32, u32);

/// One refused declaration whose name a later duplicate reuses.
struct DuplicateCase {
    /// The refused declaration, written first.
    declaration: &'static str,
    /// The unknown type inside `declaration` that refuses it.
    unknown: WrittenSpan,
    /// The later declaration of the same name.
    duplicate: WrittenSpan,
    /// The annotation naming that name, which is steered to the first refusal.
    annotation: WrittenSpan,
}

/// Both declaration forms that can reserve the name `Box`: the original refusal, the
/// name conflict, and the steered annotation are reported in that order for each.
const DUPLICATE_CASES: &[DuplicateCase] = &[
    DuplicateCase {
        declaration: "enum Box<T> { item(value: Missing) }",
        unknown: (39, 46, 3, 27),
        duplicate: (55, 58, 4, 6),
        annotation: (103, 111, 5, 23),
    },
    DuplicateCase {
        declaration: "struct Box<T> { value: Missing }",
        unknown: (36, 43, 3, 24),
        duplicate: (51, 54, 4, 6),
        annotation: (99, 107, 5, 23),
    },
];

#[test]
fn a_generic_enum_duplicate_preserves_the_original_refusal() {
    for case in DUPLICATE_CASES {
        let declaration = case.declaration;
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
        let type_error = marrow_codes::Code::CheckType;
        assert_eq!(
            actual,
            vec![
                ("src/main.mw", type_error, case.unknown),
                (
                    "src/main.mw",
                    marrow_codes::Code::CheckNameConflict,
                    case.duplicate,
                ),
                ("src/main.mw", type_error, case.annotation),
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

    let conflict = marrow_codes::Code::CheckNameConflict;
    let limit = marrow_codes::Code::CheckResourceLimit;
    let cause = (
        DeclarationNamespace::NamedType,
        limit,
        RefusalReport::AtDeclaration,
    );
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
                ("src/main.mw", limit, renamed_use, Some(cause)),
            ],
            "renaming only the second enum preserves the first refusal and its annotation steer",
        ),
        (
            source,
            vec![
                ("src/main.mw", limit, declaration, None),
                ("src/main.mw", conflict, duplicate, None),
                ("src/main.mw", limit, collision_use, Some(cause)),
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
                    cause_facts(row),
                )
            })
            .collect();
        assert_eq!(actual, expected, "{assertion}");
    }
}

/// `_` declares nothing: every declaration kind — value, type, member and durable —
/// refuses it with one `check.type` on the placeholder's line, never a name conflict,
/// and only a `match` arm's payload position admits it.
#[test]
fn the_placeholder_declares_nothing_in_any_declaration_kind() {
    const DURABLE: &str = "resource Book {\n    required title: string\n}\n\n";
    let cases = [
        ("a module constant", "const _ = 1\n".to_string()),
        ("a function", "fn _(): int {\n    return 1\n}\n".to_string()),
        (
            "a parameter",
            "fn f(_: int): int {\n    return 1\n}\n".to_string(),
        ),
        (
            "a type parameter",
            "fn f<_>(x: int): int {\n    return x\n}\n".to_string(),
        ),
        (
            "a local constant",
            "fn f(): int {\n    const _ = 1\n    return 1\n}\n".to_string(),
        ),
        (
            "a local variable",
            "fn f(): int {\n    var _ = 1\n    return 1\n}\n".to_string(),
        ),
        (
            "a loop variable",
            "fn f(): int {\n    for _ in 0..3 {\n    }\n    return 1\n}\n".to_string(),
        ),
        ("an alias", "alias _ = int\n".to_string()),
        ("a nominal type", "type _: int in 0..1\n".to_string()),
        ("a struct", "struct _ {\n    x: int\n}\n".to_string()),
        ("a struct field", "struct S {\n    _: int\n}\n".to_string()),
        ("an enum", "enum _ {\n    a\n}\n".to_string()),
        ("an enum member", "enum E {\n    _\n}\n".to_string()),
        (
            "an enum payload field",
            "enum E {\n    a(_: int)\n}\n".to_string(),
        ),
        (
            "a resource",
            "resource _ {\n    required id: int\n}\n".to_string(),
        ),
        (
            "a resource field",
            "resource R {\n    _: int\n}\n".to_string(),
        ),
        (
            "a group",
            "resource R {\n    _ {\n        x: int\n    }\n}\n".to_string(),
        ),
        (
            "a branch",
            "resource R {\n    _[k: int] {\n        x: int\n    }\n}\n".to_string(),
        ),
        (
            "a branch key",
            "resource R {\n    notes[_: int] {\n        x: int\n    }\n}\n".to_string(),
        ),
        (
            "a store root",
            format!("{DURABLE}store ^_[id: int]: Book\n"),
        ),
        (
            "a store key",
            format!("{DURABLE}store ^books[_: int]: Book\n"),
        ),
        (
            "an index",
            format!("{DURABLE}store ^books[id: int]: Book {{\n    index _[id]\n}}\n"),
        ),
    ];
    for (label, source) in cases {
        let at = source.rfind('_').expect("the placeholder is written once");
        let line = source[..at].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let project = with_minted_ids(&[("src/main.mw", source.clone())]);
        let diagnostics = diagnostics_of(&project);
        let rows = rows(&diagnostics);
        let placeholder: Vec<_> = rows
            .iter()
            .filter(|(_, code, _, _)| *code == marrow_codes::Code::CheckType)
            .collect();
        assert_eq!(
            placeholder.len(),
            1,
            "{label}: one `check.type` row refuses the placeholder: {rows:#?}"
        );
        assert_eq!(
            placeholder[0].2, line,
            "{label}: reported on the placeholder's line: {rows:#?}"
        );
        assert!(
            rows.iter()
                .all(|(_, code, _, _)| *code != marrow_codes::Code::CheckNameConflict),
            "{label}: the placeholder is never a name conflict: {rows:#?}"
        );
    }
}

/// A checked form's binding name is validated before its operands are lowered: the
/// placeholder and a reserved built-in each report once, at the name, and nothing is
/// reported for the operands.
#[test]
fn a_checked_binding_name_is_validated_before_its_operands() {
    for (name, code) in [
        ("_", marrow_codes::Code::CheckType),
        ("ok", marrow_codes::Code::CheckNameConflict),
    ] {
        let source = format!(
            "pub fn f(a: int, b: int): int {{\n    const {name} = checked a + b\n        on out_of_range {{\n            return 0\n        }}\n    return 1\n}}\n"
        );
        assert_eq!(
            rows(&diagnostics(&source)),
            vec![("src/main.mw", code, 2, 11)],
            "{name}"
        );
    }
}
