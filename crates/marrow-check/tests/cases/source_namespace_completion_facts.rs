use std::path::Path;

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    CallableSignatureKind, SourceNamespaceCompletionFact, SourceNamespaceEnumMemberStatus,
    source_namespace_completion_fact, source_namespace_completion_file_fact,
};
use marrow_syntax::SourceFile;

fn analyze_project(name: &str) -> (AnalysisSnapshot, Vec<std::path::PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(
        name,
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

resource Book
    ;; Display title.
    required title: string

;; Public lifecycle state.
pub enum Status
    ;; Ready for use.
    active
    ;; No longer active.
    archived

enum Secret
    hidden

;; Returns a book title.
pub fn titleOf(id: int): string
    return \"title\"

fn privateTitle(): string
    return \"hidden\"

const LIMIT: int = 100
",
            ),
            (
                "src/shelf/app.mw",
                "\
module shelf::app

use shelf::books

resource Draft
    required title: string

enum LocalStatus
    draft

fn privateDraft(): string
    return \"draft\"
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);
    (snapshot, paths)
}

fn source_file<'a>(snapshot: &'a AnalysisSnapshot, file: &Path) -> &'a SourceFile {
    &snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)
        .expect("analyzed file")
        .parsed
        .file
}

fn namespace_fact(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    qualifier: &[&str],
) -> Option<SourceNamespaceCompletionFact> {
    let qualifier = qualifier
        .iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    source_namespace_completion_fact(
        &snapshot.program,
        file,
        source_file(snapshot, file),
        &qualifier,
    )
}

fn file_namespace_fact(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    qualifier: &[&str],
) -> Option<SourceNamespaceCompletionFact> {
    let qualifier = qualifier
        .iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    source_namespace_completion_file_fact(
        &snapshot.program,
        file,
        source_file(snapshot, file),
        &qualifier,
    )
}

#[test]
fn namespace_completion_resolves_used_module_alias_to_visible_members() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-used-module");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Module(fact)) =
        namespace_fact(&snapshot, app, &["books"])
    else {
        panic!("expected module namespace fact");
    };

    assert_eq!(fact.module, "shelf::books");
    assert_eq!(
        fact.resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect::<Vec<_>>(),
        ["Book"]
    );
    assert_eq!(
        fact.enums
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["Status"]
    );
    assert_eq!(
        fact.functions
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["titleOf"]
    );
}

#[test]
fn namespace_completion_keeps_same_module_private_members_visible() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-same-module");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Module(fact)) =
        namespace_fact(&snapshot, app, &["shelf", "app"])
    else {
        panic!("expected same-module namespace fact");
    };

    assert_eq!(fact.module, "shelf::app");
    assert_eq!(
        fact.resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect::<Vec<_>>(),
        ["Draft"]
    );
    assert_eq!(
        fact.enums
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["LocalStatus"]
    );
    assert_eq!(
        fact.functions
            .iter()
            .map(|completion| completion.name.as_str())
            .collect::<Vec<_>>(),
        ["privateDraft"]
    );
}

#[test]
fn namespace_completion_returns_enum_members_with_docs_and_status() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-enum-members");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Enum(fact)) =
        namespace_fact(&snapshot, app, &["books", "Status"])
    else {
        panic!("expected enum namespace fact");
    };

    assert_eq!(fact.enum_name, "Status");
    assert_eq!(
        fact.members
            .iter()
            .map(|member| (member.name.as_str(), member.docs.as_slice(), member.status,))
            .collect::<Vec<_>>(),
        [
            (
                "active",
                ["Ready for use.".to_string()].as_slice(),
                SourceNamespaceEnumMemberStatus::Selectable,
            ),
            (
                "archived",
                ["No longer active.".to_string()].as_slice(),
                SourceNamespaceEnumMemberStatus::Selectable,
            ),
        ]
    );
}

#[test]
fn namespace_completion_fails_closed_for_resources_and_module_prefixes() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-fail-closed");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    assert_eq!(namespace_fact(&snapshot, app, &["books", "Book"]), None);
    assert_eq!(
        namespace_fact(&snapshot, app, &["shelf", "books", "Book"]),
        None
    );
    assert_eq!(namespace_fact(&snapshot, app, &["shelf"]), None);
}

#[test]
fn namespace_completion_fails_closed_for_ambiguous_single_segment_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-ambiguous-single-alias",
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

pub enum Status
    active

pub fn titleOf(): string
    return \"title\"
",
            ),
            (
                "src/archive/books.mw",
                "module archive::books\n\npub fn oldTitle(): string\n    return \"old\"\n",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use shelf::books
use archive::books
",
            ),
        ],
    );
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert_eq!(namespace_fact(&snapshot, app, &["books"]), None);
}

