//! A reference to a store root whose durable identity failed admission is steered to the
//! `check.durable_identity` reports, not reported as a bare unknown name.
//!
//! The ledger confound: an identity-less root drops from the durable registry, so a
//! `^root` reference — even from another module — would read as `not in scope`,
//! misdirecting toward a typo. A genuinely undeclared root keeps the plain not-in-scope
//! message.

use marrow_codes::Code;
use marrow_compile::{NameFamily, Steer, Unresolved};
use marrow_project::ProjectInput;

use super::project_capture::project_with_ids;
use super::refused as diagnostics;

/// A project with no ledger, so every declared store fails identity admission.
fn project(files: &[(&str, &str)]) -> ProjectInput {
    project_with(files, None)
}

/// A project against an explicit ledger, so admission can succeed for some stores only.
fn project_with(files: &[(&str, &str)], ids: Option<&str>) -> ProjectInput {
    project_with_ids(files, ids.map(str::as_bytes))
}

const STORE_MODULE: &str = "module main\n\n\
     resource Member {\n\
     \x20   required email: string\n\
     }\n\n\
     store ^members[id: int]: Member\n";

/// `^members` is declared in `main` but fails admission, so it drops from the registry. A
/// reference from another module must still name the admission failure.
#[test]
fn a_reference_to_an_admission_failed_root_is_steered_to_the_identity_reports() {
    let reference = "module report\n\n\
         pub fn lookup(id: int): string? {\n\
         \x20   return ^members[id].email\n\
         }\n";
    let diagnostics = diagnostics(&project(&[
        ("src/main.mw", STORE_MODULE),
        ("src/report.mw", reference),
    ]));

    let steering = diagnostics
        .iter()
        .find(|d| d.file().as_str() == "src/report.mw" && d.code() == Code::CheckType)
        .unwrap_or_else(|| panic!("expected a reference-site diagnostic, got {diagnostics:#?}"));
    assert_eq!(
        steering.message(),
        "`members` was declared but failed identity admission; see the \
         `check.durable_identity` reports",
        "the reference site names the admission failure, not a bare unknown name",
    );
    assert!(
        steering.unresolved().is_none(),
        "an admission-failed root must not read as an unknown name: {}",
        steering.message(),
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == Code::CheckDurableIdentity),
        "the primary identity gaps are still reported: {diagnostics:#?}",
    );
}

/// Roots are project-wide, so the steering does not depend on crossing a module boundary.
#[test]
fn the_steering_holds_within_the_declaring_module() {
    let source = "module main\n\n\
         resource Member {\n\
         \x20   required email: string\n\
         }\n\n\
         store ^members[id: int]: Member\n\n\
         pub fn lookup(id: int): string? {\n\
         \x20   return ^members[id].email\n\
         }\n";
    let diagnostics = diagnostics(&project(&[("src/main.mw", source)]));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == Code::CheckType
                && d.message().contains("failed identity admission")),
        "the declaring module's own reference is steered too: {diagnostics:#?}",
    );
}

/// The steering fires only for a declared root that failed admission, never for a typo.
#[test]
fn a_genuinely_undeclared_root_keeps_the_unknown_name_message() {
    let reference = "module report\n\n\
         pub fn lookup(id: int): string? {\n\
         \x20   return ^ghosts[id].email\n\
         }\n";
    let diagnostics = diagnostics(&project(&[
        ("src/main.mw", STORE_MODULE),
        ("src/report.mw", reference),
    ]));
    assert!(
        diagnostics.iter().any(|d| d.unresolved()
            == Some(&Unresolved {
                family: NameFamily::Root,
                name: "ghosts".to_string(),
            })),
        "an undeclared root is a plain unknown name: {diagnostics:#?}",
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| !d.message().contains("`ghosts` was declared")),
        "an undeclared root never claims to have been declared: {diagnostics:#?}",
    );
}

/// The steered-root set is compile-wide, so a generic function's once-checked template body
/// and an ordinary body referencing the same dropped root yield one steer, not two.
#[test]
fn a_dropped_root_referenced_from_a_generic_and_an_ordinary_function_steers_once() {
    let source = "module main\n\n\
         resource Member {\n\
         \x20   required email: string\n\
         }\n\n\
         store ^members[id: int]: Member\n\n\
         pub fn probe<T>(seed: T, id: int): T {\n\
         \x20   if exists(^members[id]) {\n\
         \x20       return seed\n\
         \x20   }\n\
         \x20   return seed\n\
         }\n\n\
         pub fn other(id: int): bool {\n\
         \x20   return exists(^members[id])\n\
         }\n";
    let diagnostics = diagnostics(&project(&[("src/main.mw", source)]));
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.code() == Code::CheckType
                && d.message().contains("failed identity admission"))
            .count(),
        1,
        "one steer per dropped root, not one per reference site: {diagnostics:#?}",
    );
}

/// Admits `^b` over `Book` but not `^a`, so one of two stores over that Product is refused.
const PARTIAL_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

