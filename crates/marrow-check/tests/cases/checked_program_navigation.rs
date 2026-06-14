use crate::support;
use marrow_check::check_project;

use support::{config, temp_project, write};

/// `nextId(^books)` over a single-`int` root types to `Id(^books)`, so a function
/// returning it under a declared `Id(^books)` return type checks clean. (`nextId`
/// is a saved-data read, so it lives in a function body, not a module const.)
/// The local-const annotation `const id: Id(^books) = nextId(^books)` likewise
/// checks clean.
#[test]
fn next_id_types_to_the_resource_identity() {
    let root = temp_project("program-nextid-id", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn fresh(): Id(^books)\n\
             \x20   const id: Id(^books) = nextId(^books)\n\
             \x20   return id\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Enrollment\n\
             \x20   required grade: string\n\
             store ^enrollments(studentId: string, courseId: string): Enrollment\n\
             fn fresh()\n\
             \x20   const id = nextId(^enrollments)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Tag\n\
             \x20   required name: string\n\
             store ^tags(slug: string): Tag\n\
             fn fresh()\n\
             \x20   const id = nextId(^tags)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Settings\n\
             \x20   required theme: string\n\
             store ^settings: Settings\n\
             fn fresh()\n\
             \x20   const id = nextId(^settings)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn afterTitle(id: int, fallback: Id(^books)): string\n\
             \x20   return ^books(next(^books(id)) ?? fallback).title ?? \"\"\n\
             pub fn beforeTitle(id: int, fallback: Id(^books)): string\n\
             \x20   return ^books(prev(^books(id)) ?? fallback).title ?? \"\"\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn bad(id: int)\n\
             \x20   const x = next(^books(id), ^books(id))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Book\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
             pub fn nextPos(id: int, p: int): int\n\
             \x20   return next(^books(id).tags(p)) ?? -1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Enrollment\n\
             \x20   required grade: string\n\
             store ^enrollments(studentId: string, courseId: string): Enrollment\n\
             fn step(s: string, c: string)\n\
             \x20   const n = next(^enrollments(s, c))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Enrollment\n\
             \x20   required grade: string\n\
             store ^enrollments(studentId: string, courseId: string): Enrollment\n\
             fn step()\n\
             \x20   const n = next(^enrollments)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn step(id: Id(^books))\n\
             \x20   const n = next(id)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Book\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byShelf(shelf, id)\n\
             fn step(s: string)\n\
             \x20   const n = next(^books.byShelf(s))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

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
             resource Enrollment\n\
             \x20   required credits: int\n\
             store ^enrollments(studentId: string, courseId: string): Enrollment\n\
             \x20   index byStudent(studentId, courseId)\n\
             fn total(studentId: string): int\n\
             \x20   var credits = 0\n\
             \x20   for id in keys(^enrollments.byStudent(studentId))\n\
             \x20       credits = credits + (^enrollments(id).credits ?? 0)\n\
             \x20   return credits\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}
