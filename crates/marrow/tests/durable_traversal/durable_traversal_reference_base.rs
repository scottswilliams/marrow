//! Entry references supply ancestor keys for bounded child traversal.
//! Runtime tests cover root and per-iteration references, deletion, overflow, and
//! composite identity operands against equivalent inline durable addresses.

use crate::common::{Project, Session};
use marrow_vm::Value;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.pos 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id product Enroll 41414141414141414141414141414141\n\
     id field Enroll.term 42424242424242424242424242424242\n\
     id root grades 43434343434343434343434343434343\n\
     id key grades.student 44444444444444444444444444444444\n\
     id key grades.course 45454545454545454545454545454545\n\
     id root Enroll.marks 46464646464646464646464646464646\n\
     id key Enroll.marks.slot 47474747474747474747474747474747\n\
     id field Enroll.marks.value 48484848484848484848484848484848\n\
     high-water 0\n\
     end\n";

/// A `Book { title }` root with a single-level `notes(pos: int)` branch and a composite
/// `^grades[student, course]` root whose `Enroll` carries a `marks(slot: int)` branch.
/// The `sum*ViaReference` exports bind a entry reference over an entry and traverse a branch
/// beneath it, folding the visited keys and adding 1000 in `on more`, so one returned
/// int witnesses which keys were frozen and whether `on more` ran.
const SOURCE: &str = r#"resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

resource Enroll {
    required term: string

    marks[slot: int] {
        required value: int
    }
}

store ^grades[student: string, course: string]: Enroll

pub fn putBook(id: int, t: string) {
    transaction {
        ^books[id] = Book(title: t)
    }
}

pub fn putNote(id: int, pos: int, t: string) {
    transaction {
        ^books[id].notes[pos] = Book.notes(text: t)
    }
}

pub fn putGrade(s: string, c: string, t: string) {
    transaction {
        ^grades[s, c] = Enroll(term: t)
    }
}

pub fn putMark(s: string, c: string, slot: int, v: int) {
    transaction {
        ^grades[s, c].marks[slot] = Enroll.marks(value: v)
    }
}

pub fn sumNotesViaReference(id: int): int {
    var total = 0
    ref b = ^books[id] else { return 0 }
    for pos in b.notes at most 100 {
        total += pos
    } on more {
        total = total + 1000
    }
    return total
}

pub fn sumNotesViaReferenceFirst2(id: int): int {
    var total = 0
    ref b = ^books[id] else { return 0 }
    for pos in b.notes at most 2 {
        total += pos
    } on more {
        total = total + 1000
    }
    return total
}

pub fn clearNotesViaReference(id: int): int {
    var total = 0
    transaction {
        ref b = ^books[id] else { return 0 }
        for pos in b.notes at most 100 {
            ref note = ^books[id].notes[pos] else { continue }
            total += pos
            delete note
        } on more {
            total = total + 1000
        }
    }
    return total
}

pub fn sumAllNotesViaReference(): int {
    var total = 0
    for id in ^books at most 100 {
        ref book = ^books[id] else { continue }
        for pos in book.notes at most 100 {
            total += pos
        } on more {
            total = total + 1000
        }
    } on more {
        total = total + 100000
    }
    return total
}

pub fn sumMarksViaReference(s: string, c: string): int {
    var total = 0
    ref g = ^grades[s, c] else { return 0 }
    for slot in g.marks at most 100 {
        total += slot
    } on more {
        total = total + 1000
    }
    return total
}

pub fn sumNotesViaIdReference(id: int): int {
    var total = 0
    ref b = ^books[Id(^books, id)] else { return 0 }
    for pos in b.notes at most 100 {
        total += pos
    } on more {
        total = total + 1000
    }
    return total
}

pub fn sumNotesViaInlineId(id: int): int {
    var total = 0
    for pos in ^books[Id(^books, id)].notes at most 100 {
        total += pos
    } on more {
        total = total + 1000
    }
    return total
}

pub fn clearNotesViaInlineId(id: int): int {
    var total = 0
    transaction {
        for pos in ^books[Id(^books, id)].notes at most 100 {
            ref note = ^books[Id(^books, id)].notes[pos] else { continue }
            total += pos
            delete note
        } on more {
            total = total + 1000
        }
    }
    return total
}
"#;

fn seed_notes(session: &mut Session) {
    for id in [1i64, 2, 3] {
        session.call("putBook", vec![Value::Int(id), Value::Text("t".into())]);
    }
    for pos in [10i64, 20] {
        session.call(
            "putNote",
            vec![Value::Int(1), Value::Int(pos), Value::Text("n".into())],
        );
    }
}

#[test]
fn a_root_reference_is_a_branch_traversal_base() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed_notes(&mut session);

    // `ref b = ^books[1] else { return 0 }; for pos in b.notes` folds book 1's notes {10,20} = 30; the
    // layer is exhausted, so `on more` does not run.
    assert_eq!(
        session.call("sumNotesViaReference", vec![Value::Int(1)]),
        Some(Value::Int(30))
    );
    // Book 2 has no notes: the branch under the reference is empty.
    assert_eq!(
        session.call("sumNotesViaReference", vec![Value::Int(2)]),
        Some(Value::Int(0))
    );
}

