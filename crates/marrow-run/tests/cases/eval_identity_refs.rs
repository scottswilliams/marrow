//! Typed identity reference fields (`Id(^store)`): canonical key-encoded storage,
//! round-trip through field and whole-record writes, self-reference, and value
//! equality across identity origins.

use crate::support;
use support::*;

use marrow_run::{RUN_TYPE, Value};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;

#[test]
fn an_identity_field_round_trips_through_saved_data() {
    // A `Book.authorId: Id(^authors)` field stores an identity and reads it back as
    // the same identity value produced by the author store.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_stored_identity_field_reads_back_the_identity_value() {
    // The stored leaf carries the referenced identity's key segments, not a plain
    // scalar field encoding.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    // The stored leaf is the canonical identity encoding — the same
    // order-preserving key bytes a unique index entry stores — not a scalar
    // `encode_value`.
    let stored = read_data_bytes(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["authorId"]),
    );
    assert_eq!(
        stored,
        Some(encode_identity_payload(&[SavedKey::Int(1)])),
        "identity field stored as its canonical key encoding"
    );
}

#[test]
fn a_type_wrong_identity_field_does_not_decode_as_an_identity_value() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn read(): bool\n\
         \x20   const fallback = Id(^authors, 7)\n\
         \x20   const stored: Id(^authors) = ^books(1).authorId ?? fallback\n\
         \x20   return stored == fallback\n",
    );
    let store = TreeStore::memory();
    let path = data_path(&program, "books", &["authorId"]);
    write_data_bytes(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &path,
        encode_identity_payload(&[SavedKey::Str("not-an-int".to_string())]),
    );

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::read")),
        RUN_TYPE,
    );
}

