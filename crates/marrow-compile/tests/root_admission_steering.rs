//! Root-admission steering (E07-M2A): a reference to a store root whose durable identity
//! failed admission is steered to the `check.durable_identity` reports, not reported as a
//! bare unknown name. The ledger confound: an identity-less root drops from the durable
//! registry, so a `^root` reference — even from another module — read as `not in scope`,
//! misdirecting toward a typo. A genuinely undeclared root keeps the plain not-in-scope
//! message.

use marrow_compile::{CompileFailure, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// Capture a multi-file project with no `.marrow/ids` ledger, so every durable identity is
/// missing and any declared store fails admission.
fn project(files: &[(&str, &str)]) -> ProjectInput {
    project_with(files, None)
}

/// Capture a project against an explicit partial ledger, so some declared stores are
/// admitted and others fail admission.
fn project_with(files: &[(&str, &str)], ids: Option<&str>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(
        &manifest,
        captured,
        ids.map(str::as_bytes),
        &CaptureLimits::DEFAULT,
    )
    .expect("capture project")
}

fn diagnostics(project: &ProjectInput) -> Vec<marrow_compile::SourceDiagnostic> {
    match compile(project) {
        Ok(compiled) => panic!("expected an admission failure, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

const STORE_MODULE: &str = "module main\n\n\
     resource Member {\n\
     \x20   required email: string\n\
     }\n\n\
     store ^members[id: int]: Member\n";

/// The two-module confound: `^members` is declared in `main` but its identity fails
/// admission (no ledger), so it drops from the registry. A reference from another module
/// names the admission failure and points at the identity reports — never a bare
/// not-in-scope error, which would misdirect toward a typo.
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
        .find(|d| d.file().as_str() == "src/report.mw" && d.code() == "check.type")
        .unwrap_or_else(|| panic!("expected a reference-site diagnostic, got {diagnostics:#?}"));
    assert_eq!(
        steering.message(),
        "`members` was declared but failed identity admission; see the \
         `check.durable_identity` reports",
        "the reference site names the admission failure, not a bare unknown name",
    );
    assert!(
        !steering.message().contains("is not in scope"),
        "an admission-failed root must not read as an unknown name: {}",
        steering.message(),
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == "check.durable_identity"),
        "the primary identity gaps are still reported: {diagnostics:#?}",
    );
}

/// A single-module reference reproduces the same steering: the confound was never
/// cross-module (roots are project-wide); it was an identity-less root dropping from the
/// registry and reading as an unknown name in its own module too.
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
            .any(|d| d.code() == "check.type" && d.message().contains("failed identity admission")),
        "the declaring module's own reference is steered too: {diagnostics:#?}",
    );
}

/// A genuinely undeclared root keeps the plain not-in-scope message: the steering fires
/// only for a declared root that failed admission, never for a typo.
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
        diagnostics
            .iter()
            .any(|d| d.code() == "check.type" && d.message() == "`ghosts` is not in scope"),
        "an undeclared root is a plain unknown name: {diagnostics:#?}",
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| !d.message().contains("`ghosts` was declared")),
        "an undeclared root never claims to have been declared: {diagnostics:#?}",
    );
}

/// The reference steer fires once per dropped root across the whole compile, even when
/// one reference sits in a generic function's once-checked template body (proved before
/// the monomorphic bodies) and another in an ordinary function. The template proof shares
/// the compile-wide steered-root set, so a root referenced from both does not steer twice.
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
            .filter(
                |d| d.code() == "check.type" && d.message().contains("failed identity admission")
            )
            .count(),
        1,
        "one steer per dropped root, not one per reference site: {diagnostics:#?}",
    );
}

/// A ledger that admits `^b` over `Book` but has no identity for `^a`, so one of the two
/// stores projecting that Product is refused and the other stands.
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

/// A refused store does not carry its cause to a Product a sibling store admits.
///
/// `Book.notes(…)` is a Product declaration question: it builds the branch's materialized
/// entry record and addresses no store root. Resolving it through the *first* store
/// binding the resource sent every use of that constructor to `^a`'s identity failure,
/// even where the write it supplies is a write to the perfectly admitted `^b`. The steer
/// belongs to `^a`'s own references.
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
            .all(|d| d.code() == "check.durable_identity"),
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
            .any(|d| d.code() == "check.type" && d.message().contains("failed identity admission")),
        "a use of the refused store is steered to its cause: {diagnostics:#?}",
    );
}

/// A ledger admitting one keyless store `^solo` over `Book`. A keyless (singleton) root
/// carries a complete durable identity and is admitted, but it is outside the executable
/// subset, so its `RootBinding` is `NotYetExecutable`.
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

/// `Book.notes` is a keyed branch, not a projectable field of `Book`'s materialized
/// whole-entry record. Naming it as a field must be steered to the durable-path form.
///
/// Whether `Book` declares a branch `notes` is a Product DECLARATION question. It is
/// answered here from the branch-record table, which is written only at a Product's first
/// *executable* root, and the steer additionally requires `ProductBinding::Declared` —
/// an executable-occurrence scan. A Product whose only store is keyless is admitted with a
/// complete identity and a complete declared branch tree, yet answers `NotYetExecutable`,
/// so the same source question degrades to the bare missing-field report.
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
    let keyed_message = keyed
        .iter()
        .find(|d| d.code() == "check.type")
        .unwrap_or_else(|| panic!("expected a check.type report, got {keyed:#?}"))
        .message()
        .to_string();

    let keyless_source = format!("module main\n\n{BOOK_DECL}\nstore ^solo: Book\n{field_use}");
    let keyless = diagnostics(&project_with(
        &[("src/main.mw", &keyless_source)],
        Some(KEYLESS_IDS),
    ));
    let keyless_message = keyless
        .iter()
        .find(|d| d.code() == "check.type")
        .unwrap_or_else(|| panic!("expected a check.type report, got {keyless:#?}"))
        .message()
        .to_string();

    assert_eq!(
        keyed_message, keyless_message,
        "the branch-versus-field answer is a declaration fact: it must not depend on \
         whether some root over the Product reached the executable subset",
    );
    assert!(
        !keyless_message.contains("record has no field"),
        "a declared branch is never reported as a missing field: {keyless_message}",
    );
}