#[test]
fn a_reference_base_carries_the_on_more_overflow_arm() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    session.call("putBook", vec![Value::Int(1), Value::Text("t".into())]);
    for pos in [10i64, 20, 30] {
        session.call(
            "putNote",
            vec![Value::Int(1), Value::Int(pos), Value::Text("n".into())],
        );
    }

    // `at most 2` over three notes freezes {10,20} = 30, and a third key existed so the
    // `on more` arm through the reference base adds 1000.
    assert_eq!(
        session.call("sumNotesViaReferenceFirst2", vec![Value::Int(1)]),
        Some(Value::Int(1030))
    );
}

#[test]
fn an_identity_keyed_reference_base_is_a_branch_traversal_base() {
    // `ref b = ^books[Id(^books, id)] else { return 0 }` binds the root through an entry identity, spread
    // into the root's key columns at the binding; `for pos in b.notes` then traverses the
    // branch beneath it. The captured slot carries its root as a typed identity column that
    // the traversal ancestor pop re-proves, so the round trip runs end to end.
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed_notes(&mut session);

    // Book 1's notes {10, 20} = 30; the layer is exhausted, so `on more` does not run.
    assert_eq!(
        session.call("sumNotesViaIdReference", vec![Value::Int(1)]),
        Some(Value::Int(30))
    );
    // Book 2 has no notes: the branch under the identity-keyed reference is empty.
    assert_eq!(
        session.call("sumNotesViaIdReference", vec![Value::Int(2)]),
        Some(Value::Int(0))
    );
}

#[test]
fn an_inline_identity_parent_is_a_branch_traversal_base() {
    // The inline sibling `for pos in ^books[Id(^books, id)].notes`: the one identity operand
    // is the branch traversal's ancestor key-path, spread into the root's key columns at emit
    // and left as a typed identity column the ancestor pop re-proves.
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed_notes(&mut session);

    assert_eq!(
        session.call("sumNotesViaInlineId", vec![Value::Int(1)]),
        Some(Value::Int(30))
    );
}

#[test]
fn an_inline_identity_parent_key_only_deletes_through_the_reference() {
    // Both the traversal and each checked reference project the identity operand
    // into the ancestor key columns before deleting the selected note.
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed_notes(&mut session);

    // Deleting book 1's notes {10, 20} folds 30; a re-count then reads an empty branch.
    assert_eq!(
        session.call("clearNotesViaInlineId", vec![Value::Int(1)]),
        Some(Value::Int(30))
    );
    assert_eq!(
        session.call("sumNotesViaInlineId", vec![Value::Int(1)]),
        Some(Value::Int(0))
    );
}

#[test]
fn a_per_iteration_reference_is_an_inner_traversal_base() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed_notes(&mut session);
    for pos in [40i64, 50] {
        session.call(
            "putNote",
            vec![Value::Int(2), Value::Int(pos), Value::Text("n".into())],
        );
    }

    // A checked `book` reference inside the outer key traversal is the
    // inner traversal base. Book 1 notes {10,20}=30, book 2 notes {40,50}=90, book 3 none;
    // total 120, no inner or outer `on more`.
    assert_eq!(
        session.call("sumAllNotesViaReference", vec![]),
        Some(Value::Int(120))
    );
}

#[test]
fn a_key_only_reference_base_deletes_through_the_reference() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    seed_notes(&mut session);

    // Each note reference combines the parent's captured key with the frozen note key.
    // Both notes contribute to the sum before erasure; the bound is not exceeded.
    assert_eq!(
        session.call("clearNotesViaReference", vec![Value::Int(1)]),
        Some(Value::Int(30))
    );
    // The deletes committed: a re-run visits nothing.
    assert_eq!(
        session.call("clearNotesViaReference", vec![Value::Int(1)]),
        Some(Value::Int(0))
    );
    // The reading reference traversal agrees the notes are gone.
    assert_eq!(
        session.call("sumNotesViaReference", vec![Value::Int(1)]),
        Some(Value::Int(0))
    );
}

#[test]
fn a_composite_root_reference_locates_a_branch_by_its_whole_key_path() {
    let mut session = Project::single(SOURCE).ids(IDS).session();
    session.call(
        "putGrade",
        vec![
            Value::Text("amy".into()),
            Value::Text("cs".into()),
            Value::Text("fall".into()),
        ],
    );
    for slot in [3i64, 4] {
        session.call(
            "putMark",
            vec![
                Value::Text("amy".into()),
                Value::Text("cs".into()),
                Value::Int(slot),
                Value::Int(0),
            ],
        );
    }
    // A different (student, course) carries its own mark that must not leak into amy/cs.
    session.call(
        "putGrade",
        vec![
            Value::Text("bob".into()),
            Value::Text("cs".into()),
            Value::Text("fall".into()),
        ],
    );
    session.call(
        "putMark",
        vec![
            Value::Text("bob".into()),
            Value::Text("cs".into()),
            Value::Int(9),
            Value::Int(0),
        ],
    );

    // the reference over `^grades[amy, cs]`: both composite key columns are
    // captured at the binding and locate the branch under amy/cs — marks {3,4}=7, scoped to
    // that parent, never bob's slot 9.
    assert_eq!(
        session.call(
            "sumMarksViaReference",
            vec![Value::Text("amy".into()), Value::Text("cs".into())]
        ),
        Some(Value::Int(7))
    );
    assert_eq!(
        session.call(
            "sumMarksViaReference",
            vec![Value::Text("bob".into()), Value::Text("cs".into())]
        ),
        Some(Value::Int(9))
    );
}
