use crate::support;
use support::*;

use marrow_check::{
    CHECK_COLLECTION_UNSUPPORTED, CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT,
    CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP, CHECK_READ_ONLY_EXPRESSION_WRITE, check_project,
    check_project_with_catalog,
};
use marrow_run::{RUN_UNSUPPORTED, Value};

const BOOKS_SOURCE: &str = "\
resource Book
    required title: string
    required shelf: string
    versions(version: int)
        required note: string

store ^books(id: int): Book
    index byShelf(shelf, id)

const PREFIX: string = \"Title: \"

pub fn seed()
    ^books(1) = Book(title: \"Dune\", shelf: \"sf\")

fn write_title(): int
    ^books(2).title = \"Foundation\"
    ^books(2).shelf = \"sf\"
    return 1

fn nested_write(): int
    return write_title()

fn allocate_book(): Id(^books)
    return nextId(^books)

fn announce(): int
    print(\"hello\")
    return 1

fn transactional_value(): int
    transaction
        return 1

fn nested_transactional_value(): int
    return transactional_value()

fn deeply_nested_transactional_value(): int
    return nested_transactional_value()

fn root_count(): int
    return count(^books)

fn root_exists(): bool
    return exists(^books)

fn nested_root_count(): int
    return root_count()

fn iter_root_count(): int
    var n = 0
    for book in ^books
        n = n + 1
    return n

fn nested_iter_root_count(): int
    return iter_root_count()

fn iter_book_versions(id: int): int
    var n = 0
    for version, book in ^books(id).versions
        n = n + 1
    return n

fn iter_index_count(shelf: string): int
    var n = 0
    for id, book in ^books.byShelf(shelf)
        n = n + 1
    return n
";

#[test]
fn evaluates_a_checked_read_only_expression_against_the_store() {
    let (checked, runtime) = committed_program_and_runtime(BOOKS_SOURCE);
    let store = empty_store();
    run_entry(&store, checked_entry!(&runtime, "test::seed")).expect("seed saved data");

    let expr = checked
        .checked_read_only_expression("test", "PREFIX + (^books(1).title ?? \"\")")
        .expect("expression is admitted");
    let mut output = String::new();
    let outcome =
        marrow_run::evaluate_checked_read_only_expression(&store, &runtime, &expr, &mut output)
            .expect("evaluate expression");

    assert_eq!(outcome.value, Some(Value::Str("Title: Dune".to_string())));
    assert_eq!(output, "");
}

#[test]
fn checked_read_only_expression_rejects_saved_writes() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression(
            "test",
            "append(^books, Book(title: \"Dune\", shelf: \"sf\"))",
        )
        .expect_err("append writes saved data");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_entries_values() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "entries(^books)")
        .expect_err("entries is loop-head only");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_COLLECTION_UNSUPPORTED),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_host_effects() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "print(\"hello\")")
        .expect_err("print is a host effect");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_saved_writes() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "write_title()")
        .expect_err("a callee writes saved data");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_nested_transitive_saved_writes() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "nested_write()")
        .expect_err("a nested callee writes saved data");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_saved_allocations() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "allocate_book()")
        .expect_err("a callee allocates saved identity");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_host_effects() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "announce()")
        .expect_err("a callee writes host output");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transaction_blocks() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "transactional_value()")
        .expect_err("callee opens a transaction");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_nested_transaction_blocks() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "nested_transactional_value()")
        .expect_err("nested callee opens a transaction");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_deeply_nested_transaction_blocks() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "deeply_nested_transactional_value()")
        .expect_err("deeply nested callee opens a transaction");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_unindexed_collection_lookups() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "count(^books)")
        .expect_err("root count is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_unindexed_root_presence_checks() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "exists(^books)")
        .expect_err("root existence is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_unindexed_collection_lookups() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "root_count()")
        .expect_err("callee root count is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_unindexed_presence_checks() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "root_exists()")
        .expect_err("callee root existence is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_nested_transitive_unindexed_collection_lookups() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "nested_root_count()")
        .expect_err("nested callee root count is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_unindexed_root_iteration() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "iter_root_count()")
        .expect_err("callee root iteration is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_nested_transitive_unindexed_root_iteration() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "nested_iter_root_count()")
        .expect_err("nested callee root iteration is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_rejects_transitive_unindexed_child_layer_iteration() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    let diagnostics = checked
        .checked_read_only_expression("test", "iter_book_versions(1)")
        .expect_err("callee child-layer iteration is an unindexed lookup");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_read_only_expression_allows_declared_index_iteration() {
    let (checked, _runtime) = committed_program_and_runtime(BOOKS_SOURCE);

    checked
        .checked_read_only_expression("test", "iter_index_count(\"sf\")")
        .expect("declared index traversal is bounded by the index shape");
}

#[test]
fn checked_read_only_expression_is_bound_to_the_program_catalog_context() {
    let (path, text) = checked_source_file(BOOKS_SOURCE, &[]);
    let root = TempDir::new("marrow-run-read-only-context").expect("create project");
    write_temp_source(root.path(), &path, &text);
    let config = test_project_config();
    let (_, first_a) = check_project(root.path(), &config).expect("check first program");
    let (_, first_b) = check_project(root.path(), &config).expect("check second program");
    let store_a = empty_store();
    let store_b = empty_store();
    marrow_run::evolution::commit_catalog_baseline(&store_a, &first_a)
        .expect("commit first catalog");
    marrow_run::evolution::commit_catalog_baseline(&store_b, &first_b)
        .expect("commit second catalog");
    let accepted_a = store_a
        .read_catalog_snapshot()
        .expect("read first catalog snapshot");
    let accepted_b = store_b
        .read_catalog_snapshot()
        .expect("read second catalog snapshot");
    let (_, checked_a) =
        check_project_with_catalog(root.path(), &config, accepted_a.as_ref()).expect("check A");
    let (_, checked_b) =
        check_project_with_catalog(root.path(), &config, accepted_b.as_ref()).expect("check B");
    let runtime_b = checked_b.runtime();
    assert_eq!(checked_a.source_digest(), checked_b.source_digest());
    assert_ne!(
        checked_a.catalog.accepted_digest, checked_b.catalog.accepted_digest,
        "same source in independent first-run commits should carry different durable ids"
    );
    let expr = checked_a
        .checked_read_only_expression("test", "^books(1).title ?? \"\"")
        .expect("expression is admitted");

    let error = marrow_run::evaluate_checked_read_only_expression(
        &store_b,
        &runtime_b,
        &expr,
        &mut String::new(),
    )
    .expect_err("expression from another catalog context is rejected");

    assert_eq!(
        error.code(),
        RUN_UNSUPPORTED,
        "wrong program context should fail before evaluating: {error:?}"
    );
}
