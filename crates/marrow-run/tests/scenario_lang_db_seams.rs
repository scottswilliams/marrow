//! Tier-2 scenarios over the production runtime pipeline that exercise the seams
//! where typed `.mw` language semantics meet durable saved data: typed read-back
//! and typed absence, presence proofs that match storage, source spelling that is
//! independent of the physical store key, cross-module saves, and the all-or-nothing
//! contract of a multi-write transaction.
//!
//! Each scenario characterizes current v0.1 behavior with typed oracles: runtime
//! `Value`s, typed `RuntimeError` codes, and direct store effects read back through
//! `read_data_value` against the resource's content-independent catalog identity.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::value::{SavedValue, ScalarType};

/// A book whose `published` flag and `pages` count exercise non-string scalar
/// read-back, plus a sparse `subtitle` that is never written by `seed`.
const TYPED_BOOK: &str = "\
resource Book
    required title: string
    published: bool
    pages: int
    subtitle: string
store ^books(id: int): Book

pub fn seed(id: int)
    ^books(id).title = \"Mort\"
    ^books(id).published = true
    ^books(id).pages = 243

pub fn titleOf(id: int): string
    return ^books(id).title ?? \"\"

pub fn publishedOf(id: int): bool
    return ^books(id).published ?? false

pub fn pagesOf(id: int): int
    return ^books(id).pages ?? 0
";

#[test]
fn a_saved_scalar_reads_back_as_its_declared_type() {
    // A bool and an int field round-trip through the runtime as their declared
    // types, and the store carries the same canonical scalar values.
    let program = checked_program(TYPED_BOOK);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into())),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::publishedOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::pagesOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Int(243)),
    );

    // Direct store-effect oracle: the persisted bytes decode to the same scalars.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["published"]),
            ScalarType::Bool,
        ),
        Some(SavedValue::Bool(true)),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["pages"]),
            ScalarType::Int,
        ),
        Some(SavedValue::Int(243)),
    );
}

#[test]
fn an_absent_sparse_field_is_typed_absence_not_a_wrong_typed_default() {
    // `seed` never writes `subtitle`. The store holds no value at that path (absence
    // is structural, not a zero-of-type), and the only typed value at the read site
    // is the explicit `??` fallback the program supplies.
    let program = checked_program(&format!(
        "{TYPED_BOOK}\npub fn subtitleOr(id: int, fallback: string): string\n    return ^books(id).subtitle ?? fallback\n"
    ));
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");

    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["subtitle"]),
            ScalarType::Str,
        ),
        None,
        "an unwritten sparse field is absent in the store, not an empty string",
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::subtitleOr",
                Value::Int(1),
                Value::Str("none".into())
            )
        )
        .expect("read")
        .value,
        Some(Value::Str("none".into())),
        "the read resolves to the supplied fallback, not a stored value",
    );
}

#[test]
fn presence_narrows_true_after_a_write_and_false_before_and_after_delete() {
    // The checker's `exists(...)` proof tracks what the runtime actually reads from
    // the store across the field's whole lifecycle: absent, then present, then gone.
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\n\
         pub fn put(id: int, t: string)\n    ^books(id).title = t\n\n\
         pub fn drop(id: int)\n    delete ^books(id).title\n\n\
         pub fn present(id: int): bool\n    return exists(^books(id).title)\n",
    );
    let store = empty_store();

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::present", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "absent before any write",
    );
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::put",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::present", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(true)),
        "present after the write",
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::drop", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::present", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "absent again after the delete",
    );
}

#[test]
fn quoted_field_spelling_addresses_the_same_physical_key_as_the_bare_name() {
    // Quoting a field at the access site is render-only: `^books(id)."title"` and
    // `^books(id).title` resolve to the same content-independent stored key, so a
    // bare write is visible through a quoted read and vice versa.
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\n\
         pub fn setBare(id: int, t: string)\n    ^books(id).title = t\n\n\
         pub fn setQuoted(id: int, t: string)\n    ^books(id).\"title\" = t\n\n\
         pub fn getBare(id: int): string\n    return ^books(id).title ?? \"<absent>\"\n\n\
         pub fn getQuoted(id: int): string\n    return ^books(id).\"title\" ?? \"<absent>\"\n",
    );
    let store = empty_store();

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setBare",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("bare write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::getQuoted", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into())),
        "a quoted read sees the value a bare write stored",
    );

    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setQuoted",
            Value::Int(1),
            Value::Str("Reaper".into())
        ),
    )
    .expect("quoted write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::getBare", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Reaper".into())),
        "a bare read sees the value a quoted write stored",
    );

    // The store holds exactly one value at the single member's catalog key.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Reaper".into())),
    );
}

