use crate::support;
use marrow_check::{
    CHECK_CALL_ARGUMENT, CHECK_KEY_REQUIRES_SINGLE_KEY, CHECK_NEIGHBOR_UNSUPPORTED,
    CHECK_NEXT_ID_REQUIRES_SINGLE_INT, CHECK_OPERATOR_TYPE, CHECK_UNRESOLVED_CALL,
    DiagnosticPayload, check_project,
};

use support::{check_module, check_module_report, config, temp_project, with_code, write};

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
            .any(|diagnostic| diagnostic.code == CHECK_NEXT_ID_REQUIRES_SINGLE_INT),
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
            .any(|diagnostic| diagnostic.code == CHECK_NEXT_ID_REQUIRES_SINGLE_INT),
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
            .any(|diagnostic| diagnostic.code == CHECK_NEXT_ID_REQUIRES_SINGLE_INT),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over an argument that is not a saved store root — a literal, a
/// non-identity local, or a scalar field — cannot allocate an identity, so it is
/// rejected at check with `check.call_argument` rather than checking clean and
/// faulting `write.next_id_unsupported`/`run.unsupported` at run. The diagnostic
/// reports at the call span and carries no typed payload.
#[test]
fn next_id_over_a_non_root_argument_is_flagged() {
    let root = temp_project("program-nextid-non-root", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn bad()\n\
             \x20   const x = nextId(5)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let flagged = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == CHECK_CALL_ARGUMENT)
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(flagged.span.line, 6);
    assert_eq!(flagged.span.column, 15);
    assert_eq!(flagged.payload, DiagnosticPayload::None);
}

