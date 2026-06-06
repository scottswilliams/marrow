mod support;

use marrow_check::{DiagnosticPayload, check_project};

use support::{assert_clean, config, temp_project, with_code, write};

/// Check a two-module project (`src/aaa.mw` + `src/zzz.mw`), returning the whole
/// report. The two modules let a call in `zzz` be resolved against `zzz`'s own
/// declarations, `aaa`'s declarations, and any imports — exercising the
/// module-aware resolver across a real module boundary.
fn check_two_modules(name: &str, aaa: &str, zzz: &str) -> marrow_check::CheckReport {
    let root = temp_project(name, |root| {
        write(root, "src/aaa.mw", aaa);
        write(root, "src/zzz.mw", zzz);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    report
}

#[test]
fn bare_call_resolves_in_own_module_not_a_foreign_one() {
    // Two modules each declare `fn greet`. `zzz::run` calls a bare `greet()`: a
    // bare name resolves in its own module first, so it must reach `zzz::greet`
    // and check clean — never a foreign `aaa::greet`.
    let report = check_two_modules(
        "resolve-bare-own-module",
        "module aaa\npub fn greet(): int\n    return 1\n",
        "module zzz\nfn greet(): int\n    return 2\nfn run(): int\n    return greet()\n",
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty()
            && with_code(&report, "check.private_function").is_empty(),
        "a bare call to a same-module function must resolve clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_bare_call_is_unresolved_not_first_match() {
    // `aaa` declares `pub fn greet`; `zzz` declares no `greet` and calls a bare
    // `greet()`. Imports bring module names, not bare names, so a cross-module
    // function is only reachable as `aaa::greet`. The bare call must be
    // `check.unresolved_call` — not silently first-matched to `aaa::greet`.
    let report = check_two_modules(
        "resolve-cross-bare-unresolved",
        "module aaa\npub fn greet(): int\n    return 1\n",
        "module zzz\nfn run(): int\n    return greet()\n",
    );
    assert_eq!(
        with_code(&report, "check.unresolved_call").len(),
        1,
        "a bare cross-module call must be unresolved, not first-matched: {:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_call_to_a_private_fn_is_a_visibility_error() {
    // `aaa` declares a module-private `fn secret`; `zzz` qualifies it as
    // `aaa::secret()`. The function exists but is not `pub`, so a cross-module
    // call is a distinct visibility error (`check.private_function`), not a plain
    // unresolved call — the name resolves, the visibility does not.
    let report = check_two_modules(
        "resolve-cross-private",
        "module aaa\nfn secret(): int\n    return 1\n",
        "module zzz\nfn run(): int\n    return aaa::secret()\n",
    );
    assert_eq!(
        with_code(&report, "check.private_function").len(),
        1,
        "a cross-module call to a non-pub function is a visibility error: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "a private function resolves by name, so it is not also unresolved: {:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_use_of_a_private_enum_is_a_visibility_error() {
    let root = temp_project("cross-private-enum", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             enum Hidden\n    one\n    two\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             fn f(): a::Hidden\n    return a::Hidden::one\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.private_enum");
    assert_eq!(found.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        found
            .iter()
            .all(|diagnostic| diagnostic.payload
                == DiagnosticPayload::PrivateEnum("a::Hidden".into())),
        "{found:#?}"
    );
    assert!(
        with_code(&report, "check.unknown_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn cross_module_use_of_a_public_enum_checks_clean() {
    let root = temp_project("cross-public-enum", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             pub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\n\
             fn f(): a::Status\n    return a::Status::active\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn same_named_resources_constructor_resolves_by_module() {
    // Both modules declare a resource named `Book`. A constructor is the resource
    // NAME, which is module-scoped: a bare `Book(...)` in `zzz` constructs the
    // `zzz` resource. The call must type as a constructor (no unresolved call),
    // resolving by the calling module rather than first-matching `aaa::Book`.
    let report = check_two_modules(
        "resolve-same-named-resource",
        "module aaa\nresource Book\n    title: string\n",
        "module zzz\nresource Book\n    title: string\nfn make(): Book\n    return Book(title: \"x\")\n",
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "a bare same-module constructor must resolve, not report unresolved: {:#?}",
        report.diagnostics
    );
}

#[test]
fn bare_foreign_resource_annotation_is_unknown_not_project_wide() {
    let root = temp_project("foreign-resource-type", |root| {
        write(
            root,
            "src/a.mw",
            "module a\nresource Book\n    title: string\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             fn read(book: Book): string\n\
             \x20   return book.title\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let unknown_types = with_code(&report, "check.unknown_type");
    assert_eq!(unknown_types.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        unknown_types[0].payload,
        DiagnosticPayload::UnknownType("Book".into())
    );
}

#[test]
fn bare_call_to_a_pub_fn_in_two_modules_is_ambiguous() {
    // `aaa` and `bbb` each declare a `pub fn greet`; `zzz` declares no `greet` and
    // calls a bare `greet()`. Each is reachable only as `module::greet`, so the
    // bare name cannot pick one: a distinct `check.ambiguous_call` (qualify it),
    // not a plain unresolved call or a silent first-match to `aaa::greet`.
    let root = temp_project("resolve-ambiguous-call", |root| {
        write(
            root,
            "src/aaa.mw",
            "module aaa\npub fn greet(): int\n    return 1\n",
        );
        write(
            root,
            "src/bbb.mw",
            "module bbb\npub fn greet(): int\n    return 2\n",
        );
        write(
            root,
            "src/zzz.mw",
            "module zzz\nfn run(): int\n    return greet()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_eq!(
        with_code(&report, "check.ambiguous_call").len(),
        1,
        "a bare call to a pub fn in two modules must be ambiguous: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "an ambiguous call has candidates, so it is not also unresolved: {:#?}",
        report.diagnostics
    );
}
