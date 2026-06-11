mod support;

use marrow_check::{
    CheckedType, DiagnosticPayload, HostEffect, SavedPlaceEffect, check_project,
    check_tests_program,
};
use marrow_project::parse_config;
use marrow_schema::stdlib::Capability;

use support::{config, temp_project, write};

#[test]
fn checked_facts_assign_typed_ids_to_same_named_declarations() {
    let root = temp_project("program-fact-ids", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books_a(id: int): Book\n\
             enum Status\n\
             \x20   active\n\
             pub fn fresh(): Id(^books_a)\n\
             \x20   return nextId(^books_a)\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books_b(id: int): Book\n\
             enum Status\n\
             \x20   active\n\
             pub fn fresh(): Id(^books_b)\n\
             \x20   return nextId(^books_b)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let facts = &program.facts;

    let a = facts.module_id("a").expect("module a");
    let b = facts.module_id("b").expect("module b");
    assert_ne!(a, b);

    let a_book = facts.resource_id(a, "Book").expect("a::Book");
    let b_book = facts.resource_id(b, "Book").expect("b::Book");
    assert_ne!(a_book, b_book);
    let a_books = facts.store_id(a, "books_a").expect("^books_a");
    let b_books = facts.store_id(b, "books_b").expect("^books_b");
    assert_ne!(a_books, b_books);

    let a_status = facts.enum_id(a, "Status").expect("a::Status");
    let b_status = facts.enum_id(b, "Status").expect("b::Status");
    assert_ne!(a_status, b_status);

    let fresh = facts.function_id(a, "fresh").expect("a::fresh");
    assert_eq!(
        facts.function(fresh).return_type.as_ref(),
        Some(&CheckedType::Identity(a_books))
    );
}

#[test]
fn checked_facts_do_not_first_match_bare_foreign_resource_annotations() {
    let root = temp_project("program-no-foreign-resource-fact", |root| {
        write(
            root,
            "src/a.mw",
            "module a\nresource Book\n    title: string\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nfn read(book: Book)\n    print(\"x\")\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == marrow_check::CHECK_UNKNOWN_TYPE
                && diagnostic.payload
                    == DiagnosticPayload::UnknownType(marrow_schema::Type::Named("Book".into()))
        }),
        "{:#?}",
        report.diagnostics
    );
    let facts = &program.facts;
    let b = facts.module_id("b").expect("module b");
    assert!(
        facts.function_id(b, "read").is_none(),
        "invalid bare foreign resource annotation must not produce a first-matched function fact"
    );
}

#[test]
fn checked_facts_record_function_effects_with_typed_places() {
    let root = temp_project("program-fact-effects", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
             fn readTitle(id: Id(^books)): string\n\
             \x20   return ^books(id).title\n\
             fn rename(id: Id(^books), title: string)\n\
             \x20   transaction\n\
             \x20       ^books(id).title = title\n\
             fn addTag(id: Id(^books), tag: string): int\n\
             \x20   return append(^books(id).tags, tag)\n\
             fn logTitle(title: string)\n\
             \x20   print(title)\n\
             fn stamp(): instant\n\
             \x20   return std::clock::now()\n\
             fn fail()\n\
             \x20   throw Error(code: \"books.fail\", message: \"nope\")\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let facts = &program.facts;

    let module = facts.module_id("books").expect("books module");
    let book = facts.resource_id(module, "Book").expect("Book resource");
    let title = facts
        .resource_member_id(book, &["title"])
        .expect("Book.title member");
    let tags = facts
        .resource_member_id(book, &["tags"])
        .expect("Book.tags member");
    let title_fact = facts
        .resource_members()
        .iter()
        .find(|member| member.id == title)
        .expect("Book.title fact");
    assert_eq!(title_fact.span.line, 3);
    let title_place = SavedPlaceEffect {
        resource: book,
        members: vec![title],
    };

    let read = facts.function_id(module, "readTitle").expect("readTitle");
    assert_eq!(facts.function(read).span.line, 6);
    assert_eq!(
        facts.function(read).direct_effects.saved_reads,
        vec![title_place.clone()]
    );
    assert!(facts.function(read).direct_effects.saved_writes.is_empty());

    let rename = facts.function_id(module, "rename").expect("rename");
    assert!(facts.function(rename).direct_effects.transactions);
    assert_eq!(
        facts.function(rename).direct_effects.saved_writes,
        vec![title_place]
    );

    let add_tag = facts.function_id(module, "addTag").expect("addTag");
    assert_eq!(
        facts.function(add_tag).direct_effects.saved_writes,
        vec![SavedPlaceEffect {
            resource: book,
            members: vec![tags],
        }]
    );

    let log = facts.function_id(module, "logTitle").expect("logTitle");
    assert_eq!(
        facts.function(log).direct_effects.host_calls,
        vec![HostEffect::Output]
    );

    let stamp = facts.function_id(module, "stamp").expect("stamp");
    assert_eq!(
        facts.function(stamp).direct_effects.host_calls,
        vec![HostEffect::Capability(Capability::Clock)]
    );

    let fail = facts.function_id(module, "fail").expect("fail");
    assert!(facts.function(fail).direct_effects.throws);
}

#[test]
fn duplicate_named_functions_keep_their_own_direct_effects() {
    // Two `fn dup` declarations in one module are a hard error, but both still
    // become facts. Each fact must carry the effects of its OWN body — the reader
    // reads `^books`, the writer writes it — addressed by the fact's stable
    // `source_index`. A by-name remap would attribute the first body's effects to
    // both, so the writer would lose its write and gain a phantom read.
    let root = temp_project("program-fact-duplicate-effects", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn dup(id: Id(^books)): string\n\
             \x20   return ^books(id).title\n\
             fn dup(id: Id(^books), title: string)\n\
             \x20   transaction\n\
             \x20       ^books(id).title = title\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    // Duplicate declarations are rejected, so the program is in error; the facts
    // are still built for both bodies.
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_DUPLICATE_DECLARATION),
        "{:#?}",
        report.diagnostics
    );

    let module = &program.modules[0];
    let dup_facts: Vec<&marrow_check::FunctionFact> = program
        .facts
        .functions()
        .iter()
        .filter(|fact| fact.name == "dup")
        .collect();
    assert_eq!(dup_facts.len(), 2, "both `dup` bodies are facts");

    for fact in dup_facts {
        // The source function the fact points at is the one whose effects it must
        // carry: the reader (one param) reads, the writer (two params) writes.
        let source = &module.functions[fact.source_index as usize];
        let effects = &fact.direct_effects;
        if source.params.len() == 1 {
            assert!(!effects.saved_reads.is_empty(), "reader keeps its read");
            assert!(effects.saved_writes.is_empty(), "reader has no write");
        } else {
            assert!(!effects.saved_writes.is_empty(), "writer keeps its write");
            assert!(effects.saved_reads.is_empty(), "writer has no read");
        }
    }
}

#[test]
fn checked_facts_resolve_qualified_resource_annotations_to_the_owner() {
    let root = temp_project("program-fact-resource-owner", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^a_books(id: int): Book\n\
             fn borrowed(id: Id(^b_books)): Id(^b_books)\n\
             \x20   return id\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^b_books(id: int): Book\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let facts = &program.facts;
    let a = facts.module_id("a").expect("module a");
    let b = facts.module_id("b").expect("module b");
    let a_book = facts.resource_id(a, "Book").expect("a::Book");
    let b_book = facts.resource_id(b, "Book").expect("b::Book");
    assert_ne!(a_book, b_book);

    let borrowed = facts.function(facts.function_id(a, "borrowed").expect("borrowed"));
    let b_books = facts.store_id(b, "b_books").expect("b::^b_books");
    assert_eq!(borrowed.params[0].ty, CheckedType::Identity(b_books));
    assert_eq!(borrowed.return_type, Some(CheckedType::Identity(b_books)));
}

#[test]
fn checked_test_program_preserves_source_facts_and_resolves_test_facts() {
    let root = temp_project("program-test-fact-resource-owner", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^a_books(id: int): Book\n\
             pub fn borrowed(id: Id(^b_books)): Id(^b_books)\n\
             \x20   return id\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^b_books(id: int): Book\n",
        );
        write(
            root,
            "tests/facts_test.mw",
            "use b\n\
             fn helper(id: Id(^b_books)): Id(^b_books)\n\
             \x20   return id\n\
             pub fn smoke()\n\
             \x20   return\n",
        );
    });
    let cfg =
        parse_config(r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#).expect("config");
    let (src_report, src_program) = check_project(&root, &cfg).expect("check source");
    let (test_report, combined) =
        check_tests_program(&root, &cfg, &src_program).expect("check tests");

    assert!(!src_report.has_errors(), "{:#?}", src_report.diagnostics);
    assert!(!test_report.has_errors(), "{:#?}", test_report.diagnostics);
    let facts = &combined.facts;
    let a = facts.module_id("a").expect("module a");
    let b = facts.module_id("b").expect("module b");
    let test = facts
        .module_id("tests::facts_test")
        .expect("tests::facts_test");
    let a_book = facts.resource_id(a, "Book").expect("a::Book");
    let b_book = facts.resource_id(b, "Book").expect("b::Book");
    assert_ne!(a_book, b_book);

    let borrowed = facts.function(facts.function_id(a, "borrowed").expect("borrowed"));
    let b_books = facts.store_id(b, "b_books").expect("b::^b_books");
    assert_eq!(borrowed.params[0].ty, CheckedType::Identity(b_books));
    assert_eq!(borrowed.return_type, Some(CheckedType::Identity(b_books)));

    let helper = facts.function(facts.function_id(test, "helper").expect("helper"));
    assert_eq!(helper.params[0].ty, CheckedType::Identity(b_books));
    assert_eq!(helper.return_type, Some(CheckedType::Identity(b_books)));
}

#[test]
fn checked_facts_fail_closed_for_invalid_saved_places_and_signatures() {
    let root = temp_project("program-fact-fail-closed", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   notes(pos: int)\n\
             \x20       text: string\n\
             store ^books(id: int): Book\n\
             fn badPath()\n\
             \x20   ^books(1).notes(1).missing\n\
             fn badSignature(id: Missing): int\n\
             \x20   return 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(report.has_errors(), "{:#?}", report.diagnostics);
    let facts = &program.facts;
    let module = facts.module_id("books").expect("books module");
    let bad_path = facts.function_id(module, "badPath").expect("badPath");
    assert!(
        facts
            .function(bad_path)
            .direct_effects
            .saved_reads
            .is_empty(),
        "{:#?}",
        facts.function(bad_path).direct_effects
    );
    assert!(facts.function_id(module, "badSignature").is_none());
}

#[test]
fn checked_facts_record_saved_reads_inside_saved_path_keys() {
    let root = temp_project("program-fact-key-reads", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             resource Config\n\
             \x20   required bookId: int\n\
             store ^config: Config\n\
             fn readDefault(): string\n\
             \x20   return ^books(^config.bookId).title\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let facts = &program.facts;
    let module = facts.module_id("books").expect("books module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let title = facts
        .resource_member_id(book, &["title"])
        .expect("Book.title");
    let config = facts.resource_id(module, "Config").expect("Config");
    let book_id = facts
        .resource_member_id(config, &["bookId"])
        .expect("Config.bookId");
    let read_default = facts
        .function_id(module, "readDefault")
        .expect("readDefault");

    assert_eq!(
        facts.function(read_default).direct_effects.saved_reads,
        vec![
            SavedPlaceEffect {
                resource: book,
                members: vec![title],
            },
            SavedPlaceEffect {
                resource: config,
                members: vec![book_id],
            },
        ]
    );
}

/// The facts layer's enum-member selectability verdict is the one the schema owns:
/// a `category` member or a member with children is not selectable, every other
/// member is. The fact records the schema's answer rather than re-deriving the
/// rule, so this pins the two to the same verdict for a hierarchical enum.
#[test]
fn enum_member_selectability_matches_schema_owner() {
    let root = temp_project("program-enum-selectable", |root| {
        write(
            root,
            "src/zoo/cats.mw",
            "module zoo::cats\n\
             enum Cat\n\
             \x20   category tiger\n\
             \x20       bengal\n\
             \x20       siberian\n\
             \x20   housecat\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let facts = &program.facts;
    let module = facts.module_id("zoo::cats").expect("module fact");
    let enum_id = facts.enum_id(module, "Cat").expect("enum fact");
    let schema = program.modules[module.0 as usize]
        .enums
        .iter()
        .find(|schema| schema.name == "Cat")
        .expect("enum schema");

    let mut verdicts: Vec<(&str, bool)> = Vec::new();
    for member in facts.enum_members() {
        if member.enum_id != enum_id {
            continue;
        }
        let ordinal = schema.ordinal(&member.name).expect("schema member by name");
        assert_eq!(
            facts.enum_member_is_selectable(member.id),
            schema.is_selectable_leaf(ordinal),
            "selectability of `{}` must match the schema owner",
            member.name
        );
        verdicts.push((
            member.name.as_str(),
            facts.enum_member_is_selectable(member.id),
        ));
    }

    assert_eq!(
        verdicts,
        vec![
            ("tiger", false),
            ("bengal", true),
            ("siberian", true),
            ("housecat", true),
        ]
    );
}
