//! Local-collection indexed reads and sparse fields of materialized values are
//! resolvable maybe-present reads: `??`/`if const`/`exists` accept them, a bare
//! read is rejected, and an effectful argument is never admitted into a guard.

use crate::support;
use marrow_check::CHECK_BARE_MAYBE_PRESENT_READ;

use support::{check_module_report, with_code};

fn assert_clean(name: &str, src: &str) {
    let report = check_module_report(name, src);
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

fn assert_bare_read(name: &str, src: &str) {
    let report = check_module_report(name, src);
    assert!(
        !with_code(&report, CHECK_BARE_MAYBE_PRESENT_READ).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

fn assert_code(name: &str, src: &str, code: &str) {
    let report = check_module_report(name, src);
    assert!(
        !with_code(&report, code).is_empty(),
        "expected {code}: {:#?}",
        report.diagnostics
    );
}

#[test]
fn guards_resolve_a_local_sequence_positional_read() {
    assert_clean(
        "local-seq-guards",
        "module m\n\
         fn f()\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   print(xs(1) ?? -1)\n\
         \x20   if const v = xs(1)\n\
         \x20       print(v)\n\
         \x20   if exists(xs(1))\n\
         \x20       print(99)\n",
    );
}

#[test]
fn guards_resolve_a_local_keyed_tree_read() {
    assert_clean(
        "local-tree-guards",
        "module m\n\
         fn f()\n\
         \x20   var counts(k: string): int\n\
         \x20   counts(\"a\") = 1\n\
         \x20   print(counts(\"a\") ?? -1)\n\
         \x20   if const v = counts(\"b\")\n\
         \x20       print(v)\n\
         \x20   if exists(counts(\"a\"))\n\
         \x20       print(99)\n",
    );
}

#[test]
fn a_bare_local_sequence_read_must_be_resolved() {
    assert_bare_read(
        "local-seq-bare",
        "module m\n\
         fn f()\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   print(xs(99))\n",
    );
}

#[test]
fn a_bare_local_keyed_tree_read_must_be_resolved() {
    assert_bare_read(
        "local-tree-bare",
        "module m\n\
         fn f()\n\
         \x20   var counts(k: string): int\n\
         \x20   counts(\"a\") = 1\n\
         \x20   print(counts(\"zzz\"))\n",
    );
}

#[test]
fn guards_resolve_a_sparse_field_of_a_materialized_resource() {
    assert_clean(
        "sparse-materialized-guards",
        "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   if const p = ^books(1)\n\
         \x20       print(p.subtitle ?? \"\")\n\
         \x20       if exists(p.subtitle)\n\
         \x20           print(p.subtitle ?? \"\")\n",
    );
}

#[test]
fn a_bare_sparse_field_of_a_materialized_resource_must_be_resolved() {
    assert_bare_read(
        "sparse-materialized-bare",
        "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   if const p = ^books(1)\n\
         \x20       print(p.subtitle)\n",
    );
}

#[test]
fn guards_resolve_a_caught_error_sparse_field() {
    assert_clean(
        "error-sparse-guards",
        "module m\n\
         fn f()\n\
         \x20   try\n\
         \x20       throw Error(code: \"e.x\", message: \"boom\")\n\
         \x20   catch err\n\
         \x20       print(err.help ?? \"\")\n\
         \x20       if exists(err.data)\n\
         \x20           print(\"has data\")\n",
    );
}

#[test]
fn a_bare_caught_error_sparse_field_must_be_resolved() {
    assert_bare_read(
        "error-sparse-bare",
        "module m\n\
         fn f()\n\
         \x20   try\n\
         \x20       throw Error(code: \"e.x\", message: \"boom\")\n\
         \x20   catch err\n\
         \x20       print(err.help)\n",
    );
}

#[test]
fn a_required_field_of_a_materialized_resource_is_not_a_maybe_present_read() {
    // `title` is `required`, so it is always present on a materialized record. A
    // `??` over it has nothing to default and stays a coalesce-target error rather
    // than silently widening to a required field.
    assert_code(
        "required-field-rejected",
        "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   if const p = ^books(1)\n\
         \x20       print(p.title ?? \"\")\n",
        "check.operator_type",
    );
}

#[test]
fn exists_rejects_an_effectful_append_argument() {
    assert_code(
        "exists-append-rejected",
        "module m\n\
         fn f()\n\
         \x20   var xs: sequence[int]\n\
         \x20   if exists(append(xs, 1))\n\
         \x20       print(1)\n",
        "check.call_argument",
    );
}

#[test]
fn exists_rejects_an_effectful_next_id_argument() {
    assert_code(
        "exists-nextid-rejected",
        "module m\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         fn f()\n\
         \x20   if exists(nextId(^books))\n\
         \x20       print(1)\n",
        "check.call_argument",
    );
}

#[test]
fn coalesce_rejects_a_sequence_read_keyed_by_a_write_effect() {
    // A local-collection read is guardable, but a key argument that writes is an
    // effect inside the guard and must stay rejected by construction. `append`
    // returns an `int` position, so the rejection is the guard boundary refusing
    // the effect, not a key-type mismatch.
    assert_code(
        "coalesce-seq-write-key-rejected",
        "module m\n\
         fn f()\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   print(xs(append(xs, 20)) ?? -1)\n",
        "check.operator_type",
    );
}

/// A user function whose body writes saved data is a write effect, so a guard
/// keyed by a call to it must stay rejected — `??`, `if const`, and `exists`
/// alike. A plain key argument would type-check, so the rejection is the effect
/// boundary refusing the smuggled write, not an arity or key-type mismatch.
const SAVED_WRITER_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     fn writeBook(): int\n\
     \x20   ^books(1).title = \"x\"\n\
     \x20   return 1\n\
     fn f()\n\
     \x20   var counts(k: int): int\n\
     \x20   counts(1) = 10\n";

#[test]
fn coalesce_rejects_a_local_tree_keyed_by_a_saved_writing_function() {
    assert_code(
        "coalesce-tree-fn-write-key-rejected",
        &format!("{SAVED_WRITER_PRELUDE}\x20   print(counts(writeBook()) ?? -1)\n"),
        "check.operator_type",
    );
}

#[test]
fn if_const_rejects_a_local_tree_keyed_by_a_saved_writing_function() {
    assert_code(
        "if-const-tree-fn-write-key-rejected",
        &format!(
            "{SAVED_WRITER_PRELUDE}\x20   if const v = counts(writeBook())\n\
             \x20       print(v)\n"
        ),
        "check.condition_type",
    );
}

#[test]
fn exists_rejects_a_local_tree_keyed_by_a_saved_writing_function() {
    assert_code(
        "exists-tree-fn-write-key-rejected",
        &format!(
            "{SAVED_WRITER_PRELUDE}\x20   if exists(counts(writeBook()))\n\
             \x20       print(1)\n"
        ),
        "check.call_argument",
    );
}

/// `nextId(^s)` allocates a saved identity, an effect that may not ride into a
/// guard as a key argument. An `Id`-keyed local tree no longer masks the
/// allocation with a key-type mismatch, so the effect boundary itself must reject
/// it under `??`, `if const`, and `exists` rather than admit a check-clean read
/// that faults at runtime.
const ID_KEYED_TREE_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     fn f()\n\
     \x20   var byId(k: Id(^books)): int\n";

#[test]
fn coalesce_rejects_a_local_tree_keyed_by_next_id() {
    assert_code(
        "coalesce-tree-nextid-key-rejected",
        &format!("{ID_KEYED_TREE_PRELUDE}\x20   print(byId(nextId(^books)) ?? -1)\n"),
        "check.operator_type",
    );
}

#[test]
fn if_const_rejects_a_local_tree_keyed_by_next_id() {
    assert_code(
        "if-const-tree-nextid-key-rejected",
        &format!(
            "{ID_KEYED_TREE_PRELUDE}\x20   if const v = byId(nextId(^books))\n\
             \x20       print(v)\n"
        ),
        "check.condition_type",
    );
}

#[test]
fn exists_rejects_a_local_tree_keyed_by_next_id() {
    assert_code(
        "exists-tree-nextid-key-rejected",
        &format!(
            "{ID_KEYED_TREE_PRELUDE}\x20   if exists(byId(nextId(^books)))\n\
             \x20       print(1)\n"
        ),
        "check.call_argument",
    );
}

/// A sparse field of a group entry bound by a `for` loop is a maybe-present read
/// of the same shape as a sparse field of a materialized record: the guard is
/// accepted and a bare read is rejected. The loop binding must carry its element
/// type so the presence walk classifies the field the same way the type pass does.
const GROUP_LOOP_PRELUDE: &str = "module m\n\
     resource Library\n\
     \x20   required name: string\n\
     \x20   books(bid: int)\n\
     \x20       required title: string\n\
     \x20       subtitle: string\n\
     store ^libraries(id: int): Library\n";

#[test]
fn a_bare_sparse_field_of_a_loop_bound_group_entry_must_be_resolved() {
    assert_bare_read(
        "loop-group-sparse-bare",
        &format!(
            "{GROUP_LOOP_PRELUDE}fn f()\n\
             \x20   for b in values(^libraries(1).books)\n\
             \x20       print(b.subtitle)\n"
        ),
    );
}

#[test]
fn guards_resolve_a_sparse_field_of_a_loop_bound_group_entry() {
    assert_clean(
        "loop-group-sparse-guards",
        &format!(
            "{GROUP_LOOP_PRELUDE}fn f()\n\
             \x20   for b in values(^libraries(1).books)\n\
             \x20       print(b.subtitle ?? \"\")\n\
             \x20       if exists(b.subtitle)\n\
             \x20           print(b.subtitle ?? \"\")\n"
        ),
    );
}

/// A sparse field of the value bound by the second name of a bare two-name loop
/// is a maybe-present read of the same shape as a sparse field of a materialized
/// record: the guard is accepted and a bare read is rejected. The presence walk
/// must derive the second-name value type for every loop shape the type pass
/// binds, not only `values(...)`/`entries(...)` wrappers, or the bare read
/// escapes the check and faults at runtime.
const TWO_NAME_LOOP_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required shelf: string\n\
     \x20   subtitle: string\n\
     store ^vols(id: int): Book\n\
     \x20   index byShelf(shelf, id)\n";

#[test]
fn a_bare_sparse_field_of_a_two_name_saved_root_loop_must_be_resolved() {
    assert_bare_read(
        "two-name-root-bare",
        &format!(
            "{TWO_NAME_LOOP_PRELUDE}fn f()\n\
             \x20   for id, b in ^vols\n\
             \x20       print(b.subtitle)\n"
        ),
    );
}

#[test]
fn guards_resolve_a_sparse_field_of_a_two_name_saved_root_loop() {
    assert_clean(
        "two-name-root-guards",
        &format!(
            "{TWO_NAME_LOOP_PRELUDE}fn f()\n\
             \x20   for id, b in ^vols\n\
             \x20       print(b.subtitle ?? \"\")\n\
             \x20       if exists(b.subtitle)\n\
             \x20           print(b.subtitle ?? \"\")\n"
        ),
    );
}

#[test]
fn a_bare_sparse_field_of_a_two_name_index_branch_loop_must_be_resolved() {
    assert_bare_read(
        "two-name-index-bare",
        &format!(
            "{TWO_NAME_LOOP_PRELUDE}fn f()\n\
             \x20   for id, b in ^vols.byShelf(\"fiction\")\n\
             \x20       print(b.subtitle)\n"
        ),
    );
}

#[test]
fn guards_resolve_a_sparse_field_of_a_two_name_index_branch_loop() {
    assert_clean(
        "two-name-index-guards",
        &format!(
            "{TWO_NAME_LOOP_PRELUDE}fn f()\n\
             \x20   for id, b in ^vols.byShelf(\"fiction\")\n\
             \x20       print(b.subtitle ?? \"\")\n"
        ),
    );
}

/// A bare two-name loop over a saved record layer binds the second name to the
/// group entry, so a sparse field of it is the same maybe-present read.
const TWO_NAME_LAYER_PRELUDE: &str = "module m\n\
     resource Library\n\
     \x20   required name: string\n\
     \x20   books(bid: int)\n\
     \x20       required title: string\n\
     \x20       subtitle: string\n\
     store ^libraries(id: int): Library\n";

#[test]
fn a_bare_sparse_field_of_a_two_name_record_layer_loop_must_be_resolved() {
    assert_bare_read(
        "two-name-layer-bare",
        &format!(
            "{TWO_NAME_LAYER_PRELUDE}fn f()\n\
             \x20   for bid, b in ^libraries(1).books\n\
             \x20       print(b.subtitle)\n"
        ),
    );
}

#[test]
fn guards_resolve_a_sparse_field_of_a_two_name_record_layer_loop() {
    assert_clean(
        "two-name-layer-guards",
        &format!(
            "{TWO_NAME_LAYER_PRELUDE}fn f()\n\
             \x20   for bid, b in ^libraries(1).books\n\
             \x20       print(b.subtitle ?? \"\")\n"
        ),
    );
}

/// A bare two-name loop over a local keyed tree of resources binds the second
/// name to the resource value, so a sparse field of it is the same maybe-present
/// read as the saved shapes.
const TWO_NAME_LOCAL_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     fn f()\n\
     \x20   var shelf(slot: int): Book\n\
     \x20   shelf(1) = Book(title: \"a\")\n";

#[test]
fn a_bare_sparse_field_of_a_two_name_local_tree_loop_must_be_resolved() {
    assert_bare_read(
        "two-name-local-bare",
        &format!(
            "{TWO_NAME_LOCAL_PRELUDE}\x20   for k, b in shelf\n\
             \x20       print(b.subtitle)\n"
        ),
    );
}

#[test]
fn guards_resolve_a_sparse_field_of_a_two_name_local_tree_loop() {
    assert_clean(
        "two-name-local-guards",
        &format!(
            "{TWO_NAME_LOCAL_PRELUDE}\x20   for k, b in shelf\n\
             \x20       print(b.subtitle ?? \"\")\n"
        ),
    );
}

/// A sparse field reached through a chained group base (`p.address.zip`) is a
/// maybe-present read: the guard is accepted and a bare read is rejected. Both
/// route through the same group-entry resolver as a single-name base.
const CHAINED_GROUP_PRELUDE: &str = "module m\n\
     resource Person\n\
     \x20   required name: string\n\
     \x20   address\n\
     \x20       required city: string\n\
     \x20       zip: string\n\
     store ^people(id: int): Person\n";

#[test]
fn a_bare_sparse_field_through_a_chained_group_base_must_be_resolved() {
    assert_bare_read(
        "chained-group-sparse-bare",
        &format!(
            "{CHAINED_GROUP_PRELUDE}fn f()\n\
             \x20   if const p = ^people(1)\n\
             \x20       print(p.address.zip)\n"
        ),
    );
}

#[test]
fn guards_resolve_a_sparse_field_through_a_chained_group_base() {
    assert_clean(
        "chained-group-sparse-guards",
        &format!(
            "{CHAINED_GROUP_PRELUDE}fn f()\n\
             \x20   if const p = ^people(1)\n\
             \x20       print(p.address.zip ?? \"\")\n\
             \x20       if exists(p.address.zip)\n\
             \x20           print(p.address.zip ?? \"\")\n"
        ),
    );
}

/// A required field reached through a chained group base has nothing to default,
/// so its guard stays a coalesce-target error rather than silently widening.
#[test]
fn a_required_field_through_a_chained_group_base_is_not_maybe_present() {
    assert_code(
        "chained-group-required-rejected",
        &format!(
            "{CHAINED_GROUP_PRELUDE}fn f()\n\
             \x20   if const p = ^people(1)\n\
             \x20       print(p.address.city ?? \"\")\n"
        ),
        "check.operator_type",
    );
}

/// A sparse-field guard widens only a bound materialized value. A call or
/// constructor in the read place is rejected, because evaluating the guard base
/// would run that expression — and a function body may write saved data, open a
/// transaction, call a host capability, or throw, none of which may ride into a
/// `??`/`if const`/`exists` guard. The call result must be bound first, then its
/// sparse field guarded off the bound name. These preludes share one resource and
/// a pure `Book`-returning function so the only difference between the rejected
/// inline-call form and the accepted bound-name form is whether a name binds the
/// result.
const VALUE_BASE_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     fn makeBook(): Book\n\
     \x20   return Book(title: \"x\")\n";

#[test]
fn a_sparse_field_off_a_function_return_base_is_not_a_path_read() {
    // The base is a call, not a bound name, so `??` has no path read to default and
    // reports the coalesce-target error rather than admitting the call into the guard.
    assert_code(
        "call-base-sparse-coalesce-rejected",
        &format!("{VALUE_BASE_PRELUDE}fn f()\n\x20   print(makeBook().subtitle ?? \"\")\n"),
        "check.operator_type",
    );
}

#[test]
fn if_const_rejects_a_sparse_field_off_a_function_return_base() {
    assert_code(
        "call-base-sparse-if-const-rejected",
        &format!(
            "{VALUE_BASE_PRELUDE}fn f()\n\
             \x20   if const v = makeBook().subtitle\n\
             \x20       print(v)\n"
        ),
        "check.condition_type",
    );
}

#[test]
fn exists_rejects_a_sparse_field_off_a_function_return_base() {
    assert_code(
        "call-base-sparse-exists-rejected",
        &format!(
            "{VALUE_BASE_PRELUDE}fn f()\n\
             \x20   if exists(makeBook().subtitle)\n\
             \x20       print(99)\n"
        ),
        "check.call_argument",
    );
}

#[test]
fn a_sparse_field_off_an_inline_constructor_base_is_not_a_path_read() {
    assert_code(
        "constructor-base-sparse-coalesce-rejected",
        &format!("{VALUE_BASE_PRELUDE}fn f()\n\x20   print(Book(title: \"y\").subtitle ?? \"\")\n"),
        "check.operator_type",
    );
}

#[test]
fn exists_rejects_a_sparse_field_off_an_inline_constructor_base() {
    assert_code(
        "constructor-base-sparse-exists-rejected",
        &format!(
            "{VALUE_BASE_PRELUDE}fn f()\n\
             \x20   if exists(Book(title: \"y\").subtitle)\n\
             \x20       print(99)\n"
        ),
        "check.call_argument",
    );
}

#[test]
fn guards_resolve_a_sparse_field_off_a_bound_call_result() {
    // Binding the call result first makes the read place a bound name, so the guard
    // widens the sparse field with no call left in the read position.
    assert_clean(
        "bound-call-result-sparse-guards",
        &format!(
            "{VALUE_BASE_PRELUDE}fn f()\n\
             \x20   const b = makeBook()\n\
             \x20   print(b.subtitle ?? \"\")\n\
             \x20   if exists(b.subtitle)\n\
             \x20       print(b.subtitle ?? \"\")\n"
        ),
    );
}

/// A bound materialized value's required field has nothing to default, so a `??`
/// over it stays a coalesce-target error rather than silently widening.
#[test]
fn a_required_field_off_a_bound_call_result_is_not_maybe_present() {
    assert_code(
        "bound-call-result-required-rejected",
        &format!(
            "{VALUE_BASE_PRELUDE}fn f()\n\
             \x20   const b = makeBook()\n\
             \x20   print(b.title ?? \"\")\n"
        ),
        "check.operator_type",
    );
}

/// A function whose body carries an effect — a saved write, a transaction, a host
/// call, or a throw — must never have its result's sparse field guarded inline,
/// because evaluating the guard base would run that effect. Each prelude defines a
/// `Book`-returning function with exactly one such effect in its body; the field
/// value itself would type-check, so a rejection proves the guard boundary refuses
/// the smuggled effect, not a type mismatch. Reaching the field through `??`,
/// `if const`, or `exists` must all reject.
const SAVED_WRITE_BASE_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     store ^books(id: int): Book\n\
     fn writeBook(): Book\n\
     \x20   ^books(1).title = \"z\"\n\
     \x20   return Book(title: \"w\")\n";

#[test]
fn coalesce_rejects_a_sparse_field_off_a_saved_writing_base() {
    assert_code(
        "saved-write-base-coalesce-rejected",
        &format!("{SAVED_WRITE_BASE_PRELUDE}fn f()\n\x20   print(writeBook().subtitle ?? \"\")\n"),
        "check.operator_type",
    );
}

#[test]
fn if_const_rejects_a_sparse_field_off_a_saved_writing_base() {
    assert_code(
        "saved-write-base-if-const-rejected",
        &format!(
            "{SAVED_WRITE_BASE_PRELUDE}fn f()\n\
             \x20   if const v = writeBook().subtitle\n\
             \x20       print(v)\n"
        ),
        "check.condition_type",
    );
}

#[test]
fn exists_rejects_a_sparse_field_off_a_saved_writing_base() {
    assert_code(
        "saved-write-base-exists-rejected",
        &format!(
            "{SAVED_WRITE_BASE_PRELUDE}fn f()\n\
             \x20   if exists(writeBook().subtitle)\n\
             \x20       print(1)\n"
        ),
        "check.call_argument",
    );
}

const TRANSACTION_BASE_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     store ^books(id: int): Book\n\
     fn txnBook(): Book\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"z\"\n\
     \x20   return Book(title: \"w\")\n";

#[test]
fn coalesce_rejects_a_sparse_field_off_a_transaction_opening_base() {
    assert_code(
        "transaction-base-coalesce-rejected",
        &format!("{TRANSACTION_BASE_PRELUDE}fn f()\n\x20   print(txnBook().subtitle ?? \"\")\n"),
        "check.operator_type",
    );
}

#[test]
fn exists_rejects_a_sparse_field_off_a_transaction_opening_base() {
    assert_code(
        "transaction-base-exists-rejected",
        &format!(
            "{TRANSACTION_BASE_PRELUDE}fn f()\n\
             \x20   if exists(txnBook().subtitle)\n\
             \x20       print(1)\n"
        ),
        "check.call_argument",
    );
}

const HOST_CALL_BASE_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     fn loudBook(): Book\n\
     \x20   print(\"hello\")\n\
     \x20   return Book(title: \"w\")\n";

#[test]
fn coalesce_rejects_a_sparse_field_off_a_host_calling_base() {
    assert_code(
        "host-call-base-coalesce-rejected",
        &format!("{HOST_CALL_BASE_PRELUDE}fn f()\n\x20   print(loudBook().subtitle ?? \"\")\n"),
        "check.operator_type",
    );
}

#[test]
fn exists_rejects_a_sparse_field_off_a_host_calling_base() {
    assert_code(
        "host-call-base-exists-rejected",
        &format!(
            "{HOST_CALL_BASE_PRELUDE}fn f()\n\
             \x20   if exists(loudBook().subtitle)\n\
             \x20       print(1)\n"
        ),
        "check.call_argument",
    );
}

const THROW_BASE_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     fn riskyBook(): Book\n\
     \x20   throw Error(code: \"e.x\", message: \"boom\")\n";

#[test]
fn coalesce_rejects_a_sparse_field_off_a_throwing_base() {
    assert_code(
        "throw-base-coalesce-rejected",
        &format!("{THROW_BASE_PRELUDE}fn f()\n\x20   print(riskyBook().subtitle ?? \"\")\n"),
        "check.operator_type",
    );
}

#[test]
fn exists_rejects_a_sparse_field_off_a_throwing_base() {
    assert_code(
        "throw-base-exists-rejected",
        &format!(
            "{THROW_BASE_PRELUDE}fn f()\n\
             \x20   if exists(riskyBook().subtitle)\n\
             \x20       print(1)\n"
        ),
        "check.call_argument",
    );
}

/// A local-collection read keyed by an effectful function call must stay rejected:
/// the collection base is a bound name, but the key sub-expression is screened so
/// a transaction, host call, or throw smuggled into the key never rides into the
/// guard. The saved-write key case is covered above; these cover the other effects.
const KEY_EFFECT_PRELUDE: &str = "module m\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     fn txnInt(): int\n\
     \x20   transaction\n\
     \x20       ^books(1).title = \"z\"\n\
     \x20   return 1\n\
     fn loudInt(): int\n\
     \x20   print(\"hi\")\n\
     \x20   return 1\n\
     fn f()\n\
     \x20   var counts(k: int): int\n\
     \x20   counts(1) = 10\n";

#[test]
fn coalesce_rejects_a_local_tree_keyed_by_a_transaction_opening_function() {
    assert_code(
        "coalesce-tree-txn-key-rejected",
        &format!("{KEY_EFFECT_PRELUDE}\x20   print(counts(txnInt()) ?? -1)\n"),
        "check.operator_type",
    );
}

#[test]
fn coalesce_rejects_a_local_tree_keyed_by_a_host_calling_function() {
    assert_code(
        "coalesce-tree-host-key-rejected",
        &format!("{KEY_EFFECT_PRELUDE}\x20   print(counts(loudInt()) ?? -1)\n"),
        "check.operator_type",
    );
}
