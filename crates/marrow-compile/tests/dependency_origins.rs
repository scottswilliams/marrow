//! Compiling a project that reuses a local source dependency.
//!
//! Every case captures two trees through the one production `capture_origins` call
//! and drives the production `check`, so what is asserted is what `marrow check`
//! would report over the same two directories.

use marrow_compile::{
    CompileFailure, IdentityGap, NameFamily, SourceDiagnostic, Unresolved, check,
};
use marrow_project::ProjectInput;

use marrow_test_programs::project as project_capture;

use marrow_test_programs::ledger;

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
                        gap.kind().keyword(),
                        gap.path(),
                        gap.origin()
                            .alias()
                            .map_or("<root>", marrow_project::DependencyAlias::as_str)
                    ),
                    gap.retired(),
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
            .map(IdentityGap::path)
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

/// A match over a dependency's enum is exhaustive on that enum's members. The arm
/// header carries no enum prefix in either tree — the scrutinee supplies the enum —
/// so the report names the missing member the way the library declares it.
#[test]
fn a_match_over_a_dependency_enum_is_exhaustive() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

pub fn run(): string {
    const chosen: graphtext::Color = graphtext::Color::red
    match chosen {
        red => {
            return "red"
        }
    }
}
"#,
        )],
        &[("src/palette.mw", COLOR_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project)
            .into_iter()
            .find(|(code, _)| *code == "check.match_nonexhaustive"),
        Some((
            "check.match_nonexhaustive",
            "the `match` on `Color` does not cover `green`. A match covers every member \
             of an enum exactly once and admits no wildcard arm. Add the missing arm: \
             `green =>`."
                .to_string()
        )),
    );
}

/// A payload member crosses the boundary on the same terms: the member is named
/// through the alias, each payload field keeps the declaring tree's own type and is
/// supplied by name, and an arm in the consuming tree binds them.
#[test]
fn a_dependency_enum_payload_member_is_constructible() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

pub fn run(): int {
    const cell = graphtext::Cell::filled(at: graphtext::Point(x: 1, y: 2))
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
"#,
        )],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}

/// A bare head resolves in the tree that wrote it, so the consumer's own `Color` is
/// what `Color::red` names even where a dependency declares that name too.
#[test]
fn a_bare_enum_path_stays_in_the_tree_that_wrote_it() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

enum Color {
    red
    blue
}

pub fn mine(): Color {
    return Color::red
}

pub fn theirs(): graphtext::Color {
    return graphtext::Color::green
}

pub fn wrong(): Color {
    return Color::green
}
"#,
        )],
        &[("src/palette.mw", COLOR_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.type",
            "enum `Color` has no member `green`".to_string()
        )],
    );
}

/// A member the named enum does not declare is the enum's own report, naming the
/// head the way this site spells it rather than in the library's own terms.
#[test]
fn an_absent_dependency_enum_member_is_the_enum_diagnostic() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\npub fn run(): graphtext::Color {\n    return graphtext::Color::blue\n}\n",
        )],
        &[("src/palette.mw", COLOR_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![(
            "check.type",
            "enum `graphtext::Color` has no member `blue`".to_string()
        )],
    );
}

/// An enum path's head is the same two-segment `type_name` a type annotation takes,
/// so an undeclared first segment and a longer path both name no enum — the same
/// answer the type position gives the same head spelling.
#[test]
fn an_enum_path_head_is_bounded_and_resolves_through_declared_aliases() {
    for written in [
        // No dependency is declared under `nowhere`.
        "nowhere::Color::red",
        // Four segments: the head names no type, so this is not an enum path.
        "graphtext::palette::Color::red",
    ] {
        let project = project_capture::project_with_dependency(
            "graphtext",
            &[(
                "src/main.mw",
                &format!(
                    "module main\n\npub fn run(): int {{\n    const c = {written}\n    return 1\n}}\n"
                ),
            )],
            &[("src/palette.mw", COLOR_LIBRARY)],
        );
        assert_eq!(
            codes_and_messages(&project),
            vec![(
                "check.unsupported",
                "a qualified name is not yet supported on the beta line".to_string()
            )],
            "input {written:?}",
        );
    }
}

