use crate::support;
use marrow_check::check_project;

use support::{config, temp_project, with_code, write};

// --- Nominal identity typing ---

/// Two keyed resources whose identities are byte-identical (`Id(^books)` and
/// `Id(^magazines)` are both single-`int`) but nominally distinct. Used by the
/// nominal-identity tests below.
const TWO_BOOKISH_RESOURCES: &str = "module shelf::lib\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     resource Magazine\n\
     \x20   required title: string\n\
     store ^magazines(id: int): Magazine\n";

/// Passing a `Id(^magazines)` where a function parameter expects `Id(^books)` is a
/// nominal mismatch: the identities share a key shape but name different
/// store roots, so the call is rejected as `check.call_argument`.
#[test]
fn wrong_store_identity_argument_is_flagged() {
    let root = temp_project("program-id-arg", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn takes(b: Id(^books))\n\
                 \x20   return\n\
                 fn f(m: Id(^magazines))\n\
                 \x20   takes(m)\n"
            ),
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

/// Returning a `Id(^magazines)` from a function declared to return `Id(^books)` is a
/// nominal mismatch reported as `check.return_type`.
#[test]
fn wrong_store_identity_return_is_flagged() {
    let root = temp_project("program-id-return", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(m: Id(^magazines)): Id(^books)\n\
                 \x20   return m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.return_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Storing a `Id(^magazines)` into a `Id(^books)` place is a nominal mismatch
/// reported as `check.assignment_type`.
#[test]
fn wrong_store_identity_assignment_is_flagged() {
    let root = temp_project("program-id-assign", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn f(m: Id(^magazines))\n\
                 \x20   var b: Id(^books) = m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.assignment_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A raw scalar where an identity is expected, and an identity where a scalar is
/// expected, are both flagged as `check.call_argument`: identity and scalar are
/// distinct nominal types, not freely interchangeable.
#[test]
fn scalar_and_identity_are_not_interchangeable_arguments() {
    let root = temp_project("program-id-scalar-swap", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn takesId(b: Id(^books))\n\
                 \x20   return\n\
                 fn takesInt(n: int)\n\
                 \x20   return\n\
                 fn f(b: Id(^books))\n\
                 \x20   takesId(1)\n\
                 \x20   takesInt(b)\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let count = with_code(&report, "check.call_argument").len();
    assert!(count >= 2, "{:#?}", report.diagnostics);
}

/// Same-store identity flow checks clean: passing, returning, and storing a
/// `Id(^books)` where a `Id(^books)` is expected is well-typed and reports nothing.
#[test]
fn same_store_identity_checks_clean() {
    let root = temp_project("program-id-same", |root| {
        write(
            root,
            "src/shelf/lib.mw",
            &format!(
                "{TWO_BOOKISH_RESOURCES}\
                 fn takes(b: Id(^books))\n\
                 \x20   return\n\
                 fn f(b: Id(^books)): Id(^books)\n\
                 \x20   takes(b)\n\
                 \x20   var c: Id(^books) = b\n\
                 \x20   return c\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn qualified_resource_identity_annotation_unifies_with_owner_identity() {
    let root = temp_project("program-id-qualified", |root| {
        write(
            root,
            "src/inventory.mw",
            "module inventory\n\
             resource Item\n\
             \x20   required name: string\n\
             store ^items(id: int): Item\n\
             pub fn add(name: string): Id(^items)\n\
             \x20   const id: Id(^items) = nextId(^items)\n\
             \x20   ^items(id).name = name\n\
             \x20   return id\n\
             pub fn nameOf(id: Id(^items)): string\n\
             \x20   if const name = ^items(id).name\n\
             \x20       return name\n\
             \x20   return \"\"\n",
        );
        write(
            root,
            "src/caller.mw",
            "module caller\n\
             use inventory\n\
             pub fn demo(): string\n\
             \x20   const id: Id(^items) = inventory::add(\"widget\")\n\
             \x20   return inventory::nameOf(id)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn aliased_resource_and_identity_annotations_resolve_to_the_owner() {
    let root = temp_project("program-resource-qualified", |root| {
        write(
            root,
            "src/audit/log.mw",
            "module audit::log\n\
             resource Event\n\
             \x20   required actor: string\n\
             store ^events(id: int): Event\n",
        );
        write(
            root,
            "src/audit/query.mw",
            "module audit::query\n\
             use audit::log\n\
             pub fn actor(ev: log::Event): string\n\
             \x20   const id: Id(^events) = nextId(^events)\n\
             \x20   ^events(id).actor = \"scott\"\n\
             \x20   return ev.actor\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}
