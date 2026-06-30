use crate::checked_entry;
use crate::support::{assert_run_error, checked_program, run, run_entry};
use marrow_run::{RUN_TRAVERSAL, Value};
use marrow_store::tree::TreeStore;

#[test]
fn compound_assignment_updates_local_values() {
    let program = checked_program(
        "pub fn numbers(): int\n\
         \x20   var i: int = 2\n\
         \x20   i *= 3\n\
         \x20   return i\n\
         pub fn strings(): string\n\
         \x20   var s: string = \"a\"\n\
         \x20   s += \"b\"\n\
         \x20   return s\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::numbers")).unwrap(),
        Some(Value::Int(6))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::strings")).unwrap(),
        Some(Value::Str("ab".into()))
    );
}

#[test]
fn compound_assignment_updates_saved_field() {
    let program = checked_program(
        "resource Book\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         pub fn run(): int\n\
         \x20   var book: Book\n\
         \x20   book.pages = 10\n\
         \x20   ^books(1) = book\n\
         \x20   if exists(^books(1).pages)\n\
         \x20       ^books(1).pages *= 3\n\
         \x20   return ^books(1).pages ?? -1\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::run")).unwrap(),
        Some(Value::Int(30))
    );
}

#[test]
fn compound_assignment_updates_saved_keyed_leaf_target_once() {
    let program = checked_program(
        "resource Book\n\
         \x20   scores(pos: int): int\n\
         store ^books(id: int): Book\n\
         pub fn run(): int\n\
         \x20   var book: Book\n\
         \x20   var probes: sequence[int]\n\
         \x20   ^books(1) = book\n\
         \x20   ^books(1).scores(1) = 10\n\
         \x20   if exists(^books(1).scores(count(probes) + 1))\n\
         \x20       ^books(1).scores(count(probes) + 1) += append(probes, 0)\n\
         \x20   return count(probes) * 100 + (^books(1).scores(1) ?? -1) + (^books(1).scores(2) ?? -1)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::run")).unwrap(),
        Some(Value::Int(110))
    );
}

#[test]
fn compound_assignment_to_traversed_keyed_leaf_is_a_traversal_fault() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   scores(pos: int): int\n\
         store ^books(id: int): Book\n\
         pub fn seed()\n\
         \x20   ^books(1).title = \"a\"\n\
         \x20   ^books(1).scores(1) = 10\n\
         pub fn bump()\n\
         \x20   if exists(^books(1).scores(1))\n\
         \x20       ^books(1).scores(1) += 1\n\
         pub fn walk()\n\
         \x20   for score in ^books(1).scores\n\
         \x20       bump()\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::walk"));
    assert_run_error(faulted, RUN_TRAVERSAL);
}
