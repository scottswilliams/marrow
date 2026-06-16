use crate::support;
use marrow_check::{CHECK_KEY_TYPE, check_project};

use support::{config, temp_project, with_code, write};

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
            .any(|diagnostic| diagnostic.code == CHECK_KEY_TYPE),
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
            .any(|diagnostic| diagnostic.code == CHECK_KEY_TYPE),
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
            .any(|diagnostic| diagnostic.code == CHECK_KEY_TYPE),
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
            .any(|diagnostic| diagnostic.code == CHECK_KEY_TYPE),
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
            .any(|diagnostic| diagnostic.code == CHECK_KEY_TYPE),
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
            .any(|diagnostic| diagnostic.code == CHECK_KEY_TYPE),
        "{:#?}",
        report.diagnostics
    );
}

/// A cross-module identity imported with its owning root splices cleanly into
/// that root. The identity and saved root compare by the resolved store, not by
/// local spelling at the use site.
#[test]
fn cross_module_imported_identity_splice_checks_clean() {
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
             \x20   if const title = ^books(b).title\n\
             \x20       return title\n\
             \x20   return \"\"\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A same-shaped identity from another imported root remains nominally foreign.
/// `Id(^magazines)` cannot address `^books` even though both stores are keyed by
/// a single `int`.
#[test]
fn cross_module_imported_wrong_store_identity_splice_is_flagged() {
    let root = temp_project("program-key-cross-module-wrong-store-splice", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n",
        );
        write(
            root,
            "src/shelf/magazines.mw",
            "module shelf::magazines\n\
             resource Magazine\n\
             \x20   required title: string\n\
             store ^magazines(id: int): Magazine\n",
        );
        write(
            root,
            "src/app/main.mw",
            "module app::main\n\
             use shelf::books\n\
             use shelf::magazines\n\
             fn read(m: Id(^magazines)): string\n\
             \x20   if const title = ^books(m).title\n\
             \x20       return title\n\
             \x20   return \"\"\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let key_type = with_code(&report, CHECK_KEY_TYPE).len();
    assert_eq!(
        key_type, 1,
        "expected one key-type diagnostic: {:#?}",
        report.diagnostics
    );
}
