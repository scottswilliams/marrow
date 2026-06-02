use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{
    CheckedType, HostEffect, MarrowType, SavedPlaceEffect, check_project, check_tests_program,
};
use marrow_project::parse_config;
use marrow_schema::stdlib::Capability;
use marrow_store::value::ScalarType;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

#[test]
fn builds_a_module_for_a_clean_library_file() {
    let root = temp_project("program-clean", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert_eq!(program.modules.len(), 1, "{program:#?}");

    let module = &program.modules[0];
    assert_eq!(module.name, "shelf::books");

    assert_eq!(module.resources.len(), 1, "{:#?}", module.resources);
    assert_eq!(module.resources[0].name, "Book");

    let add = module
        .functions
        .iter()
        .find(|function| function.name == "add")
        .expect("add function");
    assert!(add.public, "{add:#?}");
    assert_eq!(add.params.len(), 1, "{:#?}", add.params);
    assert_eq!(add.params[0].name, "title");
    assert_eq!(add.params[0].ty, MarrowType::Primitive(ScalarType::Str));
    assert!(add.return_type.is_some(), "{add:#?}");
    // `add`'s body touches the `^books` saved root (allocating an id with `nextId`).
    assert!(add.touches_saved_data, "{add:#?}");
    // The body is carried into the artifact for the runtime to evaluate.
    assert!(!add.body.statements.is_empty(), "{add:#?}");
}

#[test]
fn checked_facts_assign_typed_ids_to_same_named_declarations() {
    let root = temp_project("program-fact-ids", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             resource Book at ^books_a(id: int)\n\
             \x20   required title: string\n\
             enum Status\n\
             \x20   active\n\
             pub fn fresh(): Id(^books_a)\n\
             \x20   return nextId(^books_a)\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book at ^books_b(id: int)\n\
             \x20   required title: string\n\
             enum Status\n\
             \x20   active\n\
             pub fn fresh(): Id(^books_b)\n\
             \x20   return nextId(^books_b)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
fn checked_facts_record_function_effects_with_typed_places() {
    let root = temp_project("program-fact-effects", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
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
    fs::remove_dir_all(&root).ok();

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
    assert_eq!(facts.function(read).span.line, 5);
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
fn checked_facts_resolve_qualified_resource_annotations_to_the_owner() {
    let root = temp_project("program-fact-resource-owner", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             resource Book at ^a_books(id: int)\n\
             \x20   required title: string\n\
             fn borrowed(id: Id(^b_books)): Id(^b_books)\n\
             \x20   return id\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book at ^b_books(id: int)\n\
             \x20   required title: string\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
             resource Book at ^a_books(id: int)\n\
             \x20   required title: string\n\
             pub fn borrowed(id: Id(^b_books)): Id(^b_books)\n\
             \x20   return id\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book at ^b_books(id: int)\n\
             \x20   required title: string\n",
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
    fs::remove_dir_all(&root).ok();

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
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   notes(pos: int)\n\
             \x20       text: string\n\
             fn badPath()\n\
             \x20   ^books(1).notes(1).missing\n\
             fn badSignature(id: Missing): int\n\
             \x20   return 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             resource Config at ^config\n\
             \x20   required bookId: int\n\
             fn readDefault(): string\n\
             \x20   return ^books(^config.bookId).title\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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

/// `nextId(^books)` over a single-`int` root types to `Id(^books)`, so a function
/// returning it under a declared `Id(^books)` return type checks clean. (`nextId`
/// is a saved-data read, so it lives in a function body, not a module const.)
/// Previously `nextId` typed to `Unknown`. The local-const annotation
/// `const id: Id(^books) = nextId(^books)` likewise checks clean.
#[test]
fn next_id_types_to_the_resource_identity() {
    let root = temp_project("program-nextid-id", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn fresh(): Id(^books)\n\
             \x20   const id: Id(^books) = nextId(^books)\n\
             \x20   return id\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `nextId` over a composite-identity root is rejected at check time with
/// `check.next_id_requires_single_int`, so the misuse is caught before running.
#[test]
fn next_id_over_a_composite_root_is_flagged() {
    let root = temp_project("program-nextid-composite", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment at ^enrollments(studentId: string, courseId: string)\n\
             \x20   required grade: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^enrollments)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over a single non-integer (string) root is flagged the same way.
#[test]
fn next_id_over_a_string_keyed_root_is_flagged() {
    let root = temp_project("program-nextid-string", |root| {
        write(
            root,
            "src/shelf/tags.mw",
            "module shelf::tags\n\
             resource Tag at ^tags(slug: string)\n\
             \x20   required name: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^tags)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over a keyless singleton root is flagged: a singleton has no
/// generated identity.
#[test]
fn next_id_over_a_singleton_root_is_flagged() {
    let root = temp_project("program-nextid-singleton", |root| {
        write(
            root,
            "src/shelf/settings.mw",
            "module shelf::settings\n\
             resource Settings at ^settings\n\
             \x20   required theme: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^settings)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

// --- Ordered navigation: reversed / next / prev ---

/// `reversed`, `next`, and `prev` are builtins, so they never report
/// `check.unresolved_call`. `reversed` is type-transparent: it yields the same
/// element type as its argument, so `for w in reversed(std::text::split(...))`
/// binds `w` to `string` just like `for w in std::text::split(...)` does — and
/// misusing it (`w + 1`, a string plus an int) is flagged. If `reversed` regressed
/// the element type to `Unknown`, this misuse would pass silently, so the
/// diagnostic proves the element type survives the wrapper.
#[test]
fn reversed_preserves_the_sequence_element_type() {
    let root = temp_project("program-reversed-transparent", |root| {
        write(
            root,
            "src/shelf/words.mw",
            "module shelf::words\n\
             fn shout()\n\
             \x20   for w in reversed(std::text::split(\"a,b,c\", \",\"))\n\
             \x20       var x = w + 1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    // `w` is `string`, so `w + 1` is a string-plus-int operator type error — not an
    // unresolved-call error (which would mean `reversed` was never recognized).
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.starts_with("check.") && d.code != "check.unresolved_call"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == "check.unresolved_call"),
        "reversed must be a recognized builtin: {:#?}",
        report.diagnostics
    );
}

#[test]
fn local_collections_can_be_subscripted() {
    let root = temp_project("program-local-collection-subscript", |root| {
        write(
            root,
            "src/shelf/local.mw",
            "module shelf::local\n\
             fn keyed(today: date): int\n\
             \x20   var counts(day: date, category: string): int\n\
             \x20   counts(today, \"open\") = 3\n\
             \x20   return counts(today, \"open\")\n\
             fn seqIndex(): int\n\
             \x20   var xs: sequence[int]\n\
             \x20   xs(1) = 10\n\
             \x20   return xs(1)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `next(^root(id))` over a keyed root types to the store identity; the absent
/// edge is resolved before the identity feeds the next saved read. `prev`
/// mirrors it.
#[test]
fn next_and_prev_of_a_keyed_root_type_to_the_identity() {
    let root = temp_project("program-next-identity", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn afterTitle(id: int, fallback: Id(^books)): string\n\
             \x20   return ^books(next(^books(id)) ?? fallback).title\n\
             pub fn beforeTitle(id: int, fallback: Id(^books)): string\n\
             \x20   return ^books(prev(^books(id)) ?? fallback).title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `next`/`prev` take exactly one argument; a zero- or two-argument call reports
/// the standard `check.call_argument` arity diagnostic.
#[test]
fn next_with_wrong_arity_is_flagged() {
    let root = temp_project("program-next-arity", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn bad(id: int)\n\
             \x20   const x = next(^books(id), ^books(id))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// `next` over a keyed child-layer position types to the layer's key type, so
/// `next(^books(id).tags(p)) ?? -1` defaults an `int` with an `int` and checks
/// clean — the edge fault's `??` default drives the result type.
#[test]
fn next_of_a_layer_position_coalesces_to_the_key_type() {
    let root = temp_project("program-next-layer-coalesce", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             pub fn nextPos(id: int, p: int): int\n\
             \x20   return next(^books(id).tags(p)) ?? -1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `next`/`prev` over a composite multi-key identity record is statically
/// unsupported (the runtime rejects it with an uncatchable fault), so the checker
/// reports `check.neighbor_unsupported` rather than mis-typing it as an identity.
#[test]
fn next_over_a_composite_identity_record_is_flagged() {
    let root = temp_project("program-next-composite", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment at ^enrollments(studentId: string, courseId: string)\n\
             \x20   required grade: string\n\
             fn step(s: string, c: string)\n\
             \x20   const n = next(^enrollments(s, c))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.neighbor_unsupported"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn next_over_a_bare_composite_identity_root_is_flagged() {
    let root = temp_project("program-next-bare-composite", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment at ^enrollments(studentId: string, courseId: string)\n\
             \x20   required grade: string\n\
             fn step()\n\
             \x20   const n = next(^enrollments)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.neighbor_unsupported"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn next_over_a_bare_identity_value_is_flagged() {
    let root = temp_project("program-next-identity-value", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn step(id: Id(^books))\n\
             \x20   const n = next(id)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.neighbor_unsupported"),
        "{:#?}",
        report.diagnostics
    );
}

/// `next`/`prev` over an index branch is statically unsupported the same way: an
/// index branch inspects identities, with no single key position to seek.
#[test]
fn next_over_an_index_branch_is_flagged() {
    let root = temp_project("program-next-index-branch", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             \x20   index byShelf(shelf, id)\n\
             fn step(s: string)\n\
             \x20   const n = next(^books.byShelf(s))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.neighbor_unsupported"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn keys_over_composite_identity_index_bind_reconstructed_identities() {
    let root = temp_project("program-composite-index-keys", |root| {
        write(
            root,
            "src/school/registrar.mw",
            "module school::registrar\n\
             resource Enrollment at ^enrollments(studentId: string, courseId: string)\n\
             \x20   required credits: int\n\
             \x20   index byStudent(studentId, courseId)\n\
             fn total(studentId: string): int\n\
             \x20   var credits = 0\n\
             \x20   for id in keys(^enrollments.byStudent(studentId))\n\
             \x20       credits = credits + ^enrollments(id).credits\n\
             \x20   return credits\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `use std::clock` lets a short-form `clock::now()` resolve and type to its
/// declared result (`instant`), just as the fully-qualified form does.
#[test]
fn short_form_std_import_resolves() {
    let root = temp_project("program-shortform-clock", |root| {
        write(
            root,
            "src/shelf/times.mw",
            "module shelf::times\n\
             use std::clock\n\
             pub fn stamp(): instant\n\
             \x20   return clock::now()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Without the import, the short-form `clock::now()` does not resolve and reports
/// `check.unresolved_call` — short-form requires the matching `use`.
#[test]
fn short_form_without_import_is_unresolved() {
    let root = temp_project("program-shortform-noimport", |root| {
        write(
            root,
            "src/shelf/times.mw",
            "module shelf::times\n\
             pub fn stamp(): instant\n\
             \x20   return clock::now()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.unresolved_call"),
        "{:#?}",
        report.diagnostics
    );
}

/// Short-form works for project modules too: `use shelf::books` lets `books::add`
/// resolve to the qualified function in that module.
#[test]
fn short_form_project_import_resolves() {
    let root = temp_project("program-shortform-project", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             pub fn make(): int\n\
             \x20   return 1\n",
        );
        write(
            root,
            "src/shelf/app.mw",
            "module shelf::app\n\
             use shelf::books\n\
             pub fn run(): int\n\
             \x20   return books::make()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A std helper's argument types are now checked: passing an `int` where
/// `std::text::contains` expects a `string` reports `check.call_argument`.
#[test]
fn std_call_with_wrong_argument_type_is_flagged() {
    let root = temp_project("program-std-argtype", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn bad(): bool\n\
             \x20   return std::text::contains(1, \"x\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// A std helper's arity is now checked: `std::math::modulo` takes two ints, so a
/// one-argument call reports `check.call_argument`.
#[test]
fn std_call_with_wrong_arity_is_flagged() {
    let root = temp_project("program-std-arity", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn bad(): int\n\
             \x20   return std::math::modulo(1)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// A well-typed std call checks clean: `std::clock::add(instant, duration)` with
/// the right argument types reports nothing.
#[test]
fn well_typed_std_call_checks_clean() {
    let root = temp_project("program-std-clean", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn good(): instant\n\
             \x20   return std::clock::add(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), std::clock::parseDuration(\"PT1H\"))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A duration literal types to `duration`: returned from a `: duration`
/// function and passed where a `duration` argument is expected it checks clean,
/// and returned from a `: int` function it is a return-type error.
#[test]
fn duration_literal_types_to_duration() {
    let root = temp_project("program-duration-literal", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn span(): duration\n\
             \x20   return 1.day\n\
             pub fn shift(): instant\n\
             \x20   return std::clock::add(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), 1.hour)\n\
             pub fn wrong(): int\n\
             \x20   return 1.day\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    // The duration argument to `std::clock::add` must not raise an untyped-value
    // error: a duration literal is a known type, not dynamic data.
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
    let return_type_errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "check.return_type")
        .collect();
    assert_eq!(
        return_type_errors.len(),
        1,
        "only the `: int` return should mismatch: {:#?}",
        report.diagnostics
    );
}

/// Short-form std calls are arg-checked identically to fully-qualified ones:
/// `clock::add(int, ...)` (wrong first arg) under `use std::clock` is flagged.
#[test]
fn short_form_std_call_is_arg_checked() {
    let root = temp_project("program-std-shortform-arg", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             use std::clock\n\
             pub fn bad(): instant\n\
             \x20   return clock::add(1, clock::parseDuration(\"PT1H\"))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// Short-form resolves even when the module name is a type keyword: `use std::bytes`
/// lets `bytes::base64Encode(...)` parse (a keyword can lead a `::` path) and check
/// clean, not just the fully-qualified `std::bytes::base64Encode(...)`.
#[test]
fn short_form_keyword_module_resolves() {
    let root = temp_project("program-shortform-bytes", |root| {
        write(
            root,
            "src/shelf/b.mw",
            "module shelf::b\n\
             use std::bytes\n\
             pub fn enc(): string\n\
             \x20   return bytes::base64Encode(b\"hi\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_file_with_a_parse_error_contributes_no_module() {
    let root = temp_project("program-parse-error", |root| {
        // A leading tab is a lexical error, so the file parses with errors and
        // is excluded from the artifact.
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\tconst X = 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(report.has_errors(), "{:#?}", report.diagnostics);
    assert!(program.modules.is_empty(), "{program:#?}");
}

// --- `Error` in a scalar position (regression for A08) -------------------------
//
// `MarrowType::Error` is a concrete type with no storage form: it is *not* an
// untyped value. A `catch e: Error` clause binds `e` as `Error`, so using `e`
// where a scalar is required must report the same diagnostic a wrong scalar
// would, never `check.untyped_value` and never nothing. (A08 split `Error` into
// its own arm; before that `Error` was a primitive that simply failed to match,
// which is the behavior these tests pin back in place.) The dual is preserved:
// `Error` must still satisfy an `Error`-typed slot (`std::log::error`).

/// Build a one-module project whose single function wraps `body` in a
/// `try`/`catch e: Error`, so `e` is in scope as an `Error` value, and return its
/// diagnostic codes. `signature` is the function header (e.g. `fn f()`). `slot`
/// names the project directory: each caller passes a distinct `slot` so that two
/// of these tests running concurrently under workspace parallelism never share a
/// temp project (and so cannot delete each other's files mid-run).
fn error_value_diagnostic_codes(slot: &str, signature: &str, body: &str) -> Vec<String> {
    let root = temp_project(&format!("program-error-scalar-{slot}"), |root| {
        write(
            root,
            "src/shelf/t.mw",
            &format!(
                "module shelf::t\n\
                 {signature}\n\
                 \x20   try\n\
                 \x20       var x = 1\n\
                 \x20   catch e: Error\n\
                 {body}\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// `if e` over an `Error` condition reports `check.condition_type` (a condition
/// must be `bool`), not `check.untyped_value`.
#[test]
fn error_condition_is_a_condition_type_error() {
    let codes =
        error_value_diagnostic_codes("condition", "fn f()", "        if e\n            x = 1");
    assert!(
        codes.iter().any(|code| code == "check.condition_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `return e` from a `: string` function reports `check.return_type`, not
/// `check.untyped_value`.
#[test]
fn error_return_is_a_return_type_error() {
    let codes = error_value_diagnostic_codes("return", "fn f(): string", "        return e");
    assert!(
        codes.iter().any(|code| code == "check.return_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `s = e` storing an `Error` into a `string` place reports
/// `check.assignment_type`, not `check.untyped_value`.
#[test]
fn error_assignment_is_an_assignment_type_error() {
    let codes = error_value_diagnostic_codes(
        "assignment",
        "fn f()",
        "        var s: string = \"a\"\n        s = e",
    );
    assert!(
        codes.iter().any(|code| code == "check.assignment_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// Passing `e` to a user function declared `f(s: string)` reports
/// `check.call_argument`, not `check.untyped_value`.
#[test]
fn error_argument_to_user_function_is_a_call_argument_error() {
    let root = temp_project("program-error-userfn-arg", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             fn takes(s: string)\n\
             \x20   return\n\
             fn f()\n\
             \x20   try\n\
             \x20       var x = 1\n\
             \x20   catch e: Error\n\
             \x20       takes(e)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
}

/// Build a project declaring `fn takes(e: Error)` and calling it with `arg` from
/// inside a `try`/`catch e: Error` (so the name `e` is an `Error` value in scope),
/// and return the diagnostic codes. An `Error`-typed parameter is a reachable user
/// type (`from_resolved` maps it to `MarrowType::Error`), so the argument loop must
/// check it like a scalar.
fn error_param_call_diagnostic_codes(slot: &str, arg: &str) -> Vec<String> {
    let root = temp_project(&format!("program-error-param-{slot}"), |root| {
        write(
            root,
            "src/shelf/t.mw",
            &format!(
                "module shelf::t\n\
                 fn takes(e: Error)\n\
                 \x20   return\n\
                 fn f()\n\
                 \x20   try\n\
                 \x20       var x = 1\n\
                 \x20   catch e: Error\n\
                 \x20       takes({arg})\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// Passing a `string` literal to a `takes(e: Error)` parameter reports
/// `check.call_argument`: the scalar does not satisfy the concrete `Error` slot.
/// (Before the fix the `as_primitive(&param.ty).is_some()` gate skipped any
/// `Error`-typed parameter, silently accepting the mismatch.)
#[test]
fn scalar_argument_to_error_param_is_a_call_argument_error() {
    let codes = error_param_call_diagnostic_codes("scalar", "\"oops\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// Passing an unbound name (an `Unknown` value) to a `takes(e: Error)` parameter
/// reports `check.untyped_value`: strict typing still requires a known type for a
/// concrete slot, even an `Error` one.
#[test]
fn untyped_argument_to_error_param_is_an_untyped_value_error() {
    let codes = error_param_call_diagnostic_codes("untyped", "mystery");
    assert!(
        codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// Passing a catch-bound `Error` value to a `takes(e: Error)` parameter checks
/// clean: the concrete `Error` slot is satisfied by an `Error` argument.
#[test]
fn error_argument_to_error_param_checks_clean() {
    let codes = error_param_call_diagnostic_codes("clean", "e");
    assert!(codes.is_empty(), "{codes:#?}");
}

/// Passing `e` to `std::log::info` (which expects a `string`) reports
/// `check.call_argument`, not `check.untyped_value`.
#[test]
fn error_argument_to_std_log_info_is_a_call_argument_error() {
    let codes = error_value_diagnostic_codes("log-info", "fn f()", "        std::log::info(e)");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `-e` negating an `Error` reports `check.operator_type` (no operator applies to
/// an `Error`), not `check.untyped_value`.
#[test]
fn error_unary_negation_is_an_operator_type_error() {
    let codes = error_value_diagnostic_codes("unary", "fn f()", "        y = -e");
    assert!(
        codes.iter().any(|code| code == "check.operator_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `e + 1` with an `Error` operand reports `check.operator_type` (no operator
/// applies to an `Error`), not `check.untyped_value` and never nothing.
#[test]
fn error_arithmetic_operand_is_an_operator_type_error() {
    let codes = error_value_diagnostic_codes("arithmetic", "fn f()", "        y = e + 1");
    assert!(
        codes.iter().any(|code| code == "check.operator_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `e < 1` comparing an `Error` operand reports `check.operator_type`, not
/// `check.untyped_value` and never nothing.
#[test]
fn error_comparison_operand_is_an_operator_type_error() {
    let codes = error_value_diagnostic_codes("comparison", "fn f()", "        y = e < 1");
    assert!(
        codes.iter().any(|code| code == "check.operator_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

// --- `Error` in the one slot that *expects* it (dual of the above) -------------

/// `std::log::error(e)` accepts an `Error` value: the `Error`-typed slot is
/// satisfied, so the call checks clean.
#[test]
fn error_argument_to_std_log_error_checks_clean() {
    let codes = error_value_diagnostic_codes("log-error", "fn f()", "        std::log::error(e)");
    assert!(codes.is_empty(), "{codes:#?}");
}

/// A scalar passed to `std::log::error` (which expects an `Error`) reports
/// `check.call_argument` — the scalar does not satisfy the `Error` slot.
#[test]
fn scalar_argument_to_std_log_error_is_a_call_argument_error() {
    let root = temp_project("program-logerror-scalar", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             fn f()\n\
             \x20   std::log::error(\"oops\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// An untyped value passed to `std::log::error` reports `check.untyped_value`:
/// `Unknown` is still untyped (unchanged by the `Error` fix). An unbound name
/// (`mystery`) has no known type.
#[test]
fn untyped_argument_to_std_log_error_is_an_untyped_value_error() {
    let root = temp_project("program-logerror-untyped", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             fn f()\n\
             \x20   std::log::error(mystery)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
}

// --- Nominal identity typing ---

/// Two keyed resources whose identities are byte-identical (`Id(^books)` and
/// `Id(^magazines)` are both single-`int`) but nominally distinct. Used by the
/// nominal-identity tests below.
const TWO_BOOKISH_RESOURCES: &str = "module shelf::lib\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     resource Magazine at ^magazines(id: int)\n\
     \x20   required title: string\n";

/// Passing a `Id(^magazines)` where a function parameter expects `Id(^books)` is a
/// nominal mismatch: the identities share a key shape but name different
/// store roots, so the call is rejected as `check.call_argument`.
#[test]
fn wrong_store_identity_argument_is_flagged() {
    let root = temp_project("program-id-arg", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn takes(b: Id(^books))\n\
                 \x20   return\n\
                 fn f(m: Id(^magazines))\n\
                 \x20   takes(m)\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// Returning a `Id(^magazines)` from a function declared to return `Id(^books)` is a
/// nominal mismatch reported as `check.return_type`.
#[test]
fn wrong_store_identity_return_is_flagged() {
    let root = temp_project("program-id-return", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(m: Id(^magazines)): Id(^books)\n\
                 \x20   return m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.return_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Storing a `Id(^magazines)` into a `Id(^books)` place is a nominal mismatch reported
/// as `check.assignment_type` — closing the value-side asymmetry where a
/// non-primitive place used to be left alone.
#[test]
fn wrong_store_identity_assignment_is_flagged() {
    let root = temp_project("program-id-assign", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(m: Id(^magazines))\n\
                 \x20   var b: Id(^books) = m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.assignment_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A raw scalar where an identity is expected, and an identity where a scalar is
/// expected, are both flagged as `check.call_argument`: identity and scalar are
/// distinct nominal types, not freely interchangeable.
#[test]
fn scalar_and_identity_are_not_interchangeable_arguments() {
    let root = temp_project("program-id-scalar-swap", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn takesId(b: Id(^books))\n\
                 \x20   return\n\
                 fn takesInt(n: int)\n\
                 \x20   return\n\
                 fn f(b: Id(^books))\n\
                 \x20   takesId(1)\n\
                 \x20   takesInt(b)\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    let count = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "check.call_argument")
        .count();
    assert!(count >= 2, "{:#?}", report.diagnostics);
}

/// Same-store identity flow checks clean: passing, returning, and storing a
/// `Id(^books)` where a `Id(^books)` is expected is well-typed and reports nothing.
#[test]
fn same_store_identity_checks_clean() {
    let root = temp_project("program-id-same", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn takes(b: Id(^books))\n\
                 \x20   return\n\
                 fn f(b: Id(^books)): Id(^books)\n\
                 \x20   takes(b)\n\
                 \x20   var c: Id(^books) = b\n\
                 \x20   return c\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn qualified_resource_identity_annotation_unifies_with_owner_identity() {
    let root = temp_project("program-id-qualified", |root| {
        write(
            root,
            "src/inventory.mw",
            "module inventory\n\
             resource Item at ^items(id: int)\n\
             \x20   required name: string\n\
             pub fn add(name: string): Id(^items)\n\
             \x20   const id: Id(^items) = nextId(^items)\n\
             \x20   ^items(id).name = name\n\
             \x20   return id\n\
             pub fn nameOf(id: Id(^items)): string\n\
             \x20   return ^items(id).name\n",
        );
        write(
            root,
            "src/caller.mw",
            "module caller\n\
             use inventory\n\
             pub fn demo(): string\n\
             \x20   const id: Id(^items) = inventory::add(\"widget\")\n\
             \x20   return inventory::nameOf(id)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn aliased_resource_and_identity_annotations_resolve_to_the_owner() {
    let root = temp_project("program-resource-qualified", |root| {
        write(
            root,
            "src/audit/log.mw",
            "module audit::log\n\
             resource Event at ^events(id: int)\n\
             \x20   required actor: string\n",
        );
        write(
            root,
            "src/audit/query.mw",
            "module audit::query\n\
             use audit::log\n\
             pub fn actor(ev: log::Event): string\n\
             \x20   const id: Id(^events) = nextId(^events)\n\
             \x20   ^events(id).actor = \"scott\"\n\
             \x20   return ev.actor\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn legacy_qualified_identity_constructor_is_unresolved() {
    let root = temp_project("program-id-qualified-ctor", |root| {
        write(
            root,
            "src/inventory.mw",
            "module inventory\n\
             resource Book at ^books(id: int)\n\
             \x20   required name: string\n",
        );
        write(
            root,
            "src/caller.mw",
            "module caller\n\
             use inventory\n\
             pub fn fromKey(): Id(^books)\n\
             \x20   return inventory::Book::Id(1)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.unresolved_call"
                && diagnostic.message.contains("inventory::Book::Id")),
        "{:#?}",
        report.diagnostics
    );
}

// --- Equality, coalesce, and unary over concrete non-scalar types ---

/// Two resources and a sequence-yielding helper, for the operator-soundness tests
/// below. `Book` and `Magazine` are distinct nominal resources with the same key
/// shape, so an identity of one is byte-identical to an identity of the other yet
/// must not compare equal.
const OPERATOR_OPERANDS: &str = "module shelf::ops\n\
     resource Book at ^books(id: int)\n\
     \x20   required title: string\n\
     resource Magazine at ^magazines(id: int)\n\
     \x20   required title: string\n";

/// Compare two whole records of different resources with `==`. Equality is not
/// defined over whole records, so this is `check.operator_type`, not a silent
/// fall-through to a `bool` result.
#[test]
fn resource_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-resource", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Book, m: Magazine): bool\n\
                 \x20   return b == m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare a whole record against a scalar with `==`. A record is not a scalar, so
/// the comparison is `check.operator_type`.
#[test]
fn resource_against_scalar_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-resource-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Book, n: int): bool\n\
                 \x20   return b == n\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare two sequences with `==`. Equality is not defined over sequences, so the
/// comparison is `check.operator_type`.
#[test]
fn sequence_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-sequence", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(xs: sequence[int], ys: sequence[int]): bool\n\
                 \x20   return xs == ys\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare identities of different resources with `==`. They share a key shape but
/// name different resources, so equality across them is `check.operator_type`.
#[test]
fn cross_resource_identity_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-id-cross", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books), m: Id(^magazines)): bool\n\
                 \x20   return b == m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare two identities of the *same* store with `==`. Identity equality is
/// usable, so the comparison checks clean and types to `bool` — a function that
/// returns that comparison from a `: bool` body has no diagnostic.
#[test]
fn same_store_identity_equality_checks_clean() {
    let root = temp_project("program-eq-id-same", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(a: Id(^books), b: Id(^books)): bool\n\
                 \x20   return a == b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A raw scalar `==` scalar comparison stays clean — broadening the guard does not
/// disturb the ordinary scalar-equality path.
#[test]
fn raw_scalar_equality_still_checks_clean() {
    let root = temp_project("program-eq-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(a: int, b: int): bool\n\
                 \x20   return a == b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Coalescing two identities of different resources with `??` is a nominal
/// mismatch reported as `check.operator_type`: the unique-index read on the left
/// yields a `Id(^books)`, and a `Id(^magazines)` default cannot stand in for it. The
/// left is a genuine path read (the only operand `??` accepts).
#[test]
fn cross_resource_identity_coalesce_is_flagged() {
    let root = temp_project("program-coalesce-id-cross", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   index byTitle(title) unique\n\
             resource Magazine at ^magazines(id: int)\n\
             \x20   required title: string\n\
             fn f(m: Id(^magazines)): Id(^magazines)\n\
             \x20   return ^books.byTitle(\"a\") ?? m\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A unary operator on an identity-typed value is operator misuse: no unary op
/// applies to an identity, so `-b` over a `Id(^books)` is `check.operator_type`, not
/// a silent `Unknown`.
#[test]
fn unary_on_identity_is_an_operator_type_error() {
    let root = temp_project("program-unary-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): bool\n\
                 \x20   return not b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

// --- Non-equality binary operators over concrete non-scalar operands ---
//
// `==`/`!=` over identities, records, and sequences is decided before the scalar
// gate. The other binary operators (`+`, `<`, `and`, `_`, …) shared that gate but
// dropped a concrete non-scalar operand to `Unknown` with no diagnostic. Each
// non-scalar operand is operator misuse, like the unary and `Error` cases.

/// Adding a scalar to an identity (`b + 1` where `b: Id(^books)`) is operator misuse:
/// arithmetic does not apply to an identity, so it is `check.operator_type`, not a
/// silent `Unknown`.
#[test]
fn arithmetic_with_identity_operand_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-arith", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): int\n\
                 \x20   return b + 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Ordering two identities (`b < c`) is operator misuse: comparison ordering does
/// not apply to identities, so it is `check.operator_type`.
#[test]
fn ordering_two_identities_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-order", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books), c: Id(^books)): bool\n\
                 \x20   return b < c\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A logical operator over an identity operand (`b and true`) is operator misuse:
/// `and` requires `bool`, so an identity operand is `check.operator_type`.
#[test]
fn logical_with_identity_operand_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-and", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): bool\n\
                 \x20   return b and true\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Concatenating a string with an identity (`"a" _ b`) is operator misuse: `_`
/// joins two strings, so an identity operand is `check.operator_type`.
#[test]
fn concat_with_identity_operand_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-concat", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): string\n\
                 \x20   return \"a\" _ b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A scalar-only binary operation (`1 + 2`) stays clean — broadening the non-scalar
/// guard does not disturb the ordinary scalar arithmetic path.
#[test]
fn scalar_arithmetic_still_checks_clean() {
    let root = temp_project("program-bin-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(): int\n\
                 \x20   return 1 + 2\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

// --- `if`/`while` conditions over concrete non-scalar types ---
//
// A condition must be `bool`. A condition whose type is a concrete non-scalar (an
// identity, a whole record, a sequence) cannot be `bool`, so it is flagged like a
// wrong scalar or an `Error` condition, never swallowed.

/// `if b` over an identity condition is `check.condition_type` — an identity is not
/// `bool`.
#[test]
fn if_identity_condition_is_a_condition_type_error() {
    let root = temp_project("program-if-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books))\n\
                 \x20   if b\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// `while b` over an identity condition is `check.condition_type` — the `while`
/// condition is checked the same way as `if`.
#[test]
fn while_identity_condition_is_a_condition_type_error() {
    let root = temp_project("program-while-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books))\n\
                 \x20   while b\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// `if b` over a whole-record condition is `check.condition_type` — a record is not
/// `bool`.
#[test]
fn if_whole_record_condition_is_a_condition_type_error() {
    let root = temp_project("program-if-record", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Book)\n\
                 \x20   if b\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// `if xs` over a sequence condition is `check.condition_type` — a sequence is not
/// `bool`.
#[test]
fn if_sequence_condition_is_a_condition_type_error() {
    let root = temp_project("program-if-seq", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(xs: sequence[int])\n\
                 \x20   if xs\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A `bool` condition (`if s == "x"`) stays clean — broadening the condition guard
/// does not disturb a genuine `bool` condition.
#[test]
fn bool_condition_still_checks_clean() {
    let root = temp_project("program-if-bool", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(s: string)\n\
                 \x20   if s == \"x\"\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

// --- `??` over a mixed scalar / non-scalar pair ---
//
// `??` defaults a path read with a value of the leaf's type. A pair where one side
// is a concrete non-scalar and the other is a scalar is a category error, not a
// silently-accepted default: the scalar fallback would drop the non-scalar to
// `Unknown` and pass it through. `type_compatible` drives the verdict.

/// A string-leaf read defaulted with an identity (`book.title ?? id`) is a category
/// error reported as `check.operator_type`: a `Id(^books)` cannot default a `string`
/// leaf.
#[test]
fn string_leaf_coalesced_with_identity_is_flagged() {
    let root = temp_project("program-coalesce-str-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn f(id: Id(^books)): string\n\
             \x20   return ^books(1).title ?? id\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A whole-record read defaulted with a scalar (`^books(1) ?? 1`) is a category
/// error reported as `check.operator_type`: a scalar cannot default a whole record.
#[test]
fn whole_record_coalesced_with_scalar_is_flagged() {
    let root = temp_project("program-coalesce-record-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn f(): Book\n\
             \x20   return ^books(1) ?? 1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A scalar leaf defaulted with a matching scalar (`book.title ?? "x"`) stays clean
/// — broadening the non-scalar branch does not disturb the ordinary scalar `??`.
#[test]
fn scalar_coalesce_still_checks_clean() {
    let root = temp_project("program-coalesce-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn f(): string\n\
             \x20   return ^books(1).title ?? \"x\"\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

// --- Key/identity argument typing ---

/// A string passed into an `int` keyspace — `^books("oops")` where `books` is
/// keyed by `id: int` — is rejected as `check.key_type`.
#[test]
fn string_key_into_int_keyspace_is_flagged() {
    let root = temp_project("program-key-string", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn f(): string\n\
             \x20   return ^books(\"oops\").title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A cross-resource read end-to-end: addressing `^books` with a `Id(^magazines)`
/// splices a foreign identity into the book keyspace. The identity is single-`int`
/// like a book's, so the raw key shape matches, but the nominal resource does not,
/// and it is rejected as `check.key_type`.
#[test]
fn cross_resource_key_identity_is_flagged() {
    let root = temp_project("program-key-cross-resource", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(m: Id(^magazines)): string\n\
                 \x20   return ^books(m).title\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Addressing `^books` with its own `Id(^books)` is well-typed — the splice check
/// accepts the matching nominal identity — and reports nothing.
#[test]
fn same_store_key_identity_checks_clean() {
    let root = temp_project("program-key-same-store", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(b: Id(^books)): string\n\
                 \x20   return ^books(b).title\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A cross-module identity the resolving module cannot place defers rather than
/// false-positives: an `unknown`-typed value addressing a keyed root is left to
/// the runtime. Cross-module identities type to `Unknown`, so nominal comparison is
/// permissive across module boundaries until the type IR is unified.
#[test]
fn cross_module_unknown_key_defers() {
    let root = temp_project("program-key-cross-module", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn f(k: unknown): string\n\
             \x20   return ^books(k).title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A cross-module *qualified* identity spliced into a keyed root defers rather
/// than false-positives. The root's resource name is bare (`Book`), while an
/// identity imported from another module keeps its `shelf::lib::Book`
/// qualification, so the two cannot be matched nominally without the unified type
/// IR. Splicing the imported identity into its own keyspace is valid and must be
/// left to the runtime key guard, not rejected here.
#[test]
fn cross_module_qualified_identity_splice_defers() {
    let root = temp_project("program-key-cross-module-splice", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n",
        );
        write(
            root,
            "src/app/main.mw",
            "module app::main\n\
             use shelf::lib\n\
             fn read(b: Id(^books)): string\n\
             \x20   return ^books(b).title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}
