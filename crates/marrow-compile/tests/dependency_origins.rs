//! Compiling a project that reuses a local source dependency.
//!
//! Every case captures two trees through the one production `capture_origins` call
//! and drives the production `check`, so what is asserted is what `marrow check`
//! would report over the same two directories.

use marrow_compile::{CompileFailure, SourceDiagnostic, check};
use marrow_project::ProjectInput;

#[path = "common/project.rs"]
mod project_capture;

#[path = "common/ledger.rs"]
mod ledger;

/// The diagnostics a check refused with. A source-triggered refusal stays a
/// diagnostic failure; any other arm is a compiler-coherence fault.
fn diagnostics(project: &ProjectInput) -> Vec<SourceDiagnostic> {
    match check(project) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:#?}"),
    }
}

/// The typed code and rendered message of every diagnostic, in compiler order.
fn codes_and_messages(project: &ProjectInput) -> Vec<(&'static str, String)> {
    diagnostics(project)
        .iter()
        .map(|row| (row.code().as_str(), row.message().to_string()))
        .collect()
}

const TEXT_LIBRARY: &str = r#"module text

struct Pair {
    key: string
    value: string
}

pub fn parsePair(line: string): Pair {
    return Pair(key: line, value: line)
}
"#;

/// The consuming project writes `use graphtext::text` and calls `text::parsePair`;
/// the library keeps its own unprefixed `module text` header.
#[test]
fn a_dependency_module_is_imported_under_its_alias() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::text

pub fn run(line: string): string {
  const pair = text::parsePair(line)
  return pair.key
}
"#,
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}

/// A `use` whose first segment is a declared dependency reports against that
/// dependency, spelling the missing path the way the dependency's own source does.
#[test]
fn an_absent_dependency_module_names_the_dependency() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", "module main\n\nuse graphtext::missing\n")],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.import",
            "no module `missing` in the dependency `graphtext`".to_string()
        )],
    );
}

/// An alias roots a dependency's modules and names no module of its own.
#[test]
fn a_bare_alias_is_not_a_module() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", "module main\n\nuse graphtext\n")],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.import",
            "`graphtext` is a declared dependency, not a module; name one of its \
             modules, as in `graphtext::<module>`"
                .to_string()
        )],
    );
}

/// A dependency file's `module` header is checked against the path its own tree
/// spells, so the library still checks standalone; a header that spells the
/// consumer's alias-rooted path is the mismatch.
#[test]
fn a_dependency_header_is_checked_unprefixed() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", "module main\n")],
        &[("src/text.mw", "module graphtext::text\n")],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.module_path",
            "module header `graphtext::text` does not match its path; expected \
             `module text`"
                .to_string()
        )],
    );
}

/// Each tree owns its type namespace: the two `Pair` declarations are two types,
/// and a consumer names the dependency's through the alias.
#[test]
fn a_qualified_type_name_resolves_through_the_alias() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::text

struct Pair {
    left: int
}

pub fn run(line: string): string {
    const parsed: graphtext::Pair = text::parsePair(line)
    const mine = Pair(left: 1)
    if mine.left == 0 {
        return ""
    }
    return parsed.key
}
"#,
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}

/// A bare name resolves in the tree that wrote it. The consumer declares its own
/// `Pair`, so the name binds that one and the dependency's fields are not its.
#[test]
fn a_bare_type_name_does_not_reach_across_origins() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

struct Pair {
    left: int
}

pub fn run(): int {
    const p = Pair(key: "a", value: "b")
    return p.left
}
"#,
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.type", "check.type"],
    );
}

/// A qualified name whose first segment names no declared dependency is outside the
/// admitted set, not a bare name carrying a `::`.
#[test]
fn an_unknown_qualifier_names_no_type() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\npub fn run(p: nowhere::Pair): int {\n    return 1\n}\n",
        )],
        &[("src/text.mw", TEXT_LIBRARY)],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.unsupported"],
    );
}

