mod support;

use marrow_check::check_project;

use support::{config, temp_project, write};

// --- Key/identity argument typing ---

/// Two keyed resources whose identities are byte-identical (`Id(^books)` and
/// `Id(^magazines)` are both single-`int`) but nominally distinct. Used by the
/// cross-resource key-typing tests below.
const TWO_BOOKISH_RESOURCES: &str = "module shelf::lib\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     resource Magazine\n\
     \x20   required title: string\n\
     store ^magazines(id: int): Magazine\n";

/// A string passed into an `int` keyspace — `^books("oops")` where `books` is
/// keyed by `id: int` — is rejected as `check.key_type`.
#[test]
fn string_key_into_int_keyspace_is_flagged() {
    let root = temp_project("program-key-string", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn f(): string\n\
             \x20   return ^books(\"oops\").title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A cross-resource read end-to-end: addressing `^books` with a `Id(^magazines)`
/// splices a foreign identity into the book keyspace. The identity is single-`int`
/// like a book's, so the raw key shape matches, but the nominal resource does not,
/// and it is rejected as `check.key_type`.
#[test]
fn cross_resource_key_identity_is_flagged() {
    let root = temp_project("program-key-cross-resource", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(m: Id(^magazines)): string\n\
                 \x20   return ^books(m).title\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Addressing `^books` with its own `Id(^books)` is well-typed — the splice check
/// accepts the matching nominal identity — and reports nothing.
#[test]
fn same_store_key_identity_checks_clean() {
    let root = temp_project("program-key-same-store", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(b: Id(^books)): string\n\
                 \x20   if const title = ^books(b).title\n\
                 \x20       return title\n\
                 \x20   return \"\"\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `unknown` is not `any` at saved identity boundaries: a dynamic value cannot
/// reenter a keyed root until it has been converted to the declared key type.
#[test]
fn unknown_key_reentry_is_rejected() {
    let root = temp_project("program-key-cross-module", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn f(k: unknown): string\n\
             \x20   return ^books(k).title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn explicit_identity_constructor_typechecks_against_store_keys() {
    let root = temp_project("program-id-constructor", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: string): Book\n\
             fn f(): Id(^books)\n\
             \x20   return Id(^books, \"book-17\")\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        program.facts.presence_proofs().is_empty(),
        "{:#?}",
        program.facts.presence_proofs()
    );
}

#[test]
fn explicit_identity_constructor_rejects_wrong_key_shape() {
    let root = temp_project("program-id-constructor-shape", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Enrollment\n\
             \x20   required grade: string\n\
             store ^enrollments(student: string, course: string): Enrollment\n\
             fn f(): Id(^enrollments)\n\
             \x20   return Id(^enrollments, \"student-1\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn explicit_identity_constructor_rejects_unknown_key_arguments() {
    let root = temp_project("program-id-constructor-unknown", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn f(raw: unknown): Id(^books)\n\
             \x20   return Id(^books, raw)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn explicit_identity_constructor_rejects_singleton_roots() {
    let root = temp_project("program-id-constructor-singleton", |root| {
        write(
            root,
            "src/shelf/settings.mw",
            "module shelf::settings\n\
             resource Settings\n\
             \x20   required theme: string\n\
             store ^settings: Settings\n\
             fn f(): unknown\n\
             \x20   return Id(^settings)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A cross-module *qualified* identity spliced into a keyed root defers rather
/// than false-positives. The root's resource name is bare (`Book`), while an
/// identity imported from another module keeps its `shelf::lib::Book`
/// qualification, so the two cannot be matched nominally without the unified type
/// IR. Splicing the imported identity into its own keyspace is valid and must be
/// left to the runtime key guard, not rejected here.
#[test]
fn cross_module_qualified_identity_splice_defers() {
    let root = temp_project("program-key-cross-module-splice", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            "module shelf::lib\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n",
        );
        write(
            root,
            "src/app/main.mw",
            "module app::main\n\
             use shelf::lib\n\
             fn read(b: Id(^books)): string\n\
             \x20   return ^books(b).title\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.key_type"),
        "{:#?}",
        report.diagnostics
    );
}