const COLOR_ONLY: &str = "module palette\n\nenum Color {\n    red\n    green\n}\n";

/// The source a `NAME::red` fixture shares, so the local and imported programs differ
/// in exactly one thing: whether the enum is reached through an alias.
const COLOR_APP: &str = r#"module main

pub fn run(): string {
    const c: NAME = NAME::red
    match c {
        red => {
            return "red"
        }
        green => {
            return "green"
        }
    }
}
"#;

/// Where an enum is declared is not part of what it compiles to. The image records an
/// origin only as a source coordinate — the file spelling a dependency's own code is
/// reported at — so an enum reached through an alias costs the image no byte, adds no
/// export, and never spells the alias; the program is otherwise the same program.
#[test]
fn an_imported_enum_costs_the_image_nothing() {
    let local_main = COLOR_APP.replace("NAME", "Color");
    let local = project_capture::project(&[
        ("src/palette.mw", COLOR_ONLY),
        ("src/main.mw", local_main.as_str()),
    ]);
    let imported_main = COLOR_APP.replace("NAME", "graphtext::Color");
    let imported = project_capture::project_with_dependency(
        "graphtext",
        &[("src/main.mw", imported_main.as_str())],
        &[("src/palette.mw", COLOR_ONLY)],
    );
    let compiled = |project| {
        marrow_compile::compile(project)
            .unwrap_or_else(|failure| panic!("expected a clean compile, got {failure:#?}"))
    };
    let (local, imported) = (compiled(&local), compiled(&imported));
    assert_eq!(local.image.bytes.len(), imported.image.bytes.len());
    assert_eq!(
        imported
            .exports
            .iter()
            .map(|entry| (entry.module.as_str(), entry.item.as_str()))
            .collect::<Vec<_>>(),
        local
            .exports
            .iter()
            .map(|entry| (entry.module.as_str(), entry.item.as_str()))
            .collect::<Vec<_>>(),
    );
    // The alias is the consumer's private name for a tree. It names no program
    // element, so no image byte spells it.
    assert!(
        !imported
            .image
            .bytes
            .windows("graphtext".len())
            .any(|window| window == b"graphtext"),
    );
    // What is left of the origin is the source coordinate: the consumer writes eleven
    // more characters, so the two images differ only where spans record that. Renaming
    // the alias to another spelling of the same length is byte-identical.
    let renamed_main = COLOR_APP.replace("NAME", "othername::Color");
    let renamed = compiled(&project_capture::project_with_dependency(
        "othername",
        &[("src/main.mw", renamed_main.as_str())],
        &[("src/palette.mw", COLOR_ONLY)],
    ));
    assert_eq!(imported.image.bytes, renamed.image.bytes);
    assert_eq!(imported.image.image_id, renamed.image.image_id);
}

const CONSTRUCTED_LIBRARY: &str = r#"module text

struct Pair {
    key: string
    value: string
}

struct Boxed<T> {
    item: T
}

type Age: int in 0..150

resource Book {
    required title: string
}
"#;

/// A dependency's type is constructed through the same alias that annotates it: a
/// struct, a generic struct template, a nominal int, and a resource record all take
/// their own tree's arguments.
#[test]
fn a_dependency_type_is_constructible_through_its_alias() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

pub fn run(): string {
    const pair: graphtext::Pair = graphtext::Pair(key: "a", value: "b")
    const age: graphtext::Age = graphtext::Age(7)
    const book: graphtext::Book = graphtext::Book(title: pair.key)
    const boxed: graphtext::Boxed<int> = graphtext::Boxed(item: 1)
    if age == graphtext::Age(7) and boxed.item == 1 {
        return book.title
    }
    return pair.value
}
"#,
        )],
        &[("src/text.mw", CONSTRUCTED_LIBRARY)],
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}

/// A bare constructor names the consuming tree's own type even where a dependency
/// declares that name: the two `Pair`s are two types, and the consumer's fields are
/// the ones its constructor takes.
#[test]
fn a_bare_constructor_stays_in_the_tree_that_wrote_it() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

