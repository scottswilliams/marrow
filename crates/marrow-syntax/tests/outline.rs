use marrow_syntax::{Declaration, ResourceMember, parse_source};

fn reference_sample() -> &'static str {
    r#"module shelf::sample

resource Book at ^books(id: int)
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string
    tags(pos: int): string

    notes(noteId: string)
        text: string

    versions(version: int)
        required title: string
        required shelf: string
        required changedAt: instant

    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string, changedAt: instant): Book::Id
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    book.currentVersion = 1

    let id: Book::Id = nextId(^books)

    transaction
        ^books(id) = book
        ^books(id).versions(1).title = title
        ^books(id).versions(1).shelf = shelf
        ^books(id).versions(1).changedAt = changedAt

    return id

pub fn printShelf(shelf: string)
    for id in keys(^books.byShelf(shelf))
        print($"{id}: {^books(id).title}")
"#
}

#[test]
fn parses_documented_reference_sample() {
    let sample_doc = include_str!("../../../docs/language/sample.md");
    let sample = sample_doc
        .split("```mw")
        .nth(1)
        .and_then(|tail| tail.split("```").next())
        .expect("sample.md should contain a Marrow code block");

    let parsed = parse_source(sample);

    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics from docs/language/sample.md: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed
            .file
            .module
            .as_ref()
            .map(|module| module.name.as_str()),
        Some("shelf::sample")
    );
    assert!(parsed.file.resource("Book").is_some());
    assert!(parsed.file.function("main").is_some());
}

#[test]
fn parses_reference_sample_outline() {
    let parsed = parse_source(reference_sample());

    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed.file.module.as_ref().map(|m| m.name.as_str()),
        Some("shelf::sample")
    );

    let book = parsed.file.resource("Book").expect("Book resource");
    let store = book.store.as_ref().expect("saved root");
    assert_eq!(store.root, "books");
    assert_eq!(store.keys[0].name, "id");
    assert_eq!(store.keys[0].ty.text, "int");

    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Field(field)
            if field.required && field.name == "title" && field.ty.text == "string"
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Field(field)
            if !field.required
                && field.name == "tags"
                && field.keys.len() == 1
                && field.ty.text == "string"
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Group(group)
            if group.name == "versions"
                && group.keys.len() == 1
                && group.members.iter().any(|child| matches!(
                    child,
                    ResourceMember::Field(field)
                        if field.required
                            && field.name == "changedAt"
                            && field.ty.text == "instant"
                ))
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Index(index)
            if index.name == "byShelf"
                && index.args == ["shelf", "id"]
                && !index.unique
    )));

    let add = parsed.file.function("add").expect("add function");
    assert!(add.public);
    assert_eq!(
        add.params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        ["title", "author", "shelf", "changedAt"]
    );
    assert_eq!(
        add.return_type.as_ref().map(|ty| ty.text.as_str()),
        Some("Book::Id")
    );
}

#[test]
fn attaches_doc_comments_and_stable_ids_to_resource_members() {
    let parsed = parse_source(
        r#"module shelf::books

resource Book at ^books(id: int)
    ;; Display title.
    @id("book.title")
    required title: string
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let book = parsed.file.resource("Book").expect("Book resource");
    let ResourceMember::Field(title) = &book.members[0] else {
        panic!("expected field, got {:?}", book.members[0]);
    };
    assert_eq!(title.docs, ["Display title."]);
    assert_eq!(title.stable_id.as_deref(), Some("book.title"));
}

#[test]
fn rejects_tabs_because_marrow_blocks_are_space_indented() {
    let parsed = parse_source("module app\n\tpub fn main()\n");

    assert!(parsed.has_errors());
    assert_eq!(parsed.diagnostics[0].code, "parse.syntax");
    assert_eq!(parsed.diagnostics[0].line, 2);
    assert_eq!(parsed.diagnostics[0].column, 1);
    assert!(parsed.diagnostics[0].message.contains("tabs"));
    let tab_reports = parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("tabs"))
        .count();
    assert_eq!(tab_reports, 1, "{:#?}", parsed.diagnostics);
}

#[test]
fn surfaces_lexer_diagnostics_for_function_body_tokens() {
    let parsed = parse_source("module app\nfn main()\n    return a == b\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let obsolete = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`==`"))
        .expect("expected obsolete operator diagnostic");
    assert_eq!(obsolete.code, "parse.syntax");
    assert_eq!(obsolete.kind, "parse");
    assert_eq!(obsolete.line, 3);
    assert_eq!(
        obsolete.help.as_deref(),
        Some("Use `=` for equality."),
        "{:#?}",
        obsolete.help
    );
}