#[test]
fn namespace_completion_fails_closed_when_local_declaration_collides_with_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-local-alias-collision",
        &[
            (
                "src/shelf/books.mw",
                "module shelf::books\n\npub fn titleOf(): string\n    return \"title\"\n",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use shelf::books

resource books
    required title: string
",
            ),
        ],
    );
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert_eq!(namespace_fact(&snapshot, app, &["books"]), None);
}

#[test]
fn namespace_completion_returns_std_root_modules_from_checker_fact() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-std-root");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::StandardLibraryRoot(fact)) =
        namespace_fact(&snapshot, app, &["std"])
    else {
        panic!("expected std root namespace fact");
    };

    assert_eq!(
        fact.modules
            .iter()
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>(),
        [
            "text", "bytes", "hash", "math", "json", "csv", "id", "random", "context", "audit",
            "error", "matrix", "clock", "env", "io", "assert", "log"
        ]
    );
}

#[test]
fn namespace_completion_returns_std_module_ops_from_checker_fact() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-std-module");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::StandardLibraryModule(fact)) =
        namespace_fact(&snapshot, app, &["std", "clock"])
    else {
        panic!("expected std module namespace fact");
    };

    let names = fact
        .operations
        .iter()
        .map(|operation| operation.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"now"), "clock op, got {names:?}");
    assert!(names.contains(&"today"), "clock op, got {names:?}");
    assert!(
        !names.contains(&"length"),
        "other std module ops must not leak, got {names:?}"
    );

    let now = fact
        .operations
        .iter()
        .find(|operation| operation.name == "now")
        .expect("now operation");
    assert_eq!(now.signature.kind, CallableSignatureKind::StandardLibrary);
    assert_eq!(now.signature.path, ["std", "clock", "now"]);
}

#[test]
fn namespace_completion_keeps_raw_std_before_project_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-raw-std-before-import-alias",
        &[
            (
                "src/foo/std.mw",
                "\
module foo::std

pub fn projectOnly(): string
    return \"project\"
",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use foo::std
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert!(
        matches!(
            namespace_fact(&snapshot, app, &["std"]),
            Some(SourceNamespaceCompletionFact::StandardLibraryRoot(_))
        ),
        "raw std qualifier must not expand through a project import alias"
    );
    assert!(
        matches!(
            namespace_fact(&snapshot, app, &["std", "clock"]),
            Some(SourceNamespaceCompletionFact::StandardLibraryModule(_))
        ),
        "raw std::clock qualifier must not expand through a project import alias"
    );
}

#[test]
fn namespace_completion_keeps_raw_std_project_fallback_before_project_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-raw-std-project-fallback-before-import-alias",
        &[
            (
                "src/std/widgets.mw",
                "\
module std::widgets

pub fn projectOnly(): string
    return \"project\"
",
            ),
            (
                "src/foo/std.mw",
                "\
module foo::std

pub fn wrongRoot(): string
    return \"wrong\"
",
            ),
            (
                "src/foo/std/widgets.mw",
                "\
module foo::std::widgets

pub fn wrongAlias(): string
    return \"wrong\"
",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use foo::std
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::Module(fact)) =
        namespace_fact(&snapshot, app, &["std", "widgets"])
    else {
        panic!("expected raw std project module fallback");
    };

    assert_eq!(fact.module, "std::widgets");
    assert_eq!(
        fact.functions
            .iter()
            .map(|function| function.name.as_str())
            .collect::<Vec<_>>(),
        ["projectOnly"]
    );
}

#[test]
fn namespace_completion_expands_imported_std_module_alias_to_std_facts() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-imported-std-module-alias",
        &[(
            "src/app/main.mw",
            "\
module app::main

use std::text
",
        )],
    );
    support::assert_clean(&snapshot.report);
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::StandardLibraryModule(fact)) =
        namespace_fact(&snapshot, app, &["text"])
    else {
        panic!("expected imported std module alias to expose std module facts");
    };

    assert_eq!(fact.module, "text");
    assert!(
        fact.operations
            .iter()
            .any(|operation| operation.name == "length"),
        "text std operation, got {:?}",
        fact.operations
            .iter()
            .map(|operation| operation.name.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn file_namespace_completion_keeps_stdlib_out_of_mcp_file_facts() {
    let (snapshot, paths) = analyze_project("source-namespace-completion-file-std-closed");
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/shelf/app.mw"))
        .expect("app file");

    assert_eq!(file_namespace_fact(&snapshot, app, &["std"]), None);
    assert_eq!(file_namespace_fact(&snapshot, app, &["std", "clock"]), None);
}

#[test]
fn namespace_completion_prefers_known_std_modules_but_falls_back_to_project_modules() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-std-precedence",
        &[
            (
                "src/std/clock.mw",
                "\
module std::clock

pub fn projectOnly(): string
    return \"project\"
",
            ),
            (
                "src/std/widgets.mw",
                "\
module std::widgets

pub fn projectOnly(): string
    return \"project\"
",
            ),
            ("src/app/main.mw", "module app::main\n"),
        ],
    );
    support::assert_clean(&snapshot.report);
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    let Some(SourceNamespaceCompletionFact::StandardLibraryModule(clock)) =
        namespace_fact(&snapshot, app, &["std", "clock"])
    else {
        panic!("expected known std module to keep builtin precedence");
    };
    assert!(
        !clock
            .operations
            .iter()
            .any(|operation| operation.name == "projectOnly"),
        "known std module must not expose same-named project module members"
    );

    let Some(SourceNamespaceCompletionFact::Module(widgets)) =
        namespace_fact(&snapshot, app, &["std", "widgets"])
    else {
        panic!("expected unknown std module to fall back to project module");
    };
    assert_eq!(widgets.module, "std::widgets");
    assert_eq!(
        widgets
            .functions
            .iter()
            .map(|function| function.name.as_str())
            .collect::<Vec<_>>(),
        ["projectOnly"]
    );
}

