use crate::support;
use marrow_check::{CHECK_UNRESOLVED_OPTIONAL, check_project};

use support::{config, temp_project, write};

/// Check a single `src/books.mw` module `src` and assert it raises no
/// unresolved-optional diagnostic: a maybe-present read proven present by flow
/// narrowing reads as bare `T` through the production pipeline.
fn assert_no_unresolved_optional(name: &str, src: &str) {
    let root = temp_project(name, |root| write(root, "src/books.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

/// Check a single `src/books.mw` module `src` and assert it checks with no errors.
fn assert_no_errors(name: &str, src: &str) {
    let root = temp_project(name, |root| write(root, "src/books.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Check a single `src/books.mw` module `src` and assert it raises the bare
/// maybe-present-read diagnostic: the load-bearing input is the mutation in `src` that
/// expires `if exists` narrowing, so a later read is no longer proven present.
fn assert_bare_present_read(name: &str, src: &str) {
    let root = temp_project(name, |root| write(root, "src/books.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrows_reads_inside_the_then_block() {
    let root = temp_project("presence-if-exists", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_is_key_sensitive() {
    assert_bare_present_read(
        "presence-if-exists-keyed",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(a: int, b: int): string\n\
             \x20   if exists(^books(a).subtitle)\n\
             \x20       return ^books(b).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn composite_identity_raw_write_invalidates_spliced_identity_proof() {
    assert_bare_present_read(
        "presence-composite-identity-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             fn stale(author: string, ordinal: int): string\n\
             \x20   const id: Id(^books) = Id(^books, author, ordinal)\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       delete ^books(author, ordinal).subtitle\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn function_returned_identity_raw_write_invalidates_spliced_identity_proof() {
    assert_bare_present_read(
        "presence-function-identity-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             fn makeId(author: string, ordinal: int): Id(^books)\n\
             \x20   return Id(^books, author, ordinal)\n\
             fn stale(author: string, ordinal: int): string\n\
             \x20   const id: Id(^books) = makeId(author, ordinal)\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       delete ^books(author, ordinal).subtitle\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn var_identity_raw_write_invalidates_spliced_identity_proof() {
    assert_bare_present_read(
        "presence-var-identity-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             fn makeId(author: string, ordinal: int): Id(^books)\n\
             \x20   return Id(^books, author, ordinal)\n\
             fn stale(author: string, ordinal: int): string\n\
             \x20   var id: Id(^books) = makeId(author, ordinal)\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       delete ^books(author, ordinal).subtitle\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn if_const_identity_raw_write_invalidates_spliced_identity_proof() {
    assert_bare_present_read(
        "presence-if-const-identity-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             \x20   isbn: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             fn stale(author: string, ordinal: int, isbn: string): string\n\
             \x20   if const id = ^books.byIsbn(isbn)\n\
             \x20       if exists(^books(id).subtitle)\n\
             \x20           delete ^books(author, ordinal).subtitle\n\
             \x20           return ^books(id).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn distinct_primitive_bindings_may_alias_saved_write_target() {
    assert_bare_present_read(
        "presence-primitive-binding-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn stale(id1: int, id2: int): string\n\
             \x20   if exists(^books(id1).subtitle)\n\
             \x20       delete ^books(id2).subtitle\n\
             \x20       return ^books(id1).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn different_literal_spellings_may_alias_saved_write_target() {
    assert_bare_present_read(
        "presence-literal-spelling-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn stale(): string\n\
             \x20   if exists(^books(1).subtitle)\n\
             \x20       delete ^books(01).subtitle\n\
             \x20       return ^books(1).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn distinct_identity_bindings_may_alias_saved_write_target() {
    assert_bare_present_read(
        "presence-identity-binding-alias-invalidation",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn stale(id1: Id(^books), id2: Id(^books)): string\n\
             \x20   if exists(^books(id1).subtitle)\n\
             \x20       delete ^books(id2).subtitle\n\
             \x20       return ^books(id1).subtitle\n\
             \x20   return \"missing\"\n",
    );
}

/// A guard whose saved-path key calls a function that may write saved data is
/// rejected at the guard boundary: an effectful key may not ride into `exists` or
/// `if const`, because resolving the read at the read site would run the
/// allocation/write on every evaluation. `keyOf` conditionally deletes a record,
/// so the call is opaque before per-function closures exist and disqualifies the
/// read as a guardable target — the boundary refuses the smuggled write rather
/// than admitting a check-clean guard whose repeated read would fault at runtime.
fn assert_code(name: &str, src: &str, code: &str) {
    let root = temp_project(name, |root| write(root, "src/books.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report.diagnostics.iter().any(|d| d.code == code),
        "expected {code}: {:#?}",
        report.diagnostics
    );
}

const SAVED_WRITER_KEY_PRELUDE: &str = "module books\n\
     resource Book\n\
     \x20   subtitle: string\n\
     resource Flag\n\
     \x20   seen: bool\n\
     store ^books(id: int): Book\n\
     store ^flags(id: int): Flag\n\
     fn keyOf(id: int): int\n\
     \x20   if exists(^flags(0).seen)\n\
     \x20       delete ^books(id).subtitle\n\
     \x20   else\n\
     \x20       ^flags(0).seen = true\n\
     \x20   return id\n";

#[test]
fn exists_rejects_a_saved_read_keyed_by_a_saved_writing_function() {
    assert_code(
        "presence-exists-key-expression-saved-write",
        &format!(
            "{SAVED_WRITER_KEY_PRELUDE}fn stale(): string\n\
             \x20   if exists(^books(keyOf(1)).subtitle)\n\
             \x20       return ^books(keyOf(1)).subtitle\n\
             \x20   return \"missing\"\n"
        ),
        "check.call_argument",
    );
}

#[test]
fn if_const_rejects_a_saved_read_keyed_by_a_saved_writing_function() {
    assert_code(
        "presence-if-const-key-expression-saved-write",
        &format!(
            "{SAVED_WRITER_KEY_PRELUDE}fn stale(): string\n\
             \x20   if const value = ^books(keyOf(1)).subtitle\n\
             \x20       return value\n\
             \x20   return \"missing\"\n"
        ),
        "check.condition_type",
    );
}

#[test]
fn a_write_through_a_called_function_expires_an_exists_narrowing() {
    // A function whose body deletes a saved path expires an `if exists` narrowing
    // over that path when it is called inside the guarded block, so the repeated
    // read after the call is no longer proven present. The call is a plain
    // statement, not a guard key, so it reaches the write-invalidation rule rather
    // than the guard-key effect screen.
    assert_bare_present_read(
        "presence-called-write-expires-narrowing",
        "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             fn wipe(id: int)\n\
             \x20   delete ^books(id).title\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).title)\n\
             \x20       wipe(id)\n\
             \x20       return ^books(id).title\n\
             \x20   return \"missing\"\n",
    );
}

#[test]
fn if_exists_narrowing_is_binding_sensitive() {
    assert_bare_present_read(
        "presence-if-exists-shadowed-key",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       const id: int = 2\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_binding_is_assigned() {
    assert_bare_present_read(
        "presence-if-exists-mutated-key",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       k = 2\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_field_is_assigned() {
    assert_bare_present_read(
        "presence-if-exists-mutated-key-field",
        "module books\n\
             resource Holder\n\
             \x20   required id: int\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       holder.id = 2\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_field_is_deleted() {
    assert_bare_present_read(
        "presence-if-exists-delete-field",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       delete ^books(id).subtitle\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_field_is_assigned() {
    assert_bare_present_read(
        "presence-if-exists-write-field",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn stale(): string\n\
             \x20   if exists(^books(1).subtitle)\n\
             \x20       ^books(1).subtitle = \"new\"\n\
             \x20       return ^books(1).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_root_is_replaced() {
    assert_bare_present_read(
        "presence-if-exists-replace-root",
        "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       ^books(id) = Book(title: \"new\")\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_called_function_writes_saved_data() {
    assert_bare_present_read(
        "presence-if-exists-call-writes-saved",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn dropSubtitle(id: int)\n\
             \x20   delete ^books(id).subtitle\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       dropSubtitle(id)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_called_function_transitively_writes_saved_data() {
    assert_bare_present_read(
        "presence-if-exists-call-transitive-writes-saved",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn dropSubtitle(id: int)\n\
             \x20   delete ^books(id).subtitle\n\
             fn relay(id: int)\n\
             \x20   dropSubtitle(id)\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       relay(id)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_only_child_of_parent_is_deleted() {
    let root = temp_project("presence-if-exists-delete-only-child", |root| {
        write(
            root,
            "src/items.mw",
            "module items\n\
             resource Item\n\
             \x20   note: string\n\
             store ^items(id: int): Item\n\
             fn stale(id: int): Item\n\
             \x20   if exists(^items(id))\n\
             \x20       delete ^items(id).note\n\
             \x20       return ^items(id)\n\
             \x20   return Item()\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn unique_index_coalesce_defaults_a_maybe_present_lookup() {
    let root = temp_project("presence-index-coalesce", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \n\
             \x20   index byIsbn(isbn) unique\n\
             \n\
             fn lookup(isbn: string, fallback: Id(^books)): Id(^books)\n\
             \x20   return ^books.byIsbn(isbn) ?? fallback\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn index_range_exists_types_cleanly() {
    assert_no_errors(
        "presence-index-range-exists",
        "module books\n\
         resource Post\n\
         \x20   published: int\n\
         store ^posts(id: int): Post\n\
         \x20   index byDate(published, id)\n\
         fn found(lo: int, hi: int): bool\n\
         \x20   return exists(^posts.byDate(lo..hi))\n",
    );
}

#[test]
fn next_coalesce_defaults_a_maybe_present_neighbor() {
    assert_no_errors(
        "presence-next-coalesce",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   tags(pos: int): string\n\
         store ^books(id: int): Book\n\
         fn nextPos(id: int, pos: int): int\n\
         \x20   return next(^books(id).tags(pos)) ?? -1\n",
    );
}

#[test]
fn for_loop_over_saved_layer_does_not_narrow_the_positional_leaf_read() {
    // Iterating positions proves each position present, but a positional read is
    // uniformly `T?` under the one rule, so a bare read of the leaf at the loop key is
    // rejected — consistent with the local sequence equivalent and the documented idiom.
    assert_bare_present_read(
        "presence-loop-positional-leaf",
        "module books\n\
         resource Book\n\
         \x20   tags(pos: int): string\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   for pos in ^books(1).tags\n\
         \x20   \x20   print(^books(1).tags(pos))\n",
    );
}

#[test]
fn for_loop_over_saved_layer_positional_leaf_reads_with_if_const() {
    // The documented idiom: guard the positional read with `if const` to bind its
    // present arm.
    assert_no_errors(
        "presence-loop-positional-leaf-if-const",
        "module books\n\
         resource Book\n\
         \x20   tags(pos: int): string\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   for pos in ^books(1).tags\n\
         \x20   \x20   if const tag = ^books(1).tags(pos)\n\
         \x20   \x20   \x20   print(tag)\n",
    );
}

#[test]
fn for_loop_over_composite_root_narrows_identity_reads() {
    let root = temp_project("presence-composite-root-loop-narrowing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             fn f()\n\
             \x20   for id in ^books\n\
             \x20       const book: Book = ^books(id)\n\
             \x20       print(book.title)\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn exact_non_unique_index_loop_over_composite_root_narrows_identity_reads() {
    let root = temp_project("presence-composite-index-loop-narrowing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             \x20   index byShelf(shelf, author, ordinal)\n\
             fn f()\n\
             \x20   for id in ^books.byShelf(\"fiction\")\n\
             \x20       const book: Book = ^books(id)\n\
             \x20       print(book.title)\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn bare_non_unique_index_loop_narrows_record_identity_reads() {
    // A bare loop over a non-unique index streams store identities of records
    // with that field populated, so the whole-record read is proven present.
    let root = temp_project("presence-index-bare-loop-identity", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   category: string\n\
             store ^books(id: string): Book\n\
             \x20   index byCategory(category, id)\n\
             fn f()\n\
             \x20   for id in ^books.byCategory\n\
             \x20       const book: Book = ^books(id)\n\
             \x20       print(book.title)\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn for_loop_over_composite_root_does_not_narrow_sparse_field_reads() {
    assert_bare_present_read(
        "presence-composite-root-loop-sparse-field",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(author: string, ordinal: int): Book\n\
             fn f()\n\
             \x20   for id in ^books\n\
             \x20       print(^books(id).subtitle)\n",
    );
}

#[test]
fn unknown_cannot_reenter_a_saved_identity_keyspace() {
    let root = temp_project("identity-unknown-keyspace", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn save(raw: unknown)\n\
             \x20   ^books(raw).title = \"bad\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_KEY_TYPE),
        "unknown must not act as any for saved identity keys: {:#?}",
        report.diagnostics
    );
}

#[test]
fn values_loop_does_not_narrow_value_as_an_entry_key() {
    assert_bare_present_read(
        "presence-values-loop-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for score in values(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(score))\n",
    );
}

#[test]
fn entries_loop_does_not_narrow_value_as_an_entry_key() {
    assert_bare_present_read(
        "presence-entries-loop-value-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for pos, score in entries(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(score))\n",
    );
}

#[test]
fn two_binding_keys_loop_does_not_narrow_ordinal_as_a_key() {
    assert_bare_present_read(
        "presence-two-binding-keys-loop-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for ordinal, pos in keys(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(ordinal))\n",
    );
}

#[test]
fn two_binding_reversed_keys_loop_does_not_narrow_ordinal_as_a_key() {
    assert_bare_present_read(
        "presence-two-binding-reversed-keys-loop-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for ordinal, pos in reversed(keys(^books(1).scores))\n\
             \x20   \x20   print(^books(1).scores(ordinal))\n",
    );
}

#[test]
fn two_binding_saved_path_loop_binds_the_value_present() {
    // The two-name value loop head binds each element value present, so a bare use of
    // the bound value type-checks.
    let root = temp_project("presence-two-binding-saved-path-loop-value", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for pos, score in ^books(1).scores\n\
             \x20   \x20   print(score)\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn two_binding_saved_path_loop_does_not_narrow_the_positional_re_read() {
    // Binding the value present does not narrow a positional re-read at the key: it
    // stays `T?` under the one rule.
    assert_bare_present_read(
        "presence-two-binding-saved-path-loop-reread",
        "module books\n\
         resource Book\n\
         \x20   scores(pos: int): int\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   for pos, score in ^books(1).scores\n\
         \x20   \x20   print(^books(1).scores(pos))\n",
    );
}

#[test]
fn duplicate_entries_loop_bindings_do_not_narrow_the_visible_value_as_a_key() {
    assert_bare_present_read(
        "presence-duplicate-entries-loop-bindings-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for x, x in entries(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(x))\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_same_condition_calls_saved_writer() {
    assert_bare_present_read(
        "presence-if-exists-condition-call-writes-saved",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn dropSubtitle(id: int): bool\n\
             \x20   delete ^books(id).subtitle\n\
             \x20   return true\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle) and dropSubtitle(id)\n\
             \x20   \x20   return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn a_bare_saved_read_is_an_unresolved_optional() {
    assert_bare_present_read(
        "presence-bare-pending",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn bare(id: int): string\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn a_bare_required_field_read_through_parameter_identity_requires_resolution() {
    assert_bare_present_read(
        "presence-required-param-id",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         fn requiredTitle(id: Id(^books)): string\n\
         \x20   return ^books(id).title\n",
    );
}

#[test]
fn early_return_if_not_exists_narrows_the_remainder() {
    assert_no_unresolved_optional(
        "presence-early-return-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn subtitleOrMissing(id: Id(^books)): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn if_not_exists_with_a_calling_body_does_not_narrow_the_remainder() {
    assert_bare_present_read(
        "presence-early-return-call-falls-through",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn note()\n\
         \x20   const value: int = 1\n\
         fn subtitleOrMissing(id: Id(^books)): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       note()\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn if_not_exists_with_a_looping_body_does_not_narrow_the_remainder() {
    assert_bare_present_read(
        "presence-early-return-loop-falls-through",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn subtitleOrMissing(id: Id(^books)): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       while false\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn while_body_narrowing_does_not_escape_the_loop() {
    assert_bare_present_read(
        "presence-while-body-narrowing-local",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: Id(^books)): string\n\
         \x20   while false\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn while_body_transient_invalidation_blocks_post_loop_reads() {
    assert_bare_present_read(
        "presence-while-transient-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   while true\n\
         \x20       delete ^books(id).subtitle\n\
         \x20       if stop\n\
         \x20           break\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn while_body_continue_before_reproof_blocks_post_loop_reads() {
    assert_bare_present_read(
        "presence-while-continue-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   while true\n\
         \x20       delete ^books(id).subtitle\n\
         \x20       if stop\n\
         \x20           continue\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20       break\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn while_body_saved_writing_call_blocks_post_loop_reads() {
    assert_bare_present_read(
        "presence-while-call-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn clear(id: Id(^books))\n\
         \x20   delete ^books(id).subtitle\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   while true\n\
         \x20       clear(id)\n\
         \x20       if stop\n\
         \x20           break\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn for_body_transient_invalidation_blocks_post_loop_reads() {
    assert_bare_present_read(
        "presence-for-transient-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   for other in ^books\n\
         \x20       delete ^books(id).subtitle\n\
         \x20       if stop\n\
         \x20           break\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn for_body_continue_before_reproof_blocks_post_loop_reads() {
    assert_bare_present_read(
        "presence-for-continue-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   for other in ^books\n\
         \x20       delete ^books(id).subtitle\n\
         \x20       if stop\n\
         \x20           continue\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn local_key_mutation_in_loop_invalidates_key_dependent_proof() {
    assert_bare_present_read(
        "presence-loop-key-mutation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: int, stop: bool): string\n\
         \x20   var key: int = id\n\
         \x20   if not exists(^books(key).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   while true\n\
         \x20       key = 2\n\
         \x20       if stop\n\
         \x20           break\n\
         \x20       if not exists(^books(key).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(key).subtitle\n",
    );
}

/// A guarded place the body clears on a later line must read as `Optional` at the
/// textually-earlier read: the back edge carries iteration one's clear to iteration
/// two's read, so the loop header re-imposes the one rule before the body is typed.
#[test]
fn while_loop_carried_clear_re_widens_an_earlier_body_read() {
    assert_bare_present_read(
        "presence-while-loop-carried-clear",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(go: bool)\n\
         \x20   if not exists(^books(1).subtitle)\n\
         \x20       return\n\
         \x20   while go\n\
         \x20       const s: string = ^books(1).subtitle\n\
         \x20       ^books(1).subtitle = absent\n",
    );
}

#[test]
fn for_loop_carried_clear_re_widens_an_earlier_body_read() {
    assert_bare_present_read(
        "presence-for-loop-carried-clear",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(ks: sequence[int])\n\
         \x20   if not exists(^books(1).subtitle)\n\
         \x20       return\n\
         \x20   for k in ks\n\
         \x20       const s: string = ^books(1).subtitle\n\
         \x20       ^books(1).subtitle = absent\n",
    );
}

#[test]
fn loop_carried_writer_call_re_widens_an_earlier_body_read() {
    assert_bare_present_read(
        "presence-while-loop-carried-call",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn clear(id: Id(^books))\n\
         \x20   delete ^books(id).subtitle\n\
         fn f(id: Id(^books), go: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   while go\n\
         \x20       const s: string = ^books(id).subtitle\n\
         \x20       clear(id)\n\
         \x20   return \"ok\"\n",
    );
}

/// A guard re-proved at the top of the body each iteration keeps the read sound: the
/// clear below the read is undone by the next iteration's guard, so re-widening the
/// header must not produce a false positive on the in-body read.
#[test]
fn a_guard_re_proved_in_the_loop_body_keeps_the_read_clean() {
    assert_no_unresolved_optional(
        "presence-while-in-body-guard-clean",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(go: bool)\n\
         \x20   while go\n\
         \x20       if not exists(^books(1).subtitle)\n\
         \x20           return\n\
         \x20       const s: string = ^books(1).subtitle\n\
         \x20       ^books(1).subtitle = absent\n",
    );
}

/// A loop that only reads a guarded place — never clearing it — keeps its narrowing:
/// the header re-widening drops only places the body could invalidate.
#[test]
fn a_read_only_loop_body_keeps_its_narrowing() {
    assert_no_unresolved_optional(
        "presence-while-read-only-keeps-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(go: bool)\n\
         \x20   if not exists(^books(1).subtitle)\n\
         \x20       return\n\
         \x20   while go\n\
         \x20       const s: string = ^books(1).subtitle\n\
         \x20       print(s)\n",
    );
}

#[test]
fn try_body_narrowing_does_not_escape_the_try() {
    assert_bare_present_read(
        "presence-try-body-narrowing-local",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn skip()\n\
         \x20   throw Error(code: \"test.skip\", message: \"skip\")\n\
         fn leaked(id: Id(^books)): string\n\
         \x20   try\n\
         \x20       skip()\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   catch err: Error\n\
         \x20       const ignored: int = 1\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn try_body_transient_invalidation_blocks_catch_reads() {
    assert_bare_present_read(
        "presence-try-catch-transient-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   try\n\
         \x20       delete ^books(id).subtitle\n\
         \x20       if stop\n\
         \x20           throw Error(code: \"test.stop\", message: \"stop\")\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   catch err: Error\n\
         \x20       return ^books(id).subtitle\n\
         \x20   return \"ok\"\n",
    );
}

#[test]
fn saved_writing_call_in_try_blocks_catch_reads() {
    assert_bare_present_read(
        "presence-try-call-invalidation",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn clear(id: Id(^books))\n\
         \x20   delete ^books(id).subtitle\n\
         fn leaked(id: Id(^books), stop: bool): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   try\n\
         \x20       clear(id)\n\
         \x20       if stop\n\
         \x20           throw Error(code: \"test.stop\", message: \"stop\")\n\
         \x20       if not exists(^books(id).subtitle)\n\
         \x20           return \"missing\"\n\
         \x20   catch err: Error\n\
         \x20       return ^books(id).subtitle\n\
         \x20   return \"ok\"\n",
    );
}

#[test]
fn if_const_binding_guard_binds_the_present_value() {
    assert_no_errors(
        "presence-if-const-binding-guard",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn guarded(id: Id(^books)): string\n\
         \x20   if const subtitle = ^books(id).subtitle\n\
         \x20       return subtitle\n\
         \x20   return \"missing\"\n",
    );
}

#[test]
fn a_coalesce_fallback_resolves_a_maybe_present_read() {
    assert_no_errors(
        "presence-coalesce-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn fallback(id: int): string\n\
         \x20   return ^books(id).subtitle ?? \"untitled\"\n",
    );
}

#[test]
fn an_exists_guard_resolves_a_maybe_present_read() {
    assert_no_errors(
        "presence-exists-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn found(id: int): bool\n\
         \x20   return exists(^books(id).subtitle)\n",
    );
}

#[test]
fn an_optional_chain_fallback_resolves_a_maybe_present_read() {
    assert_no_errors(
        "presence-optional-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn optional(id: int): string\n\
         \x20   return ^books(id)?.subtitle ?? \"untitled\"\n",
    );
}

/// A sparse field reached through a maybe-present record collapses to one optional
/// layer (`string?`, never `string??`), so it flows into a `string?` slot directly.
#[test]
fn optional_chain_through_a_maybe_present_record_is_one_optional_layer() {
    assert_no_errors(
        "presence-optional-chain-collapse",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn maybeSubtitle(id: int): string?\n\
         \x20   return ^books(id)?.subtitle\n",
    );
}

/// The same `string?` chain used where a definite `string` is required is the one
/// rule: the collapsed optional must be resolved first.
#[test]
fn optional_chain_at_a_definite_slot_is_the_one_rule() {
    assert_bare_present_read(
        "presence-optional-chain-one-rule",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn definiteSubtitle(id: int): string\n\
         \x20   return ^books(id)?.subtitle\n",
    );
}

/// A saved write to a *sibling* field of the narrowed key keeps the narrowing: the
/// member-precise invalidation must not drop it just because the write reuses the
/// same key binding.
#[test]
fn a_sibling_field_write_keeps_a_saved_narrowing() {
    assert_no_unresolved_optional(
        "presence-sibling-field-write-keeps-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         \x20   blurb: string\n\
         store ^books(id: int): Book\n\
         fn f(a: int)\n\
         \x20   if not exists(^books(a).subtitle)\n\
         \x20       return\n\
         \x20   ^books(a).blurb = \"x\"\n\
         \x20   const s: string = ^books(a).subtitle\n",
    );
}

/// A saved write to a *different resource* keyed by the same binding keeps the
/// narrowing: a write that merely reuses the key binding is not a clear of the
/// narrowed node.
#[test]
fn a_different_resource_write_keeps_a_saved_narrowing() {
    assert_no_unresolved_optional(
        "presence-other-resource-write-keeps-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         resource Tag\n\
         \x20   label: string\n\
         store ^books(id: int): Book\n\
         store ^tags(id: int): Tag\n\
         fn f(a: int)\n\
         \x20   if not exists(^books(a).subtitle)\n\
         \x20       return\n\
         \x20   ^tags(a).label = \"x\"\n\
         \x20   const s: string = ^books(a).subtitle\n",
    );
}

/// A write to the *same* field of the narrowed key drops the narrowing: the node
/// may have been re-cleared, so the later read is the one rule again.
#[test]
fn a_same_field_write_drops_a_saved_narrowing() {
    assert_bare_present_read(
        "presence-same-field-write-drops-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(a: int): string\n\
         \x20   if not exists(^books(a).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   ^books(a).subtitle = \"x\"\n\
         \x20   return ^books(a).subtitle\n",
    );
}

/// Reassigning the local key binding drops a saved narrowing keyed on it: the key
/// now addresses a different node.
#[test]
fn a_local_key_reassignment_drops_a_saved_narrowing() {
    assert_bare_present_read(
        "presence-local-key-reassignment-drops-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(start: int): string\n\
         \x20   var a: int = start\n\
         \x20   if not exists(^books(a).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   a = 2\n\
         \x20   return ^books(a).subtitle\n",
    );
}

/// A same-field write under an *alias-possible* key drops the narrowing: two
/// distinct key expressions may denote the same node at runtime.
#[test]
fn a_same_field_write_under_an_alias_possible_key_drops_a_saved_narrowing() {
    assert_bare_present_read(
        "presence-alias-possible-key-write-drops-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f(a: int, other: int): string\n\
         \x20   if not exists(^books(a).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   ^books(other).subtitle = \"x\"\n\
         \x20   return ^books(a).subtitle\n",
    );
}