/// A private function of a dependency is not callable from the consuming project.
#[test]
fn a_dependency_private_function_is_not_callable() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\nuse graphtext::text\n\npub fn run(line: string): string {\n    return text::secret(line)\n}\n",
        )],
        &[(
            "src/text.mw",
            "module text\n\nfn secret(line: string): string {\n    return line\n}\n",
        )],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.visibility"],
    );
}

const NOTES_LIBRARY: &str = r#"module notes

resource Note {
    required body: string
}

store ^notes[id: int]: Note

pub fn body(id: int): string? {
    return ^notes[id].body
}
"#;

const NOTES_APP: &str = r#"module main

use graphtext::notes

pub fn label(id: int): string {
    return notes::body(id) ?? "(none)"
}
"#;

/// A durable anchor a dependency declares resolves against that dependency's own
/// committed ledger, and a gap there is reported as the dependency's to mint.
#[test]
fn a_dependency_owns_the_identities_it_declares() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", NOTES_APP)],
        &[("src/notes.mw", NOTES_LIBRARY)],
    );
    let gaps: Vec<(&'static str, String, bool)> = diagnostics(&project)
        .iter()
        .filter_map(|row| {
            row.identity_gap().map(|gap| {
                (
                    row.code().as_str(),
                    format!(
                        "{} {} @{}",
                        gap.kind.keyword(),
                        gap.path,
                        gap.origin
                            .alias()
                            .map_or("<root>", marrow_project::DependencyAlias::as_str)
                    ),
                    gap.retired,
                )
            })
        })
        .collect();
    assert_eq!(
        gaps,
        vec![
            (
                "check.durable_identity",
                "application . @<root>".to_string(),
                false
            ),
            (
                "check.durable_identity",
                "root notes @graphtext".to_string(),
                false
            ),
            (
                "check.durable_identity",
                "product Note @graphtext".to_string(),
                false
            ),
            (
                "check.durable_identity",
                "key notes.id @graphtext".to_string(),
                false
            ),
            (
                "check.durable_identity",
                "field Note.body @graphtext".to_string(),
                false
            ),
        ],
    );
    // The report steers the reader to the tree that must commit the row.
    assert!(
        diagnostics(&project).iter().any(|row| row
            .message()
            .contains("the `graphtext` dependency's .marrow/ids")),
        "a dependency's gap names the dependency",
    );
}

/// Each tree's anchors resolve against its own ledger: the library's ids satisfy the
/// library's declarations and the root's satisfy the application anchor. Neither
/// ledger answers for the other, so moving a row between them re-opens the gap.
#[test]
fn each_origin_resolves_against_its_own_ledger() {
    let root_ids = ledger::ledger(&["application ."]);
    let library_ids = ledger::ledger(&[
        "root notes",
        "product Note",
        "key notes.id",
        "field Note.body",
    ]);
    let together = project_capture::dependency_project(
        "graphtext",
        &[("src/main.mw", NOTES_APP)],
        Some(&root_ids),
        &[("src/notes.mw", NOTES_LIBRARY)],
        Some(&library_ids),
    );
    assert_eq!(codes_and_messages(&together), Vec::new());

    // One ledger holding every row is not enough: an anchor is looked up only in the
    // ledger of the tree that declares it.
    let all_in_root = project_capture::dependency_project(
        "graphtext",
        &[("src/main.mw", NOTES_APP)],
        Some(&ledger::ledger(&[
            "application .",
            "root notes",
            "product Note",
            "key notes.id",
            "field Note.body",
        ])),
        &[("src/notes.mw", NOTES_LIBRARY)],
        None,
    );
    assert_eq!(
        diagnostics(&all_in_root)
            .iter()
            .filter_map(|row| row.identity_gap())
            .map(|gap| gap.path.as_str())
            .collect::<Vec<_>>(),
        vec!["notes", "Note", "notes.id", "Note.body"],
    );
}