const SHARED_PRODUCT_MODULE: &str = "module main\n\n\
     resource Book {\n\
     \x20   required title: string\n\
     \x20   notes[noteId: int] {\n\
     \x20       required text: string\n\
     \x20   }\n\
     }\n\n\
     store ^a[id: int]: Book\n\
     store ^b[id: int]: Book\n";

/// `Book.notes(…)` builds the branch's materialized entry record and addresses no store
/// root, so it must not resolve through whichever store happens to bind the resource first.
/// A refused store's steer belongs to that store's own references.
#[test]
fn a_refused_store_does_not_steer_a_product_its_sibling_admits() {
    let source = format!(
        "{SHARED_PRODUCT_MODULE}\n\
         pub fn addB(id: int, n: int, t: string) {{\n\
         \x20   transaction {{\n\
         \x20       ^b[id].notes[n] = Book.notes(text: t)\n\
         \x20   }}\n\
         }}\n"
    );
    let diagnostics = diagnostics(&project_with(
        &[("src/main.mw", &source)],
        Some(PARTIAL_IDS),
    ));
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code() == Code::CheckDurableIdentity),
        "only ^a's own identity gaps are reported; the constructor is not blamed for \
         them: {diagnostics:#?}",
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.message().contains("`a") || d.message().contains(" a.")),
        "every report names the refused store, not the Product: {diagnostics:#?}",
    );
}

/// The steer still reaches a use of the refused store itself.
#[test]
fn a_refused_store_still_steers_its_own_references() {
    let source = format!(
        "{SHARED_PRODUCT_MODULE}\n\
         pub fn addA(id: int, n: int, t: string) {{\n\
         \x20   transaction {{\n\
         \x20       ^a[id].notes[n] = Book.notes(text: t)\n\
         \x20   }}\n\
         }}\n"
    );
    let diagnostics = diagnostics(&project_with(
        &[("src/main.mw", &source)],
        Some(PARTIAL_IDS),
    ));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == Code::CheckType
                && d.message().contains("failed identity admission")),
        "a use of the refused store is steered to its cause: {diagnostics:#?}",
    );
}

/// Admits one keyless store `^solo` over `Book`. A singleton root carries a complete durable
/// identity yet sits outside the executable subset (`RootBinding::NotYetExecutable`).
const KEYLESS_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root solo 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     high-water 0\n\
     end\n";

/// A ledger admitting one keyed store `^kept` over the identical `Book` declaration.
const KEYED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root Book.notes 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n\
     id key Book.notes.noteId 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\n\
     id field Book.notes.text 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     id root kept 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id key kept.id 3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c\n\
     high-water 0\n\
     end\n";

const BOOK_DECL: &str = "resource Book {\n\
     \x20   required title: string\n\
     \x20   notes[noteId: int] {\n\
     \x20       required text: string\n\
     \x20   }\n\
     }\n";

/// `Book.notes` is a keyed branch, not a projectable field of `Book`'s whole-entry record,
/// and naming it as a field must be steered to the durable-path form.
///
/// Whether `Book` declares that branch is a declaration fact, so the answer must not depend
/// on the branch-record table or on any root over the Product reaching the executable
/// subset: a Product whose only store is keyless answers `NotYetExecutable` yet has a
/// complete declared branch tree.
#[test]
fn a_branch_named_as_a_field_is_steered_whether_or_not_a_root_is_executable() {
    let field_use = "\npub fn peek(): int {\n\
         \x20   const b = Book(title: \"t\")\n\
         \x20   const n = b.notes\n\
         \x20   return 0\n\
         }\n";

    let keyed_source =
        format!("module main\n\n{BOOK_DECL}\nstore ^kept[id: int]: Book\n{field_use}");
    let keyed = diagnostics(&project_with(
        &[("src/main.mw", &keyed_source)],
        Some(KEYED_IDS),
    ));
    let keyed_steer = keyed
        .iter()
        .find(|d| d.code() == Code::CheckType)
        .unwrap_or_else(|| panic!("expected a check.type report, got {keyed:#?}"))
        .steer()
        .cloned();

    let keyless_source = format!("module main\n\n{BOOK_DECL}\nstore ^solo: Book\n{field_use}");
    let keyless = diagnostics(&project_with(
        &[("src/main.mw", &keyless_source)],
        Some(KEYLESS_IDS),
    ));
    let keyless_steer = keyless
        .iter()
        .find(|d| d.code() == Code::CheckType)
        .unwrap_or_else(|| panic!("expected a check.type report, got {keyless:#?}"))
        .steer()
        .cloned();

    assert_eq!(
        keyed_steer, keyless_steer,
        "the branch-versus-field answer is a declaration fact: it must not depend on \
         whether some root over the Product reached the executable subset",
    );
    // A steer is present, so neither row fell through to the missing-field report.
    assert_eq!(
        keyed_steer,
        Some(Steer::KeyedBranch {
            branch: "notes".to_string(),
            resource: Some("Book".to_string()),
        }),
    );
}
