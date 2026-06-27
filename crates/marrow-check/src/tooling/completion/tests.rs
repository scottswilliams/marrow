use super::*;
use crate::{ProjectSources, ScalarType, analyze_project};
use marrow_project::parse_config;
use marrow_syntax::{Severity, SourceSpan, lex_source, parse_source};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);

fn context_at(source: &str) -> (String, SourceCompletionContext) {
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    let lexed = lex_source(&source);
    let context = source_completion_context(&source, &lexed, offset);
    (source, context)
}

#[test]
fn classifies_source_completion_cursor_contexts() {
    let (_, context) = context_at("module shelf::app\n\npub fn f()\n    delete ^|\n");
    assert_eq!(context, SourceCompletionContext::Root);

    let (_, context) = context_at("module shelf::app\n\npub fn f()\n    const x = std::clock::|\n");
    assert_eq!(
        context,
        SourceCompletionContext::Namespace {
            qualifier: vec!["std".to_string(), "clock".to_string()]
        }
    );

    let (source, context) =
        context_at("module shelf::app\n\npub fn f(id: int)\n    const x = ^books(id).|\n");
    let SourceCompletionContext::SavedPath { receiver_span } = context else {
        panic!("expected saved-path context, got {context:?}");
    };
    assert_receiver_span(&source, receiver_span, "^books(id)");

    let (_, context) =
        context_at("module shelf::app\n\npub fn f(id: int)\n    const x = ^books(id)..|\n");
    assert_eq!(context, SourceCompletionContext::InvalidSavedPath);

    let (_, context) = context_at("module shelf::app\n\npub fn f(x: |\n");
    assert_eq!(context, SourceCompletionContext::Type);

    let (_, context) = context_at("module shelf::app\n\npub fn f(total: int)\n    return t|\n");
    assert_eq!(context, SourceCompletionContext::Bare);

    let (_, context) =
        context_at("module shelf::app\n\npub fn f()\n    const draft = Draft(state: a|)\n");
    assert_eq!(context, SourceCompletionContext::Bare);
}

fn assert_receiver_span(source: &str, span: SourceSpan, receiver: &str) {
    assert_eq!(&source[span.start_byte..span.end_byte], receiver);
}

#[test]
fn source_completion_fact_returns_protocol_free_items_for_current_contexts() {
    let project = CompletionProject::new();
    let program = project.program();
    let app = project.app_file();

    let root = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    delete ^|\n",
    );
    let books = item_named(&root, "books");
    assert_eq!(books.kind, SourceCompletionItemKind::SavedRoot);
    assert_eq!(books.detail.as_deref(), Some("saved root of Book"));
    assert_eq!(books.docs, ["Books saved by id."]);

    let saved_member = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f(id: int)\n    const x = ^books(id).|\n",
    );
    let title = item_named(&saved_member, "title");
    assert_eq!(title.kind, SourceCompletionItemKind::Field);
    assert_eq!(title.detail.as_deref(), Some("required field: string"));
    let notes = item_named(&saved_member, "notes");
    assert_eq!(notes.kind, SourceCompletionItemKind::Layer);
    assert_eq!(notes.detail.as_deref(), Some("layer(noteId: string)"));

    let namespace = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const x = books::|\n",
    );
    assert_eq!(
        item_named(&namespace, "Book").kind,
        SourceCompletionItemKind::Resource
    );
    assert_eq!(
        item_named(&namespace, "Status").kind,
        SourceCompletionItemKind::Enum
    );
    assert_eq!(
        item_named(&namespace, "titleOf").detail.as_deref(),
        Some("fn titleOf(id: Id(^books)): string")
    );

    let types = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f(x: |\n",
    );
    assert_eq!(
        item_named(&types, "int").kind,
        SourceCompletionItemKind::Keyword
    );
    assert_eq!(item_named(&types, "int").detail.as_deref(), Some("type"));
    assert_eq!(
        item_named(&types, "Id(^books)").kind,
        SourceCompletionItemKind::StoreIdentity
    );

    let bare = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f(count: int)\n    const total: int = count\n    return t|\n",
    );
    assert_eq!(
        item_named(&bare, "total").kind,
        SourceCompletionItemKind::Local
    );
    assert_eq!(item_named(&bare, "total").detail.as_deref(), Some("int"));
    assert_eq!(
        item_named(&bare, "return").kind,
        SourceCompletionItemKind::Keyword
    );
    assert_eq!(
        item_named(&bare, "key").kind,
        SourceCompletionItemKind::Function
    );
    assert_eq!(
        item_named(&bare, "key").detail.as_deref(),
        Some("key(id): value")
    );

    let local_tree = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    var scores(player: string): int\n    return s|\n",
    );
    assert_eq!(
        item_named(&local_tree, "scores").detail.as_deref(),
        Some("tree[int]")
    );
}