/// Two trees may hold the same file identity, so every snapshot query and every
/// diagnostic is addressed by the whole `(origin, identity)` pair.
#[test]
fn a_snapshot_query_keys_on_the_origin_too() {
    let project = std::sync::Arc::new(project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/text.mw",
            "module text\n\npub fn here(): int {\n    return 1\n}\n",
        )],
        &[(
            "src/text.mw",
            "module text\n\npub fn here(): int {\n    return 22222\n}\n",
        )],
    ));
    let Ok(snapshot) = marrow_compile::analyze(
        std::sync::Arc::clone(&project),
        marrow_compile::InputRevision::new(1),
    ) else {
        panic!("the two-origin fixture analyzes");
    };
    let identity = marrow_project::FileIdentity::validate("src/text.mw")
        .expect("canonical identity")
        .0;
    let root = marrow_compile::ProjectFile::root(identity.clone());
    let dependency = marrow_compile::ProjectFile::new(project.origins()[1].clone(), identity);
    // The two addresses reach different bytes; an identity-only key would answer one
    // query with the other file's source.
    let formatted = |file: &marrow_compile::ProjectFile| match snapshot.format(file) {
        Ok(marrow_compile::FormatOutcome::Formatted(text)) => text,
        _ => panic!("expected formatted source for {:?}", file.spelling()),
    };
    assert!(!formatted(&root).contains("22222"));
    assert!(formatted(&dependency).contains("22222"));
}

const TESTED_LIBRARY: &str = r#"module text

pub fn twice(n: int): int {
    return n * 2
}

test "the library checks standalone" {
    assert twice(2) == 4
}
"#;

/// Only the root project's tests are discovered: a dependency's tests run where the
/// dependency is, so none enters the consuming project's test directory.
#[test]
fn only_root_origin_tests_are_discovered() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::text

pub fn run(n: int): int {
    return text::twice(n)
}

test "the app's own test is discovered" {
    assert run(3) == 6
}
"#,
        )],
        &[("src/text.mw", TESTED_LIBRARY)],
    );
    let compiled = marrow_compile::check(&project).unwrap_or_else(|failure| {
        panic!("expected a clean check, got {failure:#?}");
    });
    assert_eq!(
        compiled
            .tests
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["the app's own test is discovered"],
    );
}

/// Only the root project's exports are invocable: a dependency's `pub fn` is callable
/// from source across the boundary but is not a command-line entry of the consumer.
#[test]
fn only_root_origin_exports_are_invocable() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\nuse graphtext::text\n\npub fn run(n: int): int {\n    return text::twice(n)\n}\n",
        )],
        &[("src/text.mw", TESTED_LIBRARY)],
    );
    let compiled = marrow_compile::compile(&project).unwrap_or_else(|failure| {
        panic!("expected a clean compile, got {failure:#?}");
    });
    assert_eq!(
        compiled
            .exports
            .iter()
            .map(|entry| (entry.module.as_str(), entry.item.as_str()))
            .collect::<Vec<_>>(),
        vec![("main", "run")],
    );
}

/// One declaration budget covers every captured tree. Each half retains about 600
/// names of a thousand bytes against the 1 MiB ceiling, so neither crosses alone;
/// captured together they do, and the pass stops once with the typed limit rather
/// than charging each tree its own budget.
#[test]
fn one_declaration_budget_spans_both_origins() {
    let wide = "n".repeat(1000);
    let constants = |module: &str| {
        let mut source = format!("module {module}\n\n");
        for index in 0..600 {
            // `1 + 2` is a non-literal value, refused with `check.unsupported`, so each
            // constant retains its name in the ledger.
            source.push_str(&format!("const {wide}{index} = 1 + 2\n"));
        }
        source
    };
    let root = constants("main");
    let library = constants("text");

    for half in [
        project_capture::project(&[("src/main.mw", root.as_str())]),
        project_capture::project(&[("src/text.mw", library.as_str())]),
    ] {
        assert!(
            matches!(check(&half), Err(CompileFailure::Diagnostics(_))),
            "neither half crosses the ledger ceiling alone",
        );
    }

    let together = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", root.as_str())],
        &[("src/text.mw", library.as_str())],
    );
    match check(&together) {
        Err(CompileFailure::ResourceLimit(limit)) => assert_eq!(
            limit.kind(),
            marrow_compile::ResourceLimitKind::DeclarationLedgerBytes
        ),
        other => panic!("expected the one ledger ceiling, got {other:#?}"),
    }
}