#[test]
fn a_keyword_spelled_field_round_trips_through_its_quoted_access_form() {
    // A field declared with a keyword spelling (`type`) is written and read only
    // through its quoted access form, yet it is an ordinary managed field: the
    // value round-trips and the store holds it at that member's key.
    let program = checked_program(
        "resource Book\n    type: string\nstore ^books(id: int): Book\n\n\
         pub fn setType(id: int, t: string)\n    ^books(id).\"type\" = t\n\n\
         pub fn typeOf(id: int): string\n    return ^books(id).\"type\" ?? \"<absent>\"\n",
    );
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setType",
            Value::Int(1),
            Value::Str("novel".into())
        ),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::typeOf", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("novel".into())),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["type"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("novel".into())),
    );
}

#[test]
fn a_resource_declared_in_one_module_is_saved_and_read_across_modules() {
    // The store schema lives in module `schema`; module `app` writes and reads it
    // through `use schema`. The save lands under the shared catalog identity, so a
    // direct store read against `books` sees the cross-module write.
    let program = checked_program_modules(&[
        "module schema\nresource Book\n    required title: string\n\nstore ^books(id: int): Book\n",
        "module app\nuse schema\n\n\
         pub fn setTitle(id: int, t: string)\n    ^books(id).title = t\n\n\
         pub fn titleOf(id: int): string\n    return ^books(id).title ?? \"\"\n",
    ]);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "app::setTitle",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("cross-module write");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "app::titleOf", Value::Int(1))
        )
        .expect("cross-module read")
        .value,
        Some(Value::Str("Mort".into())),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Mort".into())),
        "the store carries the cross-module write under the shared identity",
    );
}

/// An account with a current `name` plus a fresh `note`, used to prove that a
/// transaction either persists every write or none of them.
const ACCOUNT_TXN: &str = "\
resource Account
    name: string
    note: string
store ^accounts(id: int): Account

pub fn seed(id: int)
    ^accounts(id).name = \"start\"

pub fn risky(id: int)
    transaction
        ^accounts(id).name = \"changed\"
        ^accounts(id).note = \"audit\"
        throw Error(code: \"test.boom\", message: \"forced fault\")

pub fn good(id: int)
    transaction
        ^accounts(id).name = \"changed\"
        ^accounts(id).note = \"audit\"
";

#[test]
fn a_faulting_multi_write_transaction_persists_none_of_its_writes() {
    // A transaction that writes two fields then throws rolls back as a unit: the
    // updated field reverts to its pre-transaction value and the newly created
    // field never appears in the store.
    let program = checked_program(ACCOUNT_TXN);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::risky", Value::Int(1)),
        ),
        "run.uncaught_error",
    );

    assert_eq!(
        read_data_value(
            &program,
            &store,
            "accounts",
            &[SavedKey::Int(1)],
            &data_path(&program, "accounts", &["name"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("start".into())),
        "the updated field rolled back to its pre-transaction value",
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "accounts",
            &[SavedKey::Int(1)],
            &data_path(&program, "accounts", &["note"]),
            ScalarType::Str,
        ),
        None,
        "the field first written inside the rolled-back transaction never persisted",
    );
}

#[test]
fn a_completing_multi_write_transaction_persists_all_of_its_writes() {
    // The mirror of the rollback case: when the block exits without an escaping
    // error, every write in it commits together.
    let program = checked_program(ACCOUNT_TXN);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(&program, "test::good", Value::Int(1)),
    )
    .expect("commit");

    assert_eq!(
        read_data_value(
            &program,
            &store,
            "accounts",
            &[SavedKey::Int(1)],
            &data_path(&program, "accounts", &["name"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("changed".into())),
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "accounts",
            &[SavedKey::Int(1)],
            &data_path(&program, "accounts", &["note"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("audit".into())),
    );
}