struct Pair {
    left: int
}

pub fn mine(): int {
    const p = Pair(left: 1)
    return p.left
}

pub fn theirs(): string {
    return graphtext::Pair(key: "a", value: "b").key
}

pub fn wrong(): int {
    const p = Pair(key: "a")
    return p.left
}
"#,
        )],
        &[("src/text.mw", CONSTRUCTED_LIBRARY)],
    );
    assert_eq!(
        codes_and_messages(&project),
        vec![("check.type", "`Pair` has no field `key`".to_string())],
    );
}

/// What a constructor callee resolved to, as the site's one diagnostic reports it.
enum Resolved {
    /// A declared type: the report is about the field list, which only a resolved
    /// type has.
    Type(&'static str),
    /// Nothing: the report carries the typed unresolved name, spelled as written.
    Nothing(&'static str),
}

/// A constructor call resolves its callee as a type exactly where an annotation of
/// the same text would: through a declared alias, at one or two segments. A report
/// names the type the way this site spells it.
#[test]
fn a_constructor_callee_is_a_type_name_or_nothing() {
    for (written, expected) in [
        (
            r#"graphtext::Pair(key: "a", nope: "b")"#,
            Resolved::Type("`graphtext::Pair` has no field `nope`"),
        ),
        // Three segments: the head names no type, so this is not a constructor.
        (
            r#"graphtext::text::Pair(key: "a", value: "b")"#,
            Resolved::Nothing("graphtext::text::Pair"),
        ),
        // No dependency is declared under `nowhere`.
        (
            r#"nowhere::Pair(key: "a", value: "b")"#,
            Resolved::Nothing("nowhere::Pair"),
        ),
        // The alias is declared; the dependency declares no such type.
        (
            "graphtext::Missing(item: 1)",
            Resolved::Nothing("graphtext::Missing"),
        ),
    ] {
        let project = project_capture::project_with_dependency(
            "graphtext",
            &[(
                "src/main.mw",
                &format!(
                    "module main\n\npub fn run(): int {{\n    const v = {written}\n    return 1\n}}\n"
                ),
            )],
            &[("src/text.mw", CONSTRUCTED_LIBRARY)],
        );
        match expected {
            Resolved::Type(message) => assert_eq!(
                codes_and_messages(&project),
                vec![("check.type", message.to_string())],
                "input {written:?}",
            ),
            Resolved::Nothing(name) => {
                let rows = diagnostics(&project);
                let [row] = &rows[..] else {
                    panic!("input {written:?} reports exactly one row, got {rows:#?}");
                };
                assert_eq!(
                    row.unresolved(),
                    Some(&Unresolved {
                        family: NameFamily::Function,
                        name: name.to_string(),
                    }),
                    "input {written:?}",
                );
            }
        }
    }
}

/// A type the dependency declared and the compiler refused keeps its name, so a
/// construction of it is steered to that declaration's cause rather than told the
/// name does not exist.
#[test]
fn a_refused_dependency_type_steers_to_its_declaration() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            "module main\n\npub fn run(): int {\n    const v = graphtext::Bad(x: 1)\n    return 1\n}\n",
        )],
        &[(
            "src/text.mw",
            "module text\n\nstruct Bad {\n    x: int\n    x: string\n}\n",
        )],
    );
    assert_eq!(
        diagnostics(&project)
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.name_conflict", "check.name_conflict"],
    );
    assert!(
        diagnostics(&project)[1]
            .message()
            .contains("its declaration was refused"),
        "the use is steered to the refused declaration",
    );
}

