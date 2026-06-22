use std::path::Path;

use crate::support;
use marrow_check::tooling::{
    SourceSemanticTokenFact, SourceSemanticTokenModifiers, SourceSemanticTokenRole,
    source_semantic_token_facts, source_semantic_token_facts_for_file,
};
use marrow_check::{
    AnalysisSnapshot, ProjectSources, analyze_project, build_binding_index, check_project,
};
use marrow_syntax::{lex_source, parse_source};

fn syntax_facts(source: &str) -> Vec<SourceSemanticTokenFact> {
    let lexed = lex_source(source);
    let parsed = parse_source(source);
    source_semantic_token_facts(source, &lexed, &parsed)
}

fn checked_facts(name: &str, source: &str) -> Vec<SourceSemanticTokenFact> {
    let root = support::temp_root(name);
    support::write(&root, "src/m.mw", source);
    let path = root.join("src/m.mw");
    let (report, program) = check_project(&root, &support::config()).expect("baseline check");
    support::assert_clean(&report);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("baseline check should propose catalog ids");
    let mut sources = ProjectSources::new();
    sources.insert(&path, source);
    let snapshot = analyze_project(&root, &support::config(), &sources, Some(&accepted), None)
        .expect("analyze");
    support::assert_clean(&snapshot.report);
    source_facts_for_file(&snapshot, &path).expect("snapshot-bound facts")
}

fn source_facts_for_file(
    snapshot: &AnalysisSnapshot,
    path: &Path,
) -> Option<Vec<SourceSemanticTokenFact>> {
    let binding_index = build_binding_index(snapshot);
    source_semantic_token_facts_for_file(snapshot, &binding_index, path)
}

fn fact_at<'a>(
    source: &str,
    facts: &'a [SourceSemanticTokenFact],
    line: &str,
    lexeme: &str,
) -> &'a SourceSemanticTokenFact {
    let line_start = source
        .find(line)
        .unwrap_or_else(|| panic!("source should contain line {line:?}"));
    let in_line = line
        .find(lexeme)
        .unwrap_or_else(|| panic!("line {line:?} should contain {lexeme:?}"));
    let start = line_start + in_line;
    let end = start + lexeme.len();
    facts
        .iter()
        .find(|fact| fact.span.start_byte == start && fact.span.end_byte == end)
        .unwrap_or_else(|| panic!("semantic token fact for {lexeme:?} on {line:?} should exist"))
}

fn assert_role(
    source: &str,
    facts: &[SourceSemanticTokenFact],
    line: &str,
    lexeme: &str,
    role: SourceSemanticTokenRole,
) {
    assert_fact(
        source,
        facts,
        line,
        lexeme,
        role,
        SourceSemanticTokenModifiers::default(),
    );
}

fn assert_fact(
    source: &str,
    facts: &[SourceSemanticTokenFact],
    line: &str,
    lexeme: &str,
    role: SourceSemanticTokenRole,
    modifiers: SourceSemanticTokenModifiers,
) {
    let fact = fact_at(source, facts, line, lexeme);
    assert_eq!(fact.role, role, "{lexeme:?} on {line:?} has role");
    assert_eq!(
        fact.modifiers, modifiers,
        "{lexeme:?} on {line:?} has modifiers"
    );
}

#[test]
fn syntax_roles_are_reported_as_transport_neutral_facts() {
    let source = "module a\n\nconst N: int = 42 ; count\nfn f(value: bool): bool\n    return value and true\n";
    let facts = syntax_facts(source);

    assert_role(
        source,
        &facts,
        "module a",
        "module",
        SourceSemanticTokenRole::Keyword,
    );
    assert_role(
        source,
        &facts,
        "    return value and true",
        "value",
        SourceSemanticTokenRole::Variable,
    );
    assert_role(
        source,
        &facts,
        "const N: int = 42 ; count",
        "int",
        SourceSemanticTokenRole::TypeKeyword,
    );
    assert_role(
        source,
        &facts,
        "const N: int = 42 ; count",
        "42",
        SourceSemanticTokenRole::NumberLiteral,
    );
    assert_role(
        source,
        &facts,
        "const N: int = 42 ; count",
        "; count",
        SourceSemanticTokenRole::Comment,
    );
    assert_role(
        source,
        &facts,
        "    return value and true",
        "and",
        SourceSemanticTokenRole::Operator,
    );
    assert_role(
        source,
        &facts,
        "    return value and true",
        "true",
        SourceSemanticTokenRole::BooleanLiteral,
    );
}