/// Capturing the same two trees twice yields byte-identical image bytes: nothing in
/// the compiler's origin handling depends on arrival order or on where a tree sits.
#[test]
fn two_captures_of_the_same_trees_compile_to_identical_bytes() {
    let image = || {
        let project = project_capture::project_with_dependency(
            "graphtext",
            &[(
                "src/main.mw",
                "module main\n\nuse graphtext::text\n\npub fn run(n: int): int {\n    return text::twice(n)\n}\n",
            )],
            &[("src/text.mw", TESTED_LIBRARY)],
        );
        marrow_compile::compile(&project)
            .unwrap_or_else(|failure| panic!("expected a clean compile, got {failure:#?}"))
            .image
    };
    let first = image();
    let second = image();
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.image_id, second.image_id);
}

/// An alias cannot be shadowed: a root module whose first segment occupies the alias
/// is refused where the two trees are captured, before the compiler ever sees them,
/// so the alias-rooted path has one meaning.
#[test]
fn a_root_module_may_not_shadow_an_alias() {
    let manifest = marrow_project::Manifest::parse(
        "edition = \"2026\"\n\n[dependencies]\ngraphtext = { path = \"../graphtext\" }\n",
    )
    .expect("valid manifest");
    let alias = manifest.dependencies()[0].alias().clone();
    let files = vec![
        marrow_project::CapturedFile::new(
            "src/graphtext.mw".to_string(),
            b"module graphtext\n".to_vec(),
        ),
        marrow_project::CapturedFile::in_dependency(
            alias.clone(),
            "src/text.mw".to_string(),
            TEXT_LIBRARY.as_bytes().to_vec(),
        ),
    ];
    let failure = marrow_project::capture_origins(
        &manifest,
        files,
        None,
        &[marrow_project::CapturedDependency::new(&alias, None)],
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect_err("an alias-shadowing root module is refused");
    assert_eq!(failure.code().as_str(), "project.dependency_alias");
}

/// An enum payload annotation is written in the tree that declares the enum, so the
/// one payload-admission rule resolves it in that tree's namespace. Both projects
/// declare `Point`; the library's payload carries the library's.
#[test]
fn an_enum_payload_resolves_in_its_declaring_tree() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::shapes

struct Point {
    label: string
}

pub fn run(): int {
    const mine = Point(label: "here")
    const cell = shapes::filled(3, 4)
    if isEmpty(mine.label) {
        return 0
    }
    return shapes::spread(cell)
}
"#,
        )],
        &[(
            "src/shapes.mw",
            r#"module shapes

struct Point {
    x: int
    y: int
}

enum Cell {
    empty
    filled(at: Point)
}

pub fn filled(x: int, y: int): Cell {
    return Cell::filled(at: Point(x: x, y: y))
}

pub fn spread(cell: Cell): int {
    match cell {
        empty => {
            return 0
        }
        filled(at) => {
            return at.x + at.y
        }
    }
}
"#,
        )],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}

const COLOR_LIBRARY: &str = r#"module palette

enum Color {
    red
    green
}

pub fn name(color: Color): string {
    match color {
        red => {
            return "red"
        }
        green => {
            return "green"
        }
    }
}
"#;

/// A dependency's enum is nameable as a type, so its members must be constructible
/// and matchable from the consuming tree too.
#[test]
fn a_dependency_enum_member_is_constructible_and_matchable() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::palette

pub fn run(): string {
    const chosen: graphtext::Color = graphtext::Color::red
    match chosen {
        red => {
            return palette::name(graphtext::Color::green)
        }
        green => {
            return palette::name(chosen)
        }
    }
}
"#,
        )],
        &[("src/palette.mw", COLOR_LIBRARY)],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}