#[test]
fn source_saved_path_completion_fact_returns_active_context_and_declared_children() {
    let project = CompletionProject::new();
    let program = project.program();
    let app = project.app_file();

    let (source, offset) = source_with_cursor(
        "module shelf::app\n\nuse shelf::books\n\npub fn f(id: int)\n    const x = ^books(id).|\n",
    );
    let parsed = parse_source(&source);
    let lexed = lex_source(&source);
    let fact = source_saved_path_completion_fact_at(program, app, &source, &parsed, &lexed, offset)
        .expect("saved-path completion fact");

    assert_receiver_span(&source, fact.context.receiver_span, "^books(id)");
    assert_eq!(fact.context.root.name, "books");
    let root_id = fact.context.root.store_id;
    assert_eq!(fact.context.segments.len(), 2);
    match &fact.context.segments[0] {
        SourceSavedPathCompletionSegment::Root {
            name,
            store_id,
            store_catalog_id,
        } => {
            assert_eq!(name, "books");
            assert_eq!(*store_id, root_id);
            assert_eq!(store_catalog_id.as_ref().map(|id| id.as_str()), None);
        }
        segment => panic!("expected root segment, got {segment:?}"),
    }
    match &fact.context.segments[1] {
        SourceSavedPathCompletionSegment::KeySlot { name, scalar } => {
            assert_eq!(name, "id");
            assert_eq!(*scalar, Some(ScalarType::Int));
        }
        segment => panic!("expected root key slot, got {segment:?}"),
    }
    assert_eq!(
        fact.children
            .iter()
            .map(|child| child.name.as_str())
            .collect::<Vec<_>>(),
        ["title", "notes"]
    );

    let (source, offset) = source_with_cursor(
        "module shelf::app\n\nuse shelf::books\n\npub fn f(id: int, n: string)\n    const x = ^books(id).notes(n).|\n",
    );
    let parsed = parse_source(&source);
    let lexed = lex_source(&source);
    let fact = source_saved_path_completion_fact_at(program, app, &source, &parsed, &lexed, offset)
        .expect("saved-layer completion fact");
    assert_eq!(fact.context.segments.len(), 4);
    match &fact.context.segments[2] {
        SourceSavedPathCompletionSegment::Layer {
            name,
            member_id,
            member_catalog_id,
        } => {
            assert_eq!(name, "notes");
            assert!(member_id.is_some());
            assert_eq!(member_catalog_id.as_ref().map(|id| id.as_str()), None);
        }
        segment => panic!("expected layer segment, got {segment:?}"),
    }
    match &fact.context.segments[3] {
        SourceSavedPathCompletionSegment::KeySlot { name, scalar } => {
            assert_eq!(name, "noteId");
            assert_eq!(*scalar, Some(ScalarType::Str));
        }
        segment => panic!("expected layer key slot, got {segment:?}"),
    }
    assert_eq!(
        fact.children
            .iter()
            .map(|child| child.name.as_str())
            .collect::<Vec<_>>(),
        ["text"]
    );

    let (source, offset) = source_with_cursor(
        "module shelf::app\n\nuse shelf::books\n\npub fn f(id: int)\n    const x = ^books(id)..|\n",
    );
    let parsed = parse_source(&source);
    let lexed = lex_source(&source);
    assert!(
        source_saved_path_completion_fact_at(program, app, &source, &parsed, &lexed, offset)
            .is_none(),
        "malformed saved-path context must not expose a declared-child fact"
    );
}