#[test]
fn a_self_referencing_identity_field_round_trips() {
    // A field of the same resource (`managerId: Id(^people)` on `Person`) is a valid
    // self-reference that stores and reads back like any other typed reference.
    let program = checked_program(
        "resource Person\n\
         \x20   managerId: Id(^people)\nstore ^people(id: int): Person\n\
         \n\
         pub fn seed(): bool\n\
         \x20   const person = nextId(^people)\n\
         \x20   ^people(person).managerId = person\n\
         \x20   const manager = nextId(^people)\n\
         \x20   ^people(person).managerId = manager\n\
         \x20   return read(manager)\n\
         \n\
         pub fn read(expected: Id(^people)): bool\n\
         \x20   const stored: Id(^people) = ^people(1).managerId ?? expected\n\
         \x20   return stored == expected\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::seed"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn equality_on_two_identities_of_the_same_store_evaluates() {
    // `==` on two identities of the same store is value equality of their keys:
    // equal keys are `true`, differing keys are `false`.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         pub fn same(): bool\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   return author == author\n\
         \n\
         pub fn different(): bool\n\
         \x20   const ada = nextId(^authors)\n\
         \x20   ^authors(ada).name = \"Ada\"\n\
         \x20   const grace = nextId(^authors)\n\
         \x20   ^authors(grace).name = \"Grace\"\n\
         \x20   return ada == grace\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn single_key_store_identity_behaves_like_other_identity_origins() {
    let program = checked_program(
        "resource Doc\n\
         \x20   title: string\nstore ^docs(id: int): Doc\n\
         \n\
         pub fn idValue(): Id(^docs)\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return doc\n\
         \x20   return id\n\
         \n\
         pub fn mixedEq(): bool\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return id == doc\n\
         \x20   return false\n",
    );
    assert_identity_value(
        run(checked_entry!(&program, "test::idValue")).unwrap(),
        "docs",
        &[SavedKey::Int(1)],
    );
    assert_eq!(
        run(checked_entry!(&program, "test::mixedEq")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn explicit_single_key_identity_constructor_reads_and_writes_records() {
    let program = checked_program(
        "resource Book\n\
         \x20   title: string\n\
         store ^books(id: string): Book\n\
         \n\
         pub fn make(): Id(^books)\n\
         \x20   return Id(^books, \"book-17\")\n\
         \n\
         pub fn seed()\n\
         \x20   const id = Id(^books, \"book-17\")\n\
         \x20   ^books(id).title = \"Mort\"\n\
         \n\
         pub fn read(): string\n\
         \x20   return ^books(Id(^books, \"book-17\")).title ?? \"missing\"\n",
    );
    let store = TreeStore::memory();
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::make"))
            .expect("make runs")
            .value,
        "books",
        &[SavedKey::Str("book-17".to_string())],
    );
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read runs")
            .value,
        Some(Value::Str("Mort".to_string()))
    );
}

#[test]
fn explicit_composite_identity_constructor_addresses_records() {
    let program = checked_program(
        "resource Enrollment\n\
         \x20   grade: string\n\
         store ^enrollments(student: string, course: string): Enrollment\n\
         \n\
         pub fn make(): Id(^enrollments)\n\
         \x20   return Id(^enrollments, \"student-1\", \"course-9\")\n\
         \n\
         pub fn seed()\n\
         \x20   const id = Id(^enrollments, \"student-1\", \"course-9\")\n\
         \x20   ^enrollments(id).grade = \"A\"\n\
         \n\
         pub fn read(): string\n\
         \x20   return ^enrollments(Id(^enrollments, \"student-1\", \"course-9\")).grade ?? \"missing\"\n",
    );
    let store = TreeStore::memory();
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::make"))
            .expect("make runs")
            .value,
        "enrollments",
        &[
            SavedKey::Str("student-1".to_string()),
            SavedKey::Str("course-9".to_string()),
        ],
    );
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read runs")
            .value,
        Some(Value::Str("A".to_string()))
    );
}

#[test]
fn unique_index_identity_compares_with_the_allocated_identity() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   required isbn: string\nstore ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         pub fn seed(): bool\n\
         \x20   var b: Book\n\
         \x20   b.title = \"T\"\n\
         \x20   b.isbn = \"I-1\"\n\
         \x20   const id = nextId(^books)\n\
         \x20   ^books(id) = b\n\
         \x20   const found: Id(^books) = ^books.byIsbn(\"I-1\") ?? id\n\
         \x20   return id == found\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seed")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_whole_resource_write_with_an_identity_field_round_trips() {
    // A whole-record write `^books(1) = b` carrying an identity-typed field stores
    // the reference, and a whole-record read reads it back.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   var b: Book\n\
         \x20   b.title = \"Mort\"\n\
         \x20   b.authorId = author\n\
         \x20   ^books(1) = b\n\
         \n\
         pub fn read(): bool\n\
         \x20   if exists(^books(1))\n\
         \x20       const b = ^books(1)\n\
         \x20       for author in keys(^authors)\n\
         \x20           if const stored = b.authorId\n\
         \x20               return stored == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

const DOC_STORE: &str = "resource Doc\n\
     \x20   title: string\nstore ^docs(id: int): Doc\n\n";

#[test]
fn print_and_interpolation_render_a_saved_identity_by_its_key() {
    // `print`/interpolation render an identity directly as its key. `string(...)`
    // narrows identity out (see the rejection test), so the surfaces diverge by
    // design: only the render surfaces accept identity.
    let program = checked_program(&format!(
        "{DOC_STORE}\
         pub fn show()\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   print(id)\n\
         \x20   print($\"doc {{id}}\")\n",
    ));
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::show"))
            .expect("print identity")
            .output,
        "1\ndoc 1\n"
    );
}

#[test]
fn string_of_a_saved_identity_is_rejected_at_check() {
    // `string(...)` does not accept a saved identity; the conversion matrix narrows
    // it out at check, even though `print` renders one. The matrix, not the renderer,
    // owns which shapes `string(...)` admits.
    checker_rejects(
        &format!(
            "{DOC_STORE}\
             pub fn label(): string\n\
             \x20   const id = nextId(^docs)\n\
             \x20   return string(id)\n"
        ),
        "check.call_argument",
    );
}

/// Two `^aa` records and a `Bag` whose `refs` is a `sequence[Id(^aa)]`. Append, the
/// direct positional keyed-leaf write, and a read-back all flow through one
/// identity-leaf encoder.
const ID_SEQUENCE_SCHEMA: &str = "\
resource A
    name: string
store ^aa(id: int): A

resource Bag
    refs: sequence[Id(^aa)]
store ^bags(id: int): Bag

pub fn seed()
    ^aa(1).name = \"a\"
    ^aa(2).name = \"b\"

