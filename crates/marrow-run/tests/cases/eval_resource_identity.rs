//! Resource-identity values: separate runtime roots, single- and composite-key
//! store identities, singleton stores, and unkeyed-group field reads/writes.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

#[test]
fn multiple_stores_over_one_resource_keep_runtime_roots_separate() {
    let program = checked_program(
        "resource Book\n\
         \x20   title: string\n\
         \n\
         store ^books(id: int): Book\n\
         store ^archivedBooks(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   ^books(1).title = \"live\"\n\
         \x20   ^archivedBooks(1).title = \"archived\"\n\
         \n\
         pub fn live(): string\n\
         \x20   return ^books(1).title ?? \"\"\n\
         \n\
         pub fn archived(): string\n\
         \x20   return ^archivedBooks(1).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let live = run_entry(&store, checked_entry!(&program, "test::live"))
        .expect("live")
        .value;
    assert_eq!(live, Some(Value::Str("live".into())));

    let archived = run_entry(&store, checked_entry!(&program, "test::archived"))
        .expect("archived")
        .value;
    assert_eq!(archived, Some(Value::Str("archived".into())));
}

/// A single-key store identity from `nextId(^books)` can be passed to saved reads
/// and writes. The identity carries the lowered key so `^books(id)` reads the
/// same record `^books(1)` does.
const BOOK_IDENTITY: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn save(t: string)
    const id = nextId(^books)
    ^books(id).title = t

pub fn title(): string
    for id in ^books
        return ^books(id).title ?? \"\"
    return \"\"
";

#[test]
fn allocates_and_uses_a_single_key_store_identity() {
    let program = checked_program(BOOK_IDENTITY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Str("Mort".into())),
    )
    .expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::title"))
        .expect("title")
        .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
    // The identity lowered to the same key a plain int does: `^books(1)`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Mort".into()))
    );
}

/// `key(id)` projects a single-scalar-key identity back to its key value. The
/// string slug written for a record is the exact value `key` returns when the
/// store iterator hands the identity back.
const TAG_KEY: &str = "\
resource Tag
    required label: string
store ^tags(slug: string): Tag

pub fn save(s: string, l: string)
    ^tags(s).label = l

pub fn firstSlug(): string
    for id in ^tags
        return key(id)
    return \"\"
";

#[test]
fn key_projects_a_string_identity_back_to_its_slug() {
    let program = checked_program(TAG_KEY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::save",
            Value::Str("rust".into()),
            Value::Str("Systems".into()),
        ),
    )
    .expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::firstSlug"))
        .expect("firstSlug")
        .value;
    assert_eq!(value, Some(Value::Str("rust".into())));
}

/// The int-keyed family: an allocated `Id(^books)` projects back to the `int` key
/// it lowered to. This proves `key` reads the lowered key, not a re-derived value.
const BOOK_KEY: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn save(t: string): int
    const id = nextId(^books)
    ^books(id).title = t
    return key(id)
";

#[test]
fn key_projects_an_allocated_int_identity_to_its_key() {
    let program = checked_program(BOOK_KEY);
    let store = TreeStore::memory();
    let value = run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Str("Mort".into())),
    )
    .expect("save")
    .value;
    assert_eq!(value, Some(Value::Int(1)));
}

#[test]
fn a_plain_int_identity_still_works() {
    // The bare int path remains the executable single-key store identity path.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn save()\n    ^books(1).title = \"a\"\n\npub fn read(): string\n    return ^books(1).title ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::read"))
        .expect("read")
        .value;
    assert_eq!(value, Some(Value::Str("a".into())));
}

/// A composite-key resource can still be addressed directly by its declared store
/// key order. Composite identity values come from traversal and indexes.
const ENROLLMENT_IDENTITY: &str = "\
resource Enrollment
    status: string
store ^enrollments(studentId: string, courseId: string): Enrollment

pub fn enroll(s: string, c: string, st: string)
    ^enrollments(s, c).status = st

pub fn statusOf(s: string, c: string): string
    return ^enrollments(s, c).status ?? \"\"
";