/// `nextId` over a saved path that is not a bare store root — an index branch
/// here — addresses no allocatable identity, so it is rejected at check with
/// `check.call_argument` on shape rather than checking clean (the index branch
/// types to `unknown`) and faulting `run.unsupported` at run. The diagnostic
/// reports at the call span with no typed payload.
#[test]
fn next_id_over_an_index_branch_is_flagged() {
    let root = temp_project("program-nextid-index-branch", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byShelf(shelf, id)\n\
             fn bad()\n\
             \x20   const x = nextId(^books.byShelf(\"fiction\"))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let flagged = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == CHECK_CALL_ARGUMENT)
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(flagged.span.line, 8);
    assert_eq!(flagged.span.column, 15);
    assert_eq!(flagged.payload, DiagnosticPayload::None);
}

/// A concrete non-identity argument to `key` — a literal, a non-identity local,
/// or a scalar field — has no store identity to project, so it is rejected at
/// check with `check.call_argument` rather than checking clean and faulting
/// `run.type` at run. The diagnostic reports at the call span with no payload.
#[test]
fn key_over_a_non_identity_argument_is_flagged() {
    let root = temp_project("program-key-non-identity", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn bad()\n\
             \x20   const x = key(\"hi\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let flagged = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == CHECK_CALL_ARGUMENT)
        .unwrap_or_else(|| panic!("{:#?}", report.diagnostics));
    assert_eq!(flagged.span.line, 6);
    assert_eq!(flagged.span.column, 15);
    assert_eq!(flagged.payload, DiagnosticPayload::None);
}

// --- Ordered navigation: reversed / next / prev ---

/// `reversed`, `next`, and `prev` are builtins, so they never report
/// `check.unresolved_call`. `reversed` is type-transparent over a value view: it
/// yields the same element type as its argument, so `for w in
/// reversed(values(std::text::split(...)))` binds `w` to `string` — and misusing
/// it (`w + 1`, a string plus an int) is flagged. If `reversed` regressed the
/// element type to `Unknown`, this misuse would pass silently, so the diagnostic
/// proves the element type survives the wrapper.
#[test]
fn reversed_preserves_the_sequence_element_type() {
    let root = temp_project("program-reversed-transparent", |root| {
        write(
            root,
            "src/shelf/words.mw",
            "module shelf::words\n\
             fn shout()\n\
             \x20   for w in reversed(values(std::text::split(\"a,b,c\", \",\")))\n\
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
            .any(|d| d.code == CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == CHECK_UNRESOLVED_CALL),
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
             \x20   return counts(today, \"open\") ?? -1\n\
             fn seqIndex(): int\n\
             \x20   var xs: sequence[int]\n\
             \x20   xs(1) = 10\n\
             \x20   return xs(1) ?? -1\n",
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
            .any(|diagnostic| diagnostic.code == CHECK_CALL_ARGUMENT),
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
            .any(|diagnostic| diagnostic.code == CHECK_NEIGHBOR_UNSUPPORTED),
        "{:#?}",
        report.diagnostics
    );
}

/// A rejected `next` over a composite-identity record is the one error. Reading its
/// result must not cascade a second diagnostic: the unsupported neighbor poisons its
/// result type, so no `check.untyped_value` or `check.bare_maybe_present_read` stacks.
#[test]
fn rejected_composite_neighbor_does_not_cascade() {
    let root = temp_project("program-next-composite-no-cascade", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment\n\
             \x20   required grade: string\n\
             store ^enrollments(studentId: string, courseId: string): Enrollment\n\
             fn step(s: string, c: string): string\n\
             \x20   const n = next(^enrollments(s, c))\n\
             \x20   return n.grade\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert_eq!(
        with_code(&report, CHECK_NEIGHBOR_UNSUPPORTED).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.bare_maybe_present_read").is_empty(),
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
            .any(|diagnostic| diagnostic.code == CHECK_NEIGHBOR_UNSUPPORTED),
        "{:#?}",
        report.diagnostics
    );
}

/// The composite store root is traversed whole, with no single key level to seek,
/// so its `next`/`prev` remedy must name the working traversal — `for id in ^r` or
/// `reversed(^r)` — not "scope a single key level", which only applies to a
/// composite child layer.
#[test]
fn neighbor_over_a_composite_store_root_names_the_iteration_remedy() {
    for which in ["next", "prev"] {
        let root = temp_project(&format!("program-{which}-composite-root-remedy"), |root| {
            write(
                root,
                "src/shelf/enroll.mw",
                &format!(
                    "module shelf::enroll\n\
                     resource Enrollment\n\
                     \x20   required grade: string\n\
                     store ^enrollments(studentId: string, courseId: string): Enrollment\n\
                     fn step()\n\
                     \x20   const n = {which}(^enrollments)\n"
                ),
            );
        });
        let (report, _) = check_project(&root, &config()).expect("check");

        let diagnostics = with_code(&report, CHECK_NEIGHBOR_UNSUPPORTED);
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("{:#?}", report.diagnostics);
        };
        assert!(
            diagnostic.message.contains("for id in ^enrollments")
                && diagnostic.message.contains("reversed"),
            "{}",
            diagnostic.message
        );
        assert!(
            !diagnostic.message.contains("scope a single key level"),
            "{}",
            diagnostic.message
        );
    }
}

/// A fully-keyed composite record is a single identity, so its `next`/`prev` remedy
/// stays "scope a single key level" — descend to a single-key child layer to seek.
#[test]
fn neighbor_over_a_composite_record_keeps_the_key_level_remedy() {
    let root = temp_project("program-next-composite-record-remedy", |root| {
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

    let diagnostics = with_code(&report, CHECK_NEIGHBOR_UNSUPPORTED);
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("{:#?}", report.diagnostics);
    };
    assert!(
        diagnostic.message.contains("scope a single key level"),
        "{}",
        diagnostic.message
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
            .any(|diagnostic| diagnostic.code == CHECK_NEIGHBOR_UNSUPPORTED),
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
            .any(|diagnostic| diagnostic.code == CHECK_NEIGHBOR_UNSUPPORTED),
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

/// A field read, `for`, `count`, and `reversed` over a value the checker knows is a
/// scalar can never succeed, so each is a check error rather than a deferred runtime
/// fault. The `unknown` a field-of-scalar once produced even leaked through
/// arithmetic, so `n.bogus + 1` must not type clean either.
#[test]
fn navigating_a_statically_known_scalar_is_a_check_error() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "scalar-field",
            "fn f()\n    const n: int = 5\n    const x = n.bogus\n",
            "check.unknown_field",
        ),
        (
            "scalar-optional-field",
            "fn f()\n    const n: int = 5\n    const x = n?.bogus\n",
            "check.unknown_field",
        ),
        (
            "scalar-field-in-arithmetic",
            "fn f()\n    const n: int = 5\n    const x = n.bogus + 1\n",
            "check.unknown_field",
        ),
        (
            "for-over-scalar",
            "fn f()\n    const n: int = 5\n    for x in n\n        print(x)\n",
            "check.collection_unsupported",
        ),
        (
            "count-over-scalar",
            "fn f()\n    const n: int = 5\n    const c = count(n)\n",
            "check.collection_unsupported",
        ),
        (
            "reversed-over-scalar",
            "fn f()\n    const n: int = 5\n    const r = reversed(n)\n",
            "check.collection_unsupported",
        ),
    ];
    for (name, src, code) in cases {
        let found = check_module(name, &format!("module m\n{src}"), code);
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

/// A `for` over a combinator whose inner expression is itself rejected reports the
/// error once, at the inner root cause, not once per enclosing combinator. The `for`
/// loop must not re-report an iterable its own subexpression already flagged.
#[test]
fn a_for_over_a_rejected_combinator_reports_one_error() {
    let header = "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n";
    let cases: &[(&str, &str)] = &[
        (
            "for-reversed-scalar",
            "fn f()\n    const n: int = 5\n    for x in reversed(n)\n        print(x)\n",
        ),
        (
            "for-count-scalar",
            "fn f()\n    const n: int = 5\n    for x in count(n)\n        print(x)\n",
        ),
        (
            "for-triple-nested-reversed",
            "fn f()\n    for id in reversed(reversed(reversed(^books)))\n        print(id)\n",
        ),
        // `count(^books)` is a valid call returning an int, so the combinator emits no
        // error and the for-scalar rule must still flag the non-iterable int once —
        // the suppression defers only to a real prior diagnostic.
        (
            "for-count-of-saved-layer",
            "fn f()\n    for x in count(^books)\n        print(x)\n",
        ),
    ];
    for (name, src) in cases {
        let report = check_module_report(name, &format!("{header}{src}"));
        assert_eq!(
            with_code(&report, "check.collection_unsupported").len(),
            1,
            "{name}: one collection error per root cause\n{:#?}",
            report.diagnostics,
        );
    }
}

/// The scalar split must not false-positive on a genuinely-unknown base. A local
/// collection's `count` result and a `values(...)` materialization are typed
/// `unknown`; a field read off either defers rather than firing `check.unknown_field`.
#[test]
fn navigating_an_unknown_typed_value_still_defers() {
    let report = check_module_report(
        "unknown-base-defers",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    const v = values(^books)\n    const x = v.field\n",
    );
    assert!(
        with_code(&report, "check.unknown_field").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

// --- key(id): project a single-key identity to its scalar key ---

/// `key(id)` over a single-scalar-key store types to that key's scalar. The result
/// type is proven by misuse: a string key plus an int is a `check.operator_type`
/// error, so if `key` regressed to `Unknown` the misuse would pass silently.
#[test]
fn key_of_a_string_keyed_identity_types_the_key_scalar() {
    let root = temp_project("program-key-string", |root| {
        write(
            root,
            "src/shelf/tags.mw",
            "module shelf::tags\n\
             resource Tag\n\
             \x20   required label: string\n\
             store ^tags(slug: string): Tag\n\
             fn bad(t: Id(^tags))\n\
             \x20   const x = key(t) + 1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_CALL),
        "key must be a recognized builtin: {:#?}",
        report.diagnostics
    );
}

/// Used at its declared key scalar, `key(id)` checks clean across every saved-key
/// scalar — int, bool, date, and bytes keys all project to their own type.
#[test]
fn key_projects_every_scalar_key_type_cleanly() {
    let cases: &[(&str, &str, &str)] = &[
        ("int", "int", "n + 1"),
        ("date", "date", "d"),
        ("bytes", "bytes", "b"),
    ];
    for (name, key_ty, use_expr) in cases {
        let root = temp_project(&format!("program-key-{name}"), |root| {
            write(
                root,
                "src/shelf/things.mw",
                &format!(
                    "module shelf::things\n\
                     resource Thing\n\
                     \x20   required label: string\n\
                     store ^things(k: {key_ty}): Thing\n\
                     pub fn project(t: Id(^things)): {key_ty}\n\
                     \x20   const n: {key_ty} = key(t)\n\
                     \x20   const d: {key_ty} = key(t)\n\
                     \x20   const b: {key_ty} = key(t)\n\
                     \x20   return {use_expr}\n"
                ),
            );
        });
        let (report, _) = check_project(&root, &config()).expect("check");
        assert!(!report.has_errors(), "{name}: {:#?}", report.diagnostics);
    }
}

/// A bool key projects to `bool`: returning it under a `bool` return type checks
/// clean, while the same value plus an int is an operator-type error.
#[test]
fn key_of_a_bool_keyed_identity_types_bool() {
    let root = temp_project("program-key-bool", |root| {
        write(
            root,
            "src/shelf/flags.mw",
            "module shelf::flags\n\
             resource Flag\n\
             \x20   required note: string\n\
             store ^flags(on: bool): Flag\n\
             pub fn project(f: Id(^flags)): bool\n\
             \x20   return key(f)\n\
             fn bad(f: Id(^flags))\n\
             \x20   const x = key(f) + 1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
}

/// `key(id)` over a composite multi-key identity has no single scalar to project,
/// so it is rejected with `check.key_requires_single_key` rather than leaking a
/// partial key or mis-typing the result.
#[test]
fn key_of_a_composite_identity_is_flagged() {
    let root = temp_project("program-key-composite", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment\n\
             \x20   required grade: string\n\
             store ^enrollments(studentId: string, courseId: string): Enrollment\n\
             fn project(e: Id(^enrollments))\n\
             \x20   const k = key(e)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_KEY_REQUIRES_SINGLE_KEY),
        "{:#?}",
        report.diagnostics
    );
}

/// `key` takes exactly one argument; a zero- or two-argument call reports the
/// standard `check.call_argument` arity diagnostic.
#[test]
fn key_with_wrong_arity_is_flagged() {
    let root = temp_project("program-key-arity", |root| {
        write(
            root,
            "src/shelf/tags.mw",
            "module shelf::tags\n\
             resource Tag\n\
             \x20   required label: string\n\
             store ^tags(slug: string): Tag\n\
             fn bad(t: Id(^tags))\n\
             \x20   const x = key(t, t)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CALL_ARGUMENT),
        "{:#?}",
        report.diagnostics
    );
}