pub fn appendRef(key: int): int
    return append(^bags(1).refs, Id(^aa, key))

pub fn writeRef(bag: int, pos: int, key: int)
    ^bags(bag).refs(pos) = Id(^aa, key)

pub fn refAt(bag: int, pos: int): Id(^aa)
    return ^bags(bag).refs(pos) ?? Id(^aa, 1)
";

#[test]
fn appending_an_identity_into_a_saved_sequence_round_trips_like_the_direct_write() {
    let program = checked_program(ID_SEQUENCE_SCHEMA);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");

    // Successive appends take positions 1 then 2 and read back as the same identities.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::appendRef", Value::Int(1))
        )
        .expect("append runs")
        .value,
        Some(Value::Int(1)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::appendRef", Value::Int(2))
        )
        .expect("append runs")
        .value,
        Some(Value::Int(2)),
    );
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::refAt", Value::Int(1), Value::Int(1)),
        )
        .expect("read back")
        .value,
        "aa",
        &[SavedKey::Int(1)],
    );
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::refAt", Value::Int(1), Value::Int(2)),
        )
        .expect("read back")
        .value,
        "aa",
        &[SavedKey::Int(2)],
    );

    // The direct positional write of the same identity into a sibling bag stores the
    // byte-identical canonical payload: append and the keyed-leaf write share one
    // identity-leaf encoder.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::writeRef",
            Value::Int(2),
            Value::Int(1),
            Value::Int(1)
        ),
    )
    .expect("direct positional write runs");
    let appended = read_data_bytes(
        &program,
        &store,
        "bags",
        &[SavedKey::Int(1)],
        &keyed_data_path(&program, "bags", &[("refs", vec![SavedKey::Int(1)])], &[]),
    );
    let direct = read_data_bytes(
        &program,
        &store,
        "bags",
        &[SavedKey::Int(2)],
        &keyed_data_path(&program, "bags", &[("refs", vec![SavedKey::Int(1)])], &[]),
    );
    assert_eq!(appended, Some(encode_identity_payload(&[SavedKey::Int(1)])));
    assert_eq!(
        appended, direct,
        "append and the direct positional write encode an identity leaf identically"
    );
}

#[test]
fn appending_a_dangling_identity_stores_and_reads_back_per_the_identity_contract() {
    // Appending `Id(^aa, 99)` when `^aa(99)` was never created mirrors the direct
    // positional write: an identity leaf carries the referenced key, and resolving the
    // target is the reader's concern, so the append persists and reads back unchanged.
    let program = checked_program(ID_SEQUENCE_SCHEMA);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    run_entry(
        &store,
        checked_entry!(&program, "test::appendRef", Value::Int(99)),
    )
    .expect("dangling append runs");
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::refAt", Value::Int(1), Value::Int(1)),
        )
        .expect("read back")
        .value,
        "aa",
        &[SavedKey::Int(99)],
    );
}

#[test]
fn enum_and_int_sequence_appends_still_persist_alongside_identity_support() {
    // Identity-leaf support in `value_to_leaf` must leave the enum-leaf and scalar-leaf
    // append branches intact.
    let program = checked_program(
        "enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \n\
         resource Log\n\
         \x20   states: sequence[Status]\n\
         \x20   vals: sequence[int]\n\
         store ^logs(id: int): Log\n\
         \n\
         pub fn addState(): int\n\
         \x20   return append(^logs(1).states, Status::active)\n\
         \n\
         pub fn addVal(v: int): int\n\
         \x20   return append(^logs(1).vals, v)\n\
         \n\
         pub fn isActiveAt(pos: int): bool\n\
         \x20   const s: Status = ^logs(1).states(pos) ?? Status::archived\n\
         \x20   return s == Status::active\n\
         \n\
         pub fn valAt(pos: int): int\n\
         \x20   return ^logs(1).vals(pos) ?? 0\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::addState"))
            .expect("enum append runs")
            .value,
        Some(Value::Int(1)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::addVal", Value::Int(7))
        )
        .expect("int append runs")
        .value,
        Some(Value::Int(1)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::isActiveAt", Value::Int(1))
        )
        .expect("enum read back")
        .value,
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::valAt", Value::Int(1))
        )
        .expect("int read back")
        .value,
        Some(Value::Int(7)),
    );
}
