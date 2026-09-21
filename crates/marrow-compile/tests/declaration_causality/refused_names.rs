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

/// `_` declares nothing. Every position that takes a name — value, type, member,
/// durable, module and `use` — refuses it with exactly one `check.type` whose span is
/// the placeholder token, never a name conflict; the same source with `_x` in that
/// position is admitted. Only a `match` arm's payload position admits `_`.
#[test]
fn the_placeholder_declares_nothing_in_any_declaration_kind() {
    const BOOK: &str = "resource Book {\n    required title: string\n}\n\n";
    // An index name shares the root's namespace with its stored fields, so only the
    // component row's resource carries the `_x` field its sibling projects.
    const BOOK_X: &str = "resource Book {\n    required title: string\n    _x: int\n}\n\n";
    const MAYBE: &str = "fn maybe(): int? {\n    return 1\n}\n\n";
    let durable = |body: &str| format!("{BOOK}store ^books[id: int]: Book\n\n{body}");
    // Each case writes the placeholder once, as `@`, in the file path or the source.
    let cases: Vec<(&str, &str, String)> = vec![
        ("a module header segment", "src/a/@.mw", "module a::@\n\npub fn f(): int {\n    return 1\n}\n".into()),
        ("a use segment", "src/main.mw", "use a::@\n\npub fn f(): int {\n    return 1\n}\n".into()),
        ("a module constant", "src/main.mw", "const @ = 1\n".into()),
        ("a function", "src/main.mw", "fn @(): int {\n    return 1\n}\n".into()),
        ("a parameter", "src/main.mw", "fn f(@: int): int {\n    return 1\n}\n".into()),
        ("a function type parameter", "src/main.mw", "fn f<@>(x: int): int {\n    return x\n}\n".into()),
        ("a struct type parameter", "src/main.mw", "struct S<@> {\n    x: int\n}\n".into()),
        ("an enum type parameter", "src/main.mw", "enum E<@> {\n    a\n}\n".into()),
        ("a local constant", "src/main.mw", "fn f(): int {\n    const @ = 1\n    return 1\n}\n".into()),
        ("a local variable", "src/main.mw", "fn f(): int {\n    var @ = 1\n    return 1\n}\n".into()),
        ("a loop variable", "src/main.mw", "fn f(): int {\n    for @ in 0..3 {\n    }\n    return 1\n}\n".into()),
        ("an if const binding", "src/main.mw", format!("{MAYBE}fn f(): int {{\n    if const @ = maybe() {{\n        return 1\n    }}\n    return 0\n}}\n")),
        ("a chained if const binding", "src/main.mw", format!("{MAYBE}fn f(): int {{\n    if const a = maybe() and const @ = maybe() {{\n        return a\n    }}\n    return 0\n}}\n")),
        ("a let-else binding", "src/main.mw", format!("{MAYBE}fn f(): int {{\n    const @ = maybe() else {{\n        return 0\n    }}\n    return 1\n}}\n")),
        ("a checked var binding", "src/main.mw", "fn f(a: int, b: int): int {\n    var @ = checked a + b\n        on out_of_range {\n            return 0\n        }\n    return 1\n}\n".into()),
        ("an alias", "src/main.mw", "alias @ = int\n".into()),
        ("a nominal type", "src/main.mw", "type @: int in 0..1\n".into()),
        ("a struct", "src/main.mw", "struct @ {\n    x: int\n}\n".into()),
        ("a struct field", "src/main.mw", "struct S {\n    @: int\n}\n".into()),
        ("an enum", "src/main.mw", "enum @ {\n    a\n}\n".into()),
        ("an enum member", "src/main.mw", "enum E {\n    @\n}\n".into()),
        ("an enum payload field", "src/main.mw", "enum E {\n    a(@: int)\n}\n".into()),
        ("a resource", "src/main.mw", "resource @ {\n    required id: int\n}\n".into()),
        ("a resource field", "src/main.mw", "resource R {\n    @: int\n}\n".into()),
        ("a group", "src/main.mw", "resource R {\n    @ {\n        x: int\n    }\n}\n".into()),
        ("a branch", "src/main.mw", "resource R {\n    @[k: int] {\n        x: int\n    }\n}\n".into()),
        ("a branch key", "src/main.mw", "resource R {\n    notes[@: int] {\n        x: int\n    }\n}\n".into()),
        ("a nested field", "src/main.mw", "resource R {\n    notes[k: int] {\n        @: int\n    }\n}\n".into()),
        ("a nested group", "src/main.mw", "resource R {\n    notes[k: int] {\n        @ {\n            x: int\n        }\n    }\n}\n".into()),
        ("a nested branch", "src/main.mw", "resource R {\n    notes[k: int] {\n        @[j: int] {\n            x: int\n        }\n    }\n}\n".into()),
        ("a nested branch key", "src/main.mw", "resource R {\n    notes[k: int] {\n        sub[@: int] {\n            x: int\n        }\n    }\n}\n".into()),
        ("a store root", "src/main.mw", format!("{BOOK}store ^@[id: int]: Book\n")),
        ("a store key", "src/main.mw", format!("{BOOK}store ^books[@: int]: Book\n")),
        ("an index", "src/main.mw", format!("{BOOK}store ^books[id: int]: Book {{\n    index @[id]\n}}\n")),
        ("an index component", "src/main.mw", format!("{BOOK_X}store ^books[id: int]: Book {{\n    index byX[@, id]\n}}\n")),
        ("a place", "src/main.mw", durable("pub fn f(): int {\n    place @ = ^books[1]\n    return 0\n}\n")),
        ("a traversal pin", "src/main.mw", durable("pub fn f(): int {\n    for id, @ in ^books at most 1 {\n    } on more {\n    }\n    return 0\n}\n")),
    ];
    for (label, path, template) in cases {
        let at = template.find('@').expect("the placeholder is written once");
        let line = template[..at].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let column = (at - template[..at].rfind('\n').map_or(0, |nl| nl + 1)) as u32 + 1;
        let expected = marrow_syntax::SourceSpan {
            start_byte: at,
            end_byte: at + 1,
            line,
            column,
        };

        let project = with_minted_ids(&[(&path.replace('@', "_"), template.replace('@', "_"))]);
        let diagnostics = diagnostics_of(&project);
        let refusals: Vec<_> = diagnostics
            .iter()
            .filter(|row| row.code() == marrow_codes::Code::CheckType)
            .collect();
        assert_eq!(
            refusals.len(),
            1,
            "{label}: one `check.type` row refuses the placeholder: {:#?}",
            rows(&diagnostics)
        );
        assert_eq!(
            refusals[0].span(),
            expected,
            "{label}: reported at the placeholder token: {:#?}",
            rows(&diagnostics)
        );
        assert!(
            diagnostics
                .iter()
                .all(|row| row.code() != marrow_codes::Code::CheckNameConflict),
            "{label}: the placeholder is never a name conflict: {:#?}",
            rows(&diagnostics)
        );

        let sibling = with_minted_ids(&[(&path.replace('@', "_x"), template.replace('@', "_x"))]);
        let admitted = super::diagnostics_or_empty(&sibling);
        assert!(
            admitted.iter().all(|row| {
                row.code() != marrow_codes::Code::CheckType
                    && row.code() != marrow_codes::Code::CheckNameConflict
            }),
            "{label}: `_x` is admitted in the same position: {:#?}",
            rows(&admitted)
        );
    }
}

