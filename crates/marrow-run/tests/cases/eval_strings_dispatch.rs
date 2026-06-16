//! String literals, concatenation, ordering, escapes and interpolation, entry-call
//! dispatch, recursion, and index-rebuild dispatch.

use crate::support;
use support::*;

use marrow_run::{RUN_TYPE, RUN_UNKNOWN_FUNCTION, Value};
use marrow_store::tree::TreeStore;

#[test]
fn returns_a_string_literal() {
    assert_eq!(
        eval_source(
            "pub fn f(): string\n    return \"hello\"\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Str("hello".into())))
    );
}

#[test]
fn concatenates_strings() {
    assert_eq!(
        eval_source(
            "pub fn greet(name: string): string\n    return \"Hello, \" + name\n",
            "greet",
            vec![Value::Str("World".into())]
        ),
        Ok(Some(Value::Str("Hello, World".into())))
    );
}

#[test]
fn compares_strings_for_equality_and_order() {
    assert_eq!(
        eval_source(
            "pub fn eq(a: string, b: string): bool\n    return a == b\n",
            "eq",
            vec![Value::Str("x".into()), Value::Str("x".into())]
        ),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        eval_source(
            "pub fn lt(a: string, b: string): bool\n    return a < b\n",
            "lt",
            vec![Value::Str("apple".into()), Value::Str("banana".into())]
        ),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn string_escapes_are_decoded() {
    assert_eq!(
        eval_source(
            "pub fn f(): string\n    return \"slash \\\\ quote \\\" line\\n carriage\\r tab\\t\"\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Str(
            "slash \\ quote \" line\n carriage\r tab\t".into()
        )))
    );
}

#[test]
fn unknown_string_escapes_are_rejected_at_check() {
    checker_rejects(
        "pub fn f(): string\n    return \"\\q\"\n",
        "check.string_escape",
    );
}

#[test]
fn concatenation_requires_strings() {
    checker_rejects(
        "pub fn f(): string\n    return \"x\" + 5\n",
        "check.operator_type",
    );
}

#[test]
fn evaluates_string_interpolation() {
    assert_eq!(
        eval_source(
            "pub fn f(n: int): string\n    return $\"n is {n}\"\n",
            "f",
            vec![Value::Int(5)]
        ),
        Ok(Some(Value::Str("n is 5".into())))
    );
}

#[test]
fn interpolation_renders_several_values() {
    assert_eq!(
        eval_source(
            "pub fn f(name: string, ok: bool): string\n    return $\"{name}={ok}\"\n",
            "f",
            vec![Value::Str("ready".into()), Value::Bool(true)]
        ),
        Ok(Some(Value::Str("ready=true".into())))
    );
}

#[test]
fn interpolation_unescapes_literal_braces() {
    assert_eq!(
        eval_source(
            "pub fn f(): string\n    return $\"a {{ b\"\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Str("a { b".into())))
    );
}

#[test]
fn interpolation_text_decodes_string_escapes() {
    assert_eq!(
        eval_source(
            "pub fn f(name: string): string\n    return $\"slash \\\\ quote \\\" {{\\n{name}\\r\\t}}\"\n",
            "f",
            vec![Value::Str("Ada".into())]
        ),
        Ok(Some(Value::Str("slash \\ quote \" {\nAda\r\t}".into())))
    );
}

#[test]
fn unknown_interpolation_escapes_are_rejected_at_check() {
    checker_rejects(
        "pub fn f(): string\n    return $\"\\q\"\n",
        "check.string_escape",
    );
}

#[test]
fn an_interpolation_bad_escape_beside_a_hole_is_rejected_at_check() {
    checker_rejects(
        "pub fn boom(): decimal\n    return 1.0 / 0.0\n\n\
         pub fn f(): string\n    return $\"{boom()}\\q\"\n",
        "check.string_escape",
    );
}

#[test]
fn run_entry_evaluates_a_function_by_qualified_name() {
    let program = checked_program("pub fn add(a: int, b: int): int\n    return a + b\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::add",
            Value::Int(2),
            Value::Int(3)
        )),
        Ok(Some(Value::Int(5)))
    );
}

#[test]
fn run_entry_rejects_host_values_that_do_not_match_checked_parameters() {
    let program = checked_program("pub fn needs_int(n: int)\n    print(\"ran\")\n");
    let error = rejected_entry_call(&program, "test::needs_int", vec![Value::Str("x".into())]);

    assert_eq!(error.code(), RUN_TYPE);
    let (_, message) = error_throw_fields(&error);
    assert_eq!(message, "entry argument `n` has the wrong type");
}

#[test]
fn run_entry_rejects_private_functions_as_entries() {
    let program = checked_program_modules(&["module a\n\nfn secret(): int\n    return 1\n"]);
    let error = rejected_entry_call(&program, "a::secret", vec![]);

    assert_eq!(error.code(), "run.private_function");
}

#[test]
fn run_entry_rejects_ambiguous_bare_entries() {
    let program = checked_program_modules(&[
        "module a\n\npub fn widget(): int\n    return 1\n",
        "module b\n\npub fn widget(): int\n    return 2\n",
    ]);
    let error = rejected_entry_call(&program, "widget", vec![]);

    assert_eq!(error.code(), "run.ambiguous_function");
}

#[test]
fn run_entry_rejects_host_values_for_identity_parameters() {
    let program = checked_program(
        "resource Book\n    required title: string\nstore ^books(id: int): Book\n\n\
         pub fn load(id: Id(^books))\n    print(\"ran\")\n",
    );
    let error = rejected_entry_call(&program, "test::load", vec![Value::Int(1)]);

    assert_eq!(error.code(), RUN_TYPE);
    let (_, message) = error_throw_fields(&error);
    assert_eq!(message, "entry argument `id` has the wrong type");
}

#[test]
fn run_entry_rejects_host_values_for_resource_parameters() {
    let program = checked_program(
        "resource Book\n    required title: string\nstore ^books(id: int): Book\n\n\
         pub fn show(book: Book)\n    print(\"ran\")\n",
    );
    let error = rejected_entry_call(&program, "test::show", vec![Value::Resource(vec![])]);

    assert_eq!(error.code(), RUN_TYPE);
    let (_, message) = error_throw_fields(&error);
    assert_eq!(message, "entry argument `book` has the wrong type");
}

#[test]
fn a_function_can_call_another() {
    let program = checked_program(
        "pub fn double(n: int): int\n    return n + n\n\npub fn quad(n: int): int\n    return double(n) + double(n)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::quad", Value::Int(3))),
        Ok(Some(Value::Int(12)))
    );
}