#[test]
fn file_namespace_completion_fails_closed_when_function_parameter_shadows_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-parameter-alias-collision",
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

pub enum Status
    active

pub fn titleOf(): string
    return \"title\"
",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use shelf::books

pub fn run(books: int): int
    return books
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert_eq!(file_namespace_fact(&snapshot, app, &["books"]), None);
    assert_eq!(
        file_namespace_fact(&snapshot, app, &["books", "Status"]),
        None
    );
    assert!(
        matches!(
            file_namespace_fact(&snapshot, app, &["shelf", "books", "Status"]),
            Some(SourceNamespaceCompletionFact::Enum(_))
        ),
        "fully-qualified enum lookup should not depend on the alias head"
    );
}

#[test]
fn file_namespace_completion_fails_closed_when_function_local_binding_shadows_import_alias() {
    for (name, body) in [
        ("const", "    const books = 5\n    return books\n"),
        ("var", "    var books: int = 5\n    return books\n"),
        (
            "if_const",
            "    if const books = ^notes(1).title\n        return 1\n    return 0\n",
        ),
        (
            "for_first",
            "    for books in 1..3\n        print(books)\n    return 0\n",
        ),
        (
            "for_second",
            "    var scores(name: string): int\n    for key, books in scores\n        print(books)\n    return 0\n",
        ),
        (
            "catch",
            "    try\n        print(\"try\")\n    catch books: Error\n        print(books.message)\n    return 0\n",
        ),
    ] {
        let (snapshot, paths) = support::analyze_overlay(
            &format!("source-namespace-completion-local-alias-collision-{name}"),
            &[
                (
                    "src/shelf/books.mw",
                    "\
module shelf::books

pub enum Status
    active

pub fn titleOf(): string
    return \"title\"
",
                ),
                (
                    "src/app/main.mw",
                    &format!(
                        "\
module app::main

use shelf::books

resource Note
    required title: string

store ^notes(id: int): Note

pub fn run(): int
{body}"
                    ),
                ),
            ],
        );
        support::assert_clean(&snapshot.report);
        let app = paths
            .iter()
            .find(|path| path.ends_with("src/app/main.mw"))
            .expect("app file");

        assert_eq!(
            file_namespace_fact(&snapshot, app, &["books"]),
            None,
            "case {name} should fail closed for module alias"
        );
        assert_eq!(
            file_namespace_fact(&snapshot, app, &["books", "Status"]),
            None,
            "case {name} should fail closed for enum alias"
        );
        assert!(
            matches!(
                file_namespace_fact(&snapshot, app, &["shelf", "books", "Status"]),
                Some(SourceNamespaceCompletionFact::Enum(_))
            ),
            "case {name} should preserve fully-qualified enum lookup"
        );
    }
}

#[test]
fn file_namespace_completion_fails_closed_when_evolve_transform_body_shadows_import_alias() {
    let (snapshot, paths) = support::analyze_overlay(
        "source-namespace-completion-evolve-transform-alias-collision",
        &[
            (
                "src/shelf/books.mw",
                "\
module shelf::books

pub enum Status
    active

pub fn titleOf(): string
    return \"title\"
",
            ),
            (
                "src/app/main.mw",
                "\
module app::main

use shelf::books

resource Draft
    required source: string
    title: string

store ^drafts(id: int): Draft

evolve
    transform Draft.title
        const books = old.source
        return books
",
            ),
        ],
    );
    support::assert_clean(&snapshot.report);
    let app = paths
        .iter()
        .find(|path| path.ends_with("src/app/main.mw"))
        .expect("app file");

    assert_eq!(file_namespace_fact(&snapshot, app, &["books"]), None);
    assert_eq!(
        file_namespace_fact(&snapshot, app, &["books", "Status"]),
        None
    );
    assert!(
        matches!(
            file_namespace_fact(&snapshot, app, &["shelf", "books", "Status"]),
            Some(SourceNamespaceCompletionFact::Enum(_))
        ),
        "fully-qualified enum lookup should not depend on the alias head"
    );
}