/// A manifest refuses `_` as a dependency alias with the alias's own reason.
#[test]
fn the_placeholder_is_not_a_dependency_alias() {
    let error = marrow_project::Manifest::parse(
        "edition = \"2026\"\n\n[dependencies]\n_ = { path = \"../lib\" }\n",
    )
    .expect_err("the placeholder alias is refused");
    assert_eq!(error.code(), marrow_codes::Code::ProjectDependencyAlias);
    assert_eq!(
        error.kind(),
        &marrow_project::ManifestErrorKind::DependencyAlias {
            alias: "_".to_string(),
            reason: marrow_project::DependencyAliasReason::Placeholder,
        }
    );
}

/// A checked form's binding name is validated before its operands are lowered: with
/// an operand that would report on its own, the placeholder and a reserved built-in
/// each report once, at the name, and the operand row never appears; the same operand
/// under an admitted name reports as usual.
#[test]
fn a_checked_binding_name_is_validated_before_its_operands() {
    let source = |name: &str| {
        format!(
            "pub fn f(a: int): int {{\n    const {name} = checked a + nope\n        on out_of_range {{\n            return 0\n        }}\n    return 1\n}}\n"
        )
    };
    let operand_column = "    const good = checked a + ".len() as u32 + 1;
    let control_rows = diagnostics(&source("good"));
    let control = rows(&control_rows);
    assert_eq!(control.len(), 1, "{control:#?}");
    assert_eq!(
        (control[0].2, control[0].3),
        (2, operand_column),
        "the unknown operand reports on its own: {control:#?}"
    );
    for (name, code) in [
        ("_", marrow_codes::Code::CheckType),
        ("ok", marrow_codes::Code::CheckNameConflict),
    ] {
        assert_eq!(
            rows(&diagnostics(&source(name))),
            vec![("src/main.mw", code, 2, 11)],
            "{name}"
        );
    }
}