#[test]
fn rejects_parameter_defaults() {
    let parsed = parse_source("module app\nfn f(x: int = 5)\n    return\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("parameter defaults"))
        .expect("expected parameter-defaults diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.kind, "parse");
    assert_eq!(diagnostic.line, 2);
    assert!(
        !diagnostic.message.contains("expected"),
        "diagnostic should not fall back to a generic message, got {:?}",
        diagnostic.message
    );
}

#[test]
fn rejects_user_defined_generics_on_functions() {
    let parsed = parse_source("module app\nfn f<T>(x: T)\n    return\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("user-defined generics"))
        .expect("expected user-defined-generics diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.line, 2);
}

#[test]
fn rejects_top_level_type_aliases() {
    let parsed = parse_source("module app\ntype Title = string\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("type aliases"))
        .expect("expected type-aliases diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.line, 2);
}

#[test]
fn merges_lexer_and_parser_diagnostics_in_source_order() {
    let parsed = parse_source(concat!(
        "module ;-bad-name\n",
        "fn main()\n",
        "    return ~~~\n",
    ));

    assert!(parsed.has_errors());
    let mut lines = parsed
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.line)
        .collect::<Vec<_>>();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted, "diagnostics not in source order: {lines:?}");
    lines.dedup();
    assert!(
        lines.contains(&1) && lines.contains(&3),
        "expected diagnostics on lines 1 and 3, saw {lines:?}"
    );
}

#[test]
fn rejects_internal_and_private_visibility() {
    for visibility in ["internal", "private"] {
        let parsed = parse_source(&format!("module app\n{visibility} fn main()\n    return\n"));

        assert!(parsed.has_errors(), "expected error for {visibility}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("pub")
                    && diagnostic.message.contains("module-private")),
            "diagnostics for {visibility}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn requires_indented_resource_and_function_bodies() {
    let parsed = parse_source(
        r#"module app
resource Book at ^books(id: int)
pub fn main()
"#,
    );

    assert_eq!(parsed.diagnostics.len(), 2, "{:#?}", parsed.diagnostics);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("resource body"))
    );
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body"))
    );
}

#[test]
fn rejects_resource_members_nested_under_fields() {
    let parsed = parse_source(
        r#"module app
resource Book
    title: string
        nested: string
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unexpected indentation")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_empty_saved_root_key_lists() {
    let parsed = parse_source(
        r#"module app
resource Book at ^books()
    title: string
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("key")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_empty_index_argument_lists() {
    let parsed = parse_source(
        r#"module app
resource Book at ^books(id: int)
    title: string
    index empty()
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("index argument")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_const_without_value() {
    let parsed = parse_source(
        r#"module app
const MaxLoans: int
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("const")
                && diagnostic.message.contains("=")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_module_names() {
    let parsed = parse_source("module 123\n");

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("module name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_import_names() {
    let parsed = parse_source(
        r#"module app
use *
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("import name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_const_names() {
    let parsed = parse_source(
        r#"module app
const : int = 1
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("const name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_malformed_type_annotations() {
    for source in [
        "module app\nconst Max: = 1\n",
        "module app\nfn main(value:)\n    return\n",
        "module app\nresource Book at ^books(id:)\n    title: string\n",
        "module app\nresource Book\n    title: sequence[]\n",
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("type")),
            "diagnostics for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_malformed_index_field_paths() {
    for source in [
        "module app\nresource Book at ^books(id: int)\n    title: string\n    index bad(title.)\n",
        "module app\nresource Book at ^books(id: int)\n    title: string\n    index bad(.title)\n",
        "module app\nresource Book at ^books(id: int)\n    title: string\n    index bad(title.*)\n",
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("index field path")),
            "diagnostics for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_late_or_duplicate_module_declarations() {
    let parsed = parse_source(
        r#"module app
fn main()
    return
module later
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("module declaration")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn keeps_top_level_declarations_in_source_order() {
    let parsed = parse_source(
        r#"module app
const MaxLoans: int = 5
resource Book
    title: string
fn normalize(title: string): string
    return title
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let names = parsed
        .file
        .declarations
        .iter()
        .map(|decl| match decl {
            Declaration::Const(decl) => decl.name.as_str(),
            Declaration::Resource(decl) => decl.name.as_str(),
            Declaration::Function(decl) => decl.name.as_str(),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["MaxLoans", "Book", "normalize"]);
}