#[test]
fn declaration_roles_are_reported_as_transport_neutral_facts() {
    let source = "\
module shelf::catalog
use shared::imports

const LIMIT: int = 10

resource Book
    required title: string
    notes(note_id: string)
        text: string
    tags(pos: int): string

store ^books(id: int): Book
    index byTitle(title, id)

surface Books from ^books
    fields title

pub fn paint(book_id: int, label: string): int
    return book_id

pub enum Genre
    Fiction
        Literary
    Nonfiction

evolve
    rename Book.title -> Book.name
";
    let facts = syntax_facts(source);

    assert_role(
        source,
        &facts,
        "module shelf::catalog",
        "shelf",
        SourceSemanticTokenRole::Namespace,
    );
    assert_role(
        source,
        &facts,
        "module shelf::catalog",
        "catalog",
        SourceSemanticTokenRole::Namespace,
    );
    assert_role(
        source,
        &facts,
        "use shared::imports",
        "shared",
        SourceSemanticTokenRole::Namespace,
    );
    assert_role(
        source,
        &facts,
        "use shared::imports",
        "imports",
        SourceSemanticTokenRole::Namespace,
    );
    assert_fact(
        source,
        &facts,
        "const LIMIT: int = 10",
        "LIMIT",
        SourceSemanticTokenRole::Variable,
        SourceSemanticTokenModifiers {
            readonly: true,
            ..Default::default()
        },
    );
    assert_role(
        source,
        &facts,
        "resource Book",
        "Book",
        SourceSemanticTokenRole::Resource,
    );
    assert_role(
        source,
        &facts,
        "    required title: string",
        "title",
        SourceSemanticTokenRole::ResourceMember,
    );
    assert_role(
        source,
        &facts,
        "    notes(note_id: string)",
        "notes",
        SourceSemanticTokenRole::ResourceMember,
    );
    assert_role(
        source,
        &facts,
        "    notes(note_id: string)",
        "note_id",
        SourceSemanticTokenRole::KeyParameter,
    );
    assert_role(
        source,
        &facts,
        "    tags(pos: int): string",
        "tags",
        SourceSemanticTokenRole::ResourceMember,
    );
    assert_role(
        source,
        &facts,
        "    tags(pos: int): string",
        "pos",
        SourceSemanticTokenRole::KeyParameter,
    );
    assert_fact(
        source,
        &facts,
        "store ^books(id: int): Book",
        "^",
        SourceSemanticTokenRole::SavedRoot,
        SourceSemanticTokenModifiers {
            modification: true,
            ..Default::default()
        },
    );
    assert_fact(
        source,
        &facts,
        "store ^books(id: int): Book",
        "books",
        SourceSemanticTokenRole::SavedRoot,
        SourceSemanticTokenModifiers {
            modification: true,
            ..Default::default()
        },
    );
    assert_role(
        source,
        &facts,
        "store ^books(id: int): Book",
        "id",
        SourceSemanticTokenRole::KeyParameter,
    );
    assert_role(
        source,
        &facts,
        "    index byTitle(title, id)",
        "byTitle",
        SourceSemanticTokenRole::Index,
    );
    assert_role(
        source,
        &facts,
        "    index byTitle(title, id)",
        "title",
        SourceSemanticTokenRole::KeyParameter,
    );
    assert_role(
        source,
        &facts,
        "surface Books from ^books",
        "Books",
        SourceSemanticTokenRole::Surface,
    );
    assert_role(
        source,
        &facts,
        "pub fn paint(book_id: int, label: string): int",
        "paint",
        SourceSemanticTokenRole::Function,
    );
    assert_role(
        source,
        &facts,
        "pub fn paint(book_id: int, label: string): int",
        "book_id",
        SourceSemanticTokenRole::Parameter,
    );
    assert_role(
        source,
        &facts,
        "pub enum Genre",
        "Genre",
        SourceSemanticTokenRole::Enum,
    );
    assert_role(
        source,
        &facts,
        "    Fiction",
        "Fiction",
        SourceSemanticTokenRole::EnumMember,
    );
    assert_role(
        source,
        &facts,
        "        Literary",
        "Literary",
        SourceSemanticTokenRole::EnumMember,
    );
    assert_role(
        source,
        &facts,
        "    rename Book.title -> Book.name",
        "rename",
        SourceSemanticTokenRole::Keyword,
    );
}