#[test]
fn source_completion_fact_adds_expected_enum_members_for_annotated_const_var_and_return() {
    let project = CompletionProject::new();
    let program = project.program();
    let app = project.app_file();

    for (label, source, prefix, assert_replacement) in [
        (
            "annotated const initializer",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const state: Status = a|\n",
            "Status",
            true,
        ),
        (
            "annotated var initializer",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    var state: Status = a|\n",
            "Status",
            true,
        ),
        (
            "qualified annotated const initializer",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const state: books::Status = a|\n",
            "books::Status",
            true,
        ),
        (
            "enum return expression",
            "module shelf::app\n\nuse shelf::books\n\npub fn f(): Status\n    return a|\n",
            "Status",
            true,
        ),
        (
            "nested enum return expression",
            "module shelf::app\n\nuse shelf::books\n\npub fn f(): Status\n    if true\n        return a|\n    return Status::active\n",
            "Status",
            true,
        ),
        (
            "function enum argument",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const state = books::chooseStatus(\"current\", a|)\n",
            "Status",
            true,
        ),
        (
            "resource constructor enum field",
            "module shelf::app\n\nuse shelf::books\n\nresource Draft\n    required state: books::Status\n\npub fn f()\n    const draft = Draft(state: a|)\n",
            "Status",
            false,
        ),
    ] {
        let items = completion_items(program, app, source);
        let active_label = format!("{prefix}::active");
        let active = item_named(&items, &active_label);
        assert_eq!(active.kind, SourceCompletionItemKind::EnumMember, "{label}");
        assert_eq!(active.detail.as_deref(), Some("Status"), "{label}");
        assert_eq!(active.docs, ["Ready for use."], "{label}");

        let retired_label = format!("{prefix}::archived::retired");
        let retired = item_named(&items, &retired_label);
        assert_eq!(
            retired.kind,
            SourceCompletionItemKind::EnumMember,
            "{label}"
        );
        assert_eq!(retired.detail.as_deref(), Some("Status"), "{label}");
        assert_eq!(retired.docs, ["No longer active."], "{label}");

        let category_label = format!("{prefix}::archived");
        assert!(
            !items.iter().any(|item| item.label == category_label),
            "{label}: expected enum completions must not include category members: {items:?}"
        );
        assert!(
            !items.iter().any(|item| item.label.ends_with("hidden")),
            "{label}: expected enum completions must not include private sibling enum members: {items:?}"
        );
        assert!(
            items.iter().any(|item| item.label == "return"),
            "{label}: expected enum completions should stay additive with bare completions"
        );
        if assert_replacement {
            project.assert_app_source_has_no_app_errors(&source.replacen("a|", &active_label, 1));
            project.assert_app_source_has_no_app_errors(&source.replacen("a|", &retired_label, 1));
        }
    }

    let first_arg_items = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const state = books::chooseStatus(a|, Status::active)\n",
    );
    assert!(
        !first_arg_items
            .iter()
            .any(|item| item.label == "Status::active"),
        "string arguments must not receive enum value completions: {first_arg_items:?}"
    );

    let constructor_field_name_items = completion_items(
        program,
        app,
        "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const draft = Draft(st|)\n",
    );
    assert!(
        !constructor_field_name_items
            .iter()
            .any(|item| item.label == "Status::active"),
        "constructor field-name positions must not receive enum value completions: {constructor_field_name_items:?}"
    );

    let ambiguous_project = CompletionProject::with_archive_books();
    let ambiguous_items = completion_items(
        ambiguous_project.program(),
        ambiguous_project.app_file(),
        "module shelf::app\n\nuse shelf::books\nuse archive::books\n\nenum Status\n    local\n\nresource Draft\n    required state: shelf::books::Status\n\npub fn f()\n    const draft = Draft(state: a|)\n",
    );
    assert_eq!(
        item_named(&ambiguous_items, "shelf::books::Status::active").kind,
        SourceCompletionItemKind::EnumMember,
    );
    assert!(
        !ambiguous_items
            .iter()
            .any(|item| item.label == "books::Status::active"),
        "ambiguous import aliases must not be used as enum value prefixes: {ambiguous_items:?}"
    );
}

