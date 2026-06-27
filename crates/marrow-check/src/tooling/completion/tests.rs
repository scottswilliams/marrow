use super::*;
use crate::{ProjectSources, analyze_project};
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
    let SourceCompletionContext::SavedPath { receiver, span } = context else {
        panic!("expected saved-path context, got {context:?}");
    };
    assert_eq!(receiver, "^books(id)");
    assert_receiver_span(&source, span, "^books(id)");

    let (_, context) =
        context_at("module shelf::app\n\npub fn f(id: int)\n    const x = ^books(id)..|\n");
    assert_eq!(context, SourceCompletionContext::InvalidSavedPath);

    let (_, context) = context_at("module shelf::app\n\npub fn f(x: |\n");
    assert_eq!(context, SourceCompletionContext::Type);

    let (_, context) = context_at("module shelf::app\n\npub fn f(total: int)\n    return t|\n");
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
fn source_completion_fact_adds_expected_enum_members_for_annotated_const_var_and_return() {
    let project = CompletionProject::new();
    let program = project.program();
    let app = project.app_file();

    for (label, source, prefix) in [
        (
            "annotated const initializer",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const state: Status = a|\n",
            "Status",
        ),
        (
            "annotated var initializer",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    var state: Status = a|\n",
            "Status",
        ),
        (
            "qualified annotated const initializer",
            "module shelf::app\n\nuse shelf::books\n\npub fn f()\n    const state: books::Status = a|\n",
            "books::Status",
        ),
        (
            "enum return expression",
            "module shelf::app\n\nuse shelf::books\n\npub fn f(): Status\n    return a|\n",
            "Status",
        ),
        (
            "nested enum return expression",
            "module shelf::app\n\nuse shelf::books\n\npub fn f(): Status\n    if true\n        return a|\n    return Status::active\n",
            "Status",
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
        project.assert_app_source_has_no_app_errors(&source.replacen("a|", &active_label, 1));
        project.assert_app_source_has_no_app_errors(&source.replacen("a|", &retired_label, 1));
    }
}

fn completion_items(
    program: &CheckedProgram,
    file: &Path,
    source: &str,
) -> Vec<SourceCompletionItem> {
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    let parsed = parse_source(&source);
    let lexed = lex_source(&source);
    source_completion_fact(program, file, &source, &parsed, &lexed, offset).items
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
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("src/shelf")).expect("create project dirs");
        std::fs::write(
            root.join("marrow.json"),
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        )
        .expect("write config");
        std::fs::write(root.join("src/shelf/books.mw"), BOOKS).expect("write books");
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
";

const APP: &str = "\
module shelf::app

use shelf::books

pub fn run(count: int): int
    const total: int = count
    return total
";