#[test]
fn functions_recurse() {
    let program = checked_program(
        "pub fn fact(n: int): int\n    if n <= 1\n        return 1\n    return n * fact(n - 1)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::fact", Value::Int(5))),
        Ok(Some(Value::Int(120)))
    );
}

#[test]
fn a_void_call_runs_as_a_statement() {
    let program = checked_program(
        "pub fn note(n: int)\n    const doubled = n + n\n\npub fn caller(): int\n    note(3)\n    return 2\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::caller")),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn using_a_void_call_as_a_value_is_rejected() {
    checker_rejects(
        "pub fn note(n: int)\n    const doubled = n + n\n\npub fn caller(): int\n    return note(3)\n",
        "check.untyped_value",
    );
}

#[test]
fn an_unknown_function_is_rejected() {
    let program = checked_program("pub fn f(): int\n    return 1\n");
    let error = rejected_entry_call(&program, "test::missing", Vec::new());
    assert_eq!(error.code(), RUN_UNKNOWN_FUNCTION);
}

#[test]
fn values_and_entries_over_an_index_branch_are_unsupported() {
    let resource = BOOK_SHELF_INDEX_SCHEMA;
    for builtin in ["values", "entries"] {
        checker_rejects(
            &format!("{resource}fn f()\n    {builtin}(^books.byShelf(\"x\"))\n"),
            "check.collection_unsupported",
        );
    }
}

#[test]
fn a_unique_index_lookup_loop_skips_an_absent_entry() {
    let program = checked_program(
        &[
            BOOK_ISBN_SCHEMA,
            "pub fn f()\n    for id in ^books.byIsbn(\"978-0\")\n        print($\"{id}\")\n",
        ]
        .concat(),
    );

    let outcome = run_full(checked_entry!(&program, "test::f")).expect("run");
    assert_eq!(outcome.output, "");
}