/// Two trees may each declare a resource of the same name, so the member ledger is
/// keyed by the declaring origin and not by the bare type name. Each `Point` keeps
/// exactly the members its own tree wrote, and a member the compiler refused steers
/// the use to the declaration that wrote it.
#[test]
fn a_member_ledger_keys_on_the_declaring_origin() {
    let project = project_capture::project_with_dependency(
        "graphtext",
        &[(
            "src/main.mw",
            r#"module main

use graphtext::notes

resource Point {
    required title: string
    tag: int
}

pub fn run(): int {
    const p = Point(title: "a", tag: 1)
    return p.tag ?? 0
}
"#,
        )],
        &[(
            "src/notes.mw",
            r#"module notes

resource Point {
    required label: string
    tag: Missing
}

pub fn here(): string {
    const p = Point(label: "b", tag: 1)
    return p.label
}
"#,
        )],
    );
    let reported = diagnostics(&project);
    assert_eq!(
        reported
            .iter()
            .map(|row| row.code().as_str())
            .collect::<Vec<_>>(),
        vec!["check.unsupported", "check.unsupported"],
        "{:#?}",
        codes_and_messages(&project),
    );
    // The app's `Point` never demands the library's `label`, and the library's use of
    // its own refused `tag` is steered to the library's declaration.
    assert!(
        reported[1]
            .message()
            .contains("its declaration was refused"),
        "{}",
        reported[1].message(),
    );
}

/// A library that owns its own store and drives its writer where it lives.
const SHELF_LIBRARY: &str = r#"module shelf

resource Book {
    required title: string
}

store ^shelf[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^shelf[id] = Book(title: title)
    }
}

pub fn title(id: int): string? {
    return ^shelf[id].title
}

test "the library drives its own writer" {
    add(1, "Small Gods")
    assert title(1) ?? "" == "Small Gods"
}
"#;

/// The same library offering a mutating helper that carries no block of its own, so a
/// consuming export supplies the region the write commits in.
const SHELF_HELPER_LIBRARY: &str = r#"module shelf

resource Book {
    required title: string
}

store ^shelf[id: int]: Book

pub fn stage(id: int, title: string) {
    ^shelf[id] = Book(title: title)
}

pub fn title(id: int): string? {
    return ^shelf[id].title
}

test "the library checks its reader standalone" {
    assert title(1) ?? "(none)" == "(none)"
}
"#;

/// The library's committed ledger for both shelf fixtures.
fn shelf_library_ids() -> Vec<u8> {
    ledger::ledger(&[
        "root shelf",
        "product Book",
        "key shelf.id",
        "field Book.title",
    ])
}

/// A dependency's `pub fn` is not an export of the consuming compilation — it takes no
/// export slot and the command line cannot name it — so a `transaction` block in one
/// sits outside its owning export. The consuming `check` refuses it in source terms
/// rather than handing the verifier an image whose transaction marker has no owner.
#[test]
fn a_dependency_transaction_owner_is_refused_in_the_consumer() {
    let project = project_capture::dependency_project(
        "library",
        &[(
            "src/main.mw",
            "module main\n\npub fn f(): int {\n    return 1\n}\n",
        )],
        Some(&ledger::ledger(&["application ."])),
        &[("src/shelf.mw", SHELF_LIBRARY)],
        Some(&shelf_library_ids()),
    );
    let reported = diagnostics(&project);
    assert_eq!(
        reported
            .iter()
            .map(|row| (row.code().as_str(), row.line(), row.column()))
            .collect::<Vec<_>>(),
        vec![("check.transaction_misplaced", 10, 17)],
    );
    assert!(
        reported[0].message().contains("`library`"),
        "the refusal names the dependency: {}",
        reported[0].message(),
    );
}

/// The siblings a consumer holds without owning: a library `test`, a library export
/// with durable demand, and a library helper that mutates inside the consuming export's
/// own region. None owns a block in this compilation, so all three check.
#[test]
fn a_dependency_reader_helper_and_test_check_in_the_consumer() {
    let project = project_capture::dependency_project(
        "library",
        &[(
            "src/main.mw",
            r#"module main

use library::shelf

pub fn label(id: int): string {
    return shelf::title(id) ?? "(none)"
}

pub fn put(id: int, title: string) {
    transaction {
        shelf::stage(id, title)
    }
}
"#,
        )],
        Some(&ledger::ledger(&["application ."])),
        &[("src/shelf.mw", SHELF_HELPER_LIBRARY)],
        Some(&shelf_library_ids()),
    );
    assert_eq!(codes_and_messages(&project), Vec::new());
}