#[test]
fn checked_reference_and_builtin_roles_come_from_marrow_facts() {
    let source = "\
module m

resource Book
    required title: string
    tags(pos: int): string

store ^books(id: int): Book
    index byTitle(title, id)

const LIMIT: int = 10

pub enum Status
    active

fn helper(id: int): int
    return id

fn paint(id: int, title: string): int
    const book = Book(title: title)
    const status = Status::active
    const found = ^books(id).title ?? \"\"
    const tag = ^books(id).tags(1) ?? \"\"
    const lookup = exists(^books.byTitle(title, id))
    const len = std::text::length(\"abc\")
    const copy = LIMIT
    return helper(id)
";
    let facts = checked_facts("source-semantic-token-checked-refs", source);

    assert_role(
        source,
        &facts,
        "    return helper(id)",
        "helper",
        SourceSemanticTokenRole::Function,
    );
    assert_role(
        source,
        &facts,
        "    return helper(id)",
        "id",
        SourceSemanticTokenRole::Parameter,
    );
    assert_role(
        source,
        &facts,
        "    const book = Book(title: title)",
        "Book",
        SourceSemanticTokenRole::Resource,
    );
    assert_role(
        source,
        &facts,
        "    const status = Status::active",
        "Status",
        SourceSemanticTokenRole::Enum,
    );
    assert_role(
        source,
        &facts,
        "    const status = Status::active",
        "active",
        SourceSemanticTokenRole::EnumMember,
    );
    assert_role(
        source,
        &facts,
        "    const found = ^books(id).title ?? \"\"",
        "title",
        SourceSemanticTokenRole::ResourceMember,
    );
    assert_role(
        source,
        &facts,
        "    const tag = ^books(id).tags(1) ?? \"\"",
        "tags",
        SourceSemanticTokenRole::ResourceMember,
    );
    assert_role(
        source,
        &facts,
        "    const lookup = exists(^books.byTitle(title, id))",
        "byTitle",
        SourceSemanticTokenRole::Index,
    );
    assert_fact(
        source,
        &facts,
        "    const len = std::text::length(\"abc\")",
        "std",
        SourceSemanticTokenRole::Namespace,
        SourceSemanticTokenModifiers {
            default_library: true,
            ..Default::default()
        },
    );
    assert_fact(
        source,
        &facts,
        "    const len = std::text::length(\"abc\")",
        "length",
        SourceSemanticTokenRole::Function,
        SourceSemanticTokenModifiers {
            default_library: true,
            ..Default::default()
        },
    );
    assert_fact(
        source,
        &facts,
        "    const copy = LIMIT",
        "LIMIT",
        SourceSemanticTokenRole::Variable,
        SourceSemanticTokenModifiers {
            readonly: true,
            ..Default::default()
        },
    );
}

#[test]
fn checked_identity_type_annotations_report_constructor_facts() {
    let source = "\
module m

resource Author
    name: string

store ^authors(id: int): Author

pub fn f(
    id: Id(^authors),
): Id(^authors)
    return id
";
    let facts = checked_facts("source-semantic-token-identity-types", source);

    assert_role(
        source,
        &facts,
        "    id: Id(^authors),",
        "Id",
        SourceSemanticTokenRole::IdentityTypeConstructor,
    );
    assert_role(
        source,
        &facts,
        "): Id(^authors)",
        "Id",
        SourceSemanticTokenRole::IdentityTypeConstructor,
    );
}

#[test]
fn checked_facts_are_unavailable_for_files_outside_the_snapshot() {
    let source = "\
module m

fn f(): int
    return 1
";
    let root = support::temp_root("source-semantic-token-outside-file");
    support::write(&root, "src/m.mw", source);
    let snapshot = analyze_project(
        &root,
        &support::config(),
        &ProjectSources::new(),
        None,
        None,
    )
    .expect("analyze");
    support::assert_clean(&snapshot.report);
    let binding_index = build_binding_index(&snapshot);

    let missing = root.join("src/other.mw");
    assert_eq!(
        source_semantic_token_facts_for_file(&snapshot, &binding_index, &missing),
        None
    );
}

#[test]
fn checked_public_api_signature_is_snapshot_bound() {
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(crate_root.join("src/tooling/semantic_tokens/mod.rs"))
        .or_else(|_| std::fs::read_to_string(crate_root.join("src/tooling/semantic_tokens.rs")))
        .expect("semantic token tooling module");
    let signature_start = source
        .find("pub fn source_semantic_token_facts_for_file")
        .expect("checked semantic token facts API");
    let signature_tail = &source[signature_start..];
    let signature = signature_tail
        .split_once(") ->")
        .map(|(signature, _)| signature)
        .expect("function signature closes before return type");

    assert!(signature.contains("snapshot: &AnalysisSnapshot"));
    assert!(signature.contains("binding_index: &BindingIndex"));
    assert!(signature.contains("file: &Path"));
    for forbidden in ["source:", "lexed:", "parsed:"] {
        assert!(
            !signature.contains(forbidden),
            "checked semantic-token API must not accept caller-supplied {forbidden}"
        );
    }
}