/// A rebuilt index over seeded records resolves exactly like the maintained one: a
/// store whose index subtrees are wiped resolves nothing, and `rebuild_store_indexes`
/// reconstructs both a unique and a non-unique index from data alone so the same
/// lookups resolve again. The runtime managed-write path seeds the data and the
/// reference index, so this proves the rebuild matches what the runtime maintains.
#[test]
fn rebuild_store_indexes_reconstructs_unique_and_non_unique_lookups() {
    let source = "resource Book\n    \
        required title: string\n    \
        shelf: string\n    \
        isbn: string\nstore ^books(id: int): Book\n\n    \
        index byShelf(shelf, id)\n    \
        index byIsbn(isbn) unique\n\n\
        pub fn add(id: int, t: string, s: string, i: string)\n    \
        ^books(id).title = t\n    \
        ^books(id).shelf = s\n    \
        ^books(id).isbn = i\n\n\
        pub fn isbn_title(i: string): string\n    \
        var found = \"\"\n    \
        for id in ^books.byIsbn(i)\n        \
        found = ^books(id).title ?? \"\"\n    \
        return found\n\n\
        pub fn shelf_count(s: string): int\n    \
        var c = 0\n    \
        for id in keys(^books.byShelf(s))\n        \
        c = c + 1\n    \
        return c\n";
    let (program, runtime) = committed_program_and_runtime(source);
    let store = TreeStore::memory();

    for (id, shelf, isbn) in [
        (1, "fiction", "978-1"),
        (2, "fiction", "978-2"),
        (3, "history", "978-3"),
    ] {
        run_entry(
            &store,
            checked_entry!(
                &runtime,
                "test::add",
                Value::Int(id),
                Value::Str(format!("title-{id}")),
                Value::Str(shelf.into()),
                Value::Str(isbn.into()),
            ),
        )
        .expect("seed book");
    }

    let isbn_lookup = |isbn: &str| {
        run_entry(
            &store,
            checked_entry!(&runtime, "test::isbn_title", Value::Str(isbn.into())),
        )
        .expect("isbn lookup")
        .value
    };
    let shelf_count = |shelf: &str| {
        run_entry(
            &store,
            checked_entry!(&runtime, "test::shelf_count", Value::Str(shelf.into())),
        )
        .expect("shelf count")
        .value
    };

    // The maintained indexes resolve the seeded records.
    assert_eq!(isbn_lookup("978-2"), Some(Value::Str("title-2".into())));
    assert_eq!(shelf_count("fiction"), Some(Value::Int(2)));

    // Wipe every index cell, leaving only the data: the lookups can no longer resolve.
    let place = marrow_check::checked_saved_root_place(
        &program,
        "books",
        marrow_syntax::SourceSpan::default(),
    )
    .expect("checked saved place for ^books");
    for index in &place.indexes {
        let index_id = catalog_id(&index.catalog_id);
        store
            .delete_index_subtree(&index_id, &[])
            .expect("clear index subtree");
    }
    assert_eq!(
        isbn_lookup("978-2"),
        Some(Value::Str(String::new())),
        "no unique entry resolves after the index is wiped"
    );
    assert_eq!(shelf_count("fiction"), Some(Value::Int(0)), "no entries");

    // Rebuild every index from the data, inside the caller's transaction.
    store.begin().expect("begin rebuild");
    marrow_run::evolution::rebuild_store_indexes(&program, &store).expect("rebuild indexes");
    store.commit().expect("commit rebuild");

    // The rebuilt indexes resolve exactly like the maintained ones did.
    assert_eq!(isbn_lookup("978-1"), Some(Value::Str("title-1".into())));
    assert_eq!(isbn_lookup("978-2"), Some(Value::Str("title-2".into())));
    assert_eq!(isbn_lookup("978-3"), Some(Value::Str("title-3".into())));
    assert_eq!(shelf_count("fiction"), Some(Value::Int(2)));
    assert_eq!(shelf_count("history"), Some(Value::Int(1)));
}