fn completion_items(
    program: &CheckedProgram,
    file: &Path,
    source: &str,
) -> Vec<SourceCompletionItem> {
    let (source, offset) = source_with_cursor(source);
    let parsed = parse_source(&source);
    let lexed = lex_source(&source);
    source_completion_fact(program, file, &source, &parsed, &lexed, offset).items
}

fn source_with_cursor(source: &str) -> (String, usize) {
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    (source, offset)
}

fn item_named<'a>(items: &'a [SourceCompletionItem], label: &str) -> &'a SourceCompletionItem {
    items
        .iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| panic!("expected completion {label:?}, got {items:?}"))
}

struct CompletionProject {
    root: PathBuf,
    program: CheckedProgram,
    app: PathBuf,
}

impl CompletionProject {
    fn new() -> Self {
        Self::new_with_archive_books(false)
    }

    fn with_archive_books() -> Self {
        Self::new_with_archive_books(true)
    }

    fn new_with_archive_books(include_archive_books: bool) -> Self {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src/shelf")).expect("create project dirs");
        if include_archive_books {
            std::fs::create_dir_all(root.join("src/archive")).expect("create archive project dirs");
        }
        std::fs::write(
            root.join("marrow.json"),
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        )
        .expect("write config");
        std::fs::write(root.join("src/shelf/books.mw"), BOOKS).expect("write books");
        if include_archive_books {
            std::fs::write(root.join("src/archive/books.mw"), ARCHIVE_BOOKS)
                .expect("write archive books");
        }
        let app = root.join("src/shelf/app.mw");
        std::fs::write(&app, APP).expect("write app");
        let config =
            parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
                .expect("parse config");
        let snapshot =
            analyze_project(&root, &config, &ProjectSources::new(), None, None).expect("analyze");
        Self {
            root,
            program: snapshot.program,
            app,
        }
    }

    fn program(&self) -> &CheckedProgram {
        &self.program
    }

    fn app_file(&self) -> &Path {
        &self.app
    }

    fn assert_app_source_has_no_app_errors(&self, source: &str) {
        std::fs::write(&self.app, source).expect("write app source");
        let config =
            parse_config(r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#)
                .expect("parse config");
        let snapshot = analyze_project(&self.root, &config, &ProjectSources::new(), None, None)
            .expect("analyze completed source");
        let app_errors = snapshot
            .report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.file == self.app && diagnostic.severity == Severity::Error
            })
            .collect::<Vec<_>>();
        assert!(
            app_errors.is_empty(),
            "unexpected app errors: {app_errors:?}"
        );
    }
}

impl Drop for CompletionProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn unique_temp_dir() -> PathBuf {
    let name = format!(
        "marrow-completion-fact-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos(),
        NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed)
    );
    std::env::temp_dir().join(name)
}

const BOOKS: &str = "\
module shelf::books

;; Book resource docs.
resource Book
    ;; Display title.
    required title: string
    notes(noteId: string)
        text: string

;; Books saved by id.
store ^books(id: int): Book

;; Lifecycle state.
pub enum Status
    ;; Ready for use.
    active
    category archived
        ;; No longer active.
        retired

enum Secret
    hidden

;; Returns a book title.
pub fn titleOf(id: Id(^books)): string
    return ^books(id).title

;; Keeps a lifecycle state.
pub fn chooseStatus(label: string, state: Status): Status
    return state
";

const ARCHIVE_BOOKS: &str = "\
module archive::books

pub enum Status
    inactive
";

const APP: &str = "\
module shelf::app

use shelf::books

resource Draft
    required state: books::Status

pub fn run(count: int): int
    const total: int = count
    return total
";