#[test]
fn constructs_and_uses_a_composite_identity_round_trips() {
    let program = checked_program(ENROLLMENT_IDENTITY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::enroll",
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
            Value::Str("active".into()),
        ),
    )
    .expect("enroll");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::statusOf",
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
        ),
    )
    .expect("statusOf")
    .value;
    assert_eq!(value, Some(Value::Str("active".into())));
    // Keys lowered in declared order: studentId then courseId.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "enrollments",
            &[
                SavedKey::Str("student-1".into()),
                SavedKey::Str("course-9".into()),
            ],
            &data_path(&program, "enrollments", &["status"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn composite_root_keys_write_in_declaration_order() {
    let program = checked_program(
        "resource Enrollment\n    status: string\nstore ^enrollments(studentId: string, courseId: string): Enrollment\n\npub fn enroll()\n    ^enrollments(\"s\", \"c\").status = \"active\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::enroll")).expect("enroll");
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "enrollments",
            &[SavedKey::Str("s".into()), SavedKey::Str("c".into())],
            &data_path(&program, "enrollments", &["status"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn whole_resource_read_through_an_identity() {
    // The primary-root iterator yields a composite identity that can be used for
    // a whole-resource read.
    let program = checked_program(
        "resource Enrollment\n    status: string\nstore ^enrollments(studentId: string, courseId: string): Enrollment\n\npub fn statusOf(): string\n    for id in ^enrollments\n        var e: Enrollment = ^enrollments(id)\n        return e.status ?? \"\"\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "enrollments",
        &[SavedKey::Str("s".into()), SavedKey::Str("c".into())],
        &data_path(&program, "enrollments", &["status"]),
        SavedValue::Str("active".into()),
    );
    let value = run_entry(&store, checked_entry!(&program, "test::statusOf"))
        .expect("statusOf")
        .value;
    assert_eq!(value, Some(Value::Str("active".into())));
}

// --- Singleton stores end-to-end ---

/// A singleton store (no identity keys). Field
/// read/write address the root directly, and whole read/write materialize and
/// replace the root as a resource value.
const SETTINGS: &str = "\
resource Settings
    required theme: string
    required maxLoans: int
store ^settings: Settings

pub fn init(t: string, n: int)
    transaction
        ^settings.theme = t
        ^settings.maxLoans = n

pub fn setMaxLoans(n: int)
    ^settings.maxLoans = n

pub fn setTheme(t: string)
    ^settings.theme = t

pub fn theme(): string
    return ^settings.theme ?? \"\"

pub fn themeViaGuard(): string
    if const settings = ^settings
        return settings.theme
    return \"\"

pub fn snapshot(): Settings
    var fallback: Settings
    fallback.theme = \"\"
    fallback.maxLoans = 0
    return ^settings ?? fallback

pub fn restore(s: Settings)
    ^settings = s

pub fn restoreFixture(theme: string, maxLoans: int)
    var s: Settings
    s.theme = theme
    s.maxLoans = maxLoans
    restore(s)
";

#[test]
fn singleton_field_read_and_write() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::init",
            Value::Str("light".into()),
            Value::Int(3)
        ),
    )
    .expect("init");
    run_entry(
        &store,
        checked_entry!(&program, "test::setMaxLoans", Value::Int(5)),
    )
    .expect("setMaxLoans");
    run_entry(
        &store,
        checked_entry!(&program, "test::setTheme", Value::Str("dark".into())),
    )
    .expect("setTheme");
    let value = run_entry(&store, checked_entry!(&program, "test::theme"))
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("dark".into())));
    // The field landed at `^settings.theme`, no record key in between.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "settings",
            &[],
            &data_path(&program, "settings", &["theme"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("dark".into()))
    );
}

#[test]
fn singleton_whole_read_and_write_round_trip() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    // Seed the singleton's fields directly.
    write_data_value(
        &program,
        &store,
        "settings",
        &[],
        &data_path(&program, "settings", &["theme"]),
        SavedValue::Str("light".into()),
    );
    write_data_value(
        &program,
        &store,
        "settings",
        &[],
        &data_path(&program, "settings", &["maxLoans"]),
        SavedValue::Int(5),
    );
    // A whole read materializes the singleton's present fields.
    let snapshot = run_entry(&store, checked_entry!(&program, "test::snapshot"))
        .expect("snapshot")
        .value;
    assert_eq!(
        snapshot,
        Some(Value::Resource(vec![
            ("theme".into(), Value::Str("light".into())),
            ("maxLoans".into(), Value::Int(5)),
        ]))
    );
    // A whole write replaces it; read it back via the field reader.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::restoreFixture",
            Value::Str("solar".into()),
            Value::Int(9)
        ),
    )
    .expect("restore");
    let value = run_entry(&store, checked_entry!(&program, "test::theme"))
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("solar".into())));
}

#[test]
fn singleton_whole_read_can_be_bound_by_if_const() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::init",
            Value::Str("light".into()),
            Value::Int(3)
        ),
    )
    .expect("init");
    let value = run_entry(&store, checked_entry!(&program, "test::themeViaGuard"))
        .expect("theme via guard")
        .value;
    assert_eq!(value, Some(Value::Str("light".into())));
}

// A whole read of a keyed root with no identity is untyped and rejected.
#[test]
fn a_whole_read_of_a_keyed_root_without_an_identity_is_rejected() {
    checker_rejects(
        "module test\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         pub fn read()\n\
         \x20   var b: Book = ^books\n",
        "check.untyped_value",
    );
}

#[test]
fn a_field_read_off_a_keyed_root_without_an_identity_is_rejected() {
    checker_rejects(
        "module test\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         pub fn read(): string\n\
         \x20   return ^books.title\n",
        "check.untyped_value",
    );
}

/// A resource with an unkeyed nested group (`name { first; last }`). Its fields
/// are addressed `^patients(p).name.first` — a `.field` off a `.field` off the
/// record, with no keyed layer in between.
const PATIENT_UNKEYED_GROUP: &str = "\
resource Patient
    mrn: string
    name
        first: string
        last: string
store ^patients(id: int): Patient

pub fn setName(id: int, f: string, l: string)
    ^patients(id).name.first = f
    ^patients(id).name.last = l

pub fn firstOf(id: int): string
    return ^patients(id)?.name?.first ?? \"\"

pub fn lastOf(id: int): string
    return ^patients(id)?.name?.last ?? \"\"
";

#[test]
fn unkeyed_group_field_write_then_read_round_trips() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setName",
            Value::Int(7),
            Value::Str("Terry".into()),
            Value::Str("Pratchett".into()),
        ),
    )
    .expect("setName");
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(7)))
            .expect("read")
            .value
    };
    assert_eq!(read("test::firstOf"), Some(Value::Str("Terry".into())));
    assert_eq!(read("test::lastOf"), Some(Value::Str("Pratchett".into())));
    // The field landed under the group layer `^patients(7).name.first`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "patients",
            &[SavedKey::Int(7)],
            &data_path(&program, "patients", &["name", "first"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Terry".into()))
    );
}

#[test]
fn an_absent_unkeyed_group_field_read_uses_the_default() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = TreeStore::memory();
    let value = run_entry(
        &store,
        checked_entry!(&program, "test::firstOf", Value::Int(1)),
    )
    .expect("default")
    .value;
    assert_eq!(value, Some(Value::Str(String::new())));
}
