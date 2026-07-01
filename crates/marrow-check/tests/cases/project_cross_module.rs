use crate::support;
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
fn fully_qualified_call_to_an_own_module_private_fn_resolves_clean() {
    // A module may call its own module-private function by the function's own
    // fully-qualified path. `aaa::secret` is private, but `aaa::run` is itself in
    // `aaa`, so qualifying it as `aaa::secret()` resolves like a bare same-module
    // call — visibility gates only cross-module reach. No `check.private_function`,
    // and no `check.untyped_value` cascade onto the typed `return`.
    let report = check_two_modules(
        "resolve-own-module-fqn-private",
        "module aaa\nfn secret(): int\n    return 1\nfn run(): int\n    return aaa::secret()\n",
        "module zzz\n",
    );
    assert!(
        with_code(&report, "check.private_function").is_empty(),
        "a module calling its own private fn by its full path must resolve clean: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "the self-qualified private call resolves, so its result is typed: {:#?}",
        report.diagnostics
    );
    assert_clean(&report);
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
fn many_modules_resolve_a_shared_enum_and_resource_with_unchanged_diagnostics() {
    // Cross-module enum and qualified-resource resolution route through the O(1)
    // module-name index. At scale the resolution semantics must be unchanged: every
    // module qualifying the shared module's `pub enum` and `pub resource` checks
    // clean, while a single module qualifying the shared module's private enum still
    // reports `check.private_enum` for both the annotation and the value, and nothing
    // else regresses.
    const MODULE_COUNT: usize = 64;
    let root = temp_project("resolve-many-shared-types", |root| {
        write(
            root,
            "src/shared.mw",
            "module shared\n\
             pub enum Status\n    Active\n    Inactive\n\
             enum Hidden\n    One\n    Two\n\
             resource Book\n    title: string\n",
        );
        for index in 0..MODULE_COUNT {
            write(
                root,
                &format!("src/m{index}.mw"),
                &format!(
                    "module m{index}\nuse shared\n\
                     fn pick{index}(s: shared::Status): shared::Status\n    return s\n\
                     fn shelve{index}(b: shared::Book): string\n    return b.title\n\
                     pub fn entry{index}(): shared::Status\n    return shared::Status::Active\n"
                ),
            );
        }
        // One module reaches for the shared module's private enum, both as an
        // annotation and as a value.
        write(
            root,
            "src/intruder.mw",
            "module intruder\nuse shared\n\
             fn peek(): shared::Hidden\n    return shared::Hidden::One\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let private = with_code(&report, "check.private_enum");
    assert_eq!(
        private.len(),
        2,
        "the one private-enum reach reports exactly its annotation and value, not one per module: {:#?}",
        report.diagnostics
    );
    assert!(
        private.iter().all(|diagnostic| diagnostic.payload
            == DiagnosticPayload::PrivateEnum("shared::Hidden".into())),
        "{private:#?}"
    );
    assert!(
        with_code(&report, "check.unknown_type").is_empty()
            && with_code(&report, "check.unresolved_call").is_empty(),
        "every qualified shared enum and resource reference resolves at scale: {:#?}",
        report.diagnostics
    );
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
        DiagnosticPayload::UnknownType(marrow_schema::Type::Named("Book".into()))
    );
}

#[test]
fn many_modules_resolve_independently_while_ambiguity_is_still_detected() {
    // A project with many modules resolves every bare same-module call O(1) through
    // the module-name index, not a linear scan. The index must not change resolution:
    // each module's bare call to its own helper checks clean, two modules sharing a
    // `pub fn shared` make a bare cross-module `shared()` ambiguous, and a bare call
    // to a name no module declares is still unresolved.
    const MODULE_COUNT: usize = 64;
    let root = temp_project("resolve-many-modules", |root| {
        for index in 0..MODULE_COUNT {
            write(
                root,
                &format!("src/m{index}.mw"),
                &format!(
                    "module m{index}\n\
                     fn helper{index}(x: int): int\n    return x + {index}\n\
                     pub fn entry{index}(x: int): int\n    return helper{index}(x)\n"
                ),
            );
        }
        // Two modules expose the same bare name, and a third calls it unqualified.
        write(
            root,
            "src/dup_a.mw",
            "module dup_a\npub fn shared(): int\n    return 1\n",
        );
        write(
            root,
            "src/dup_b.mw",
            "module dup_b\npub fn shared(): int\n    return 2\n",
        );
        write(
            root,
            "src/caller.mw",
            "module caller\n\
             fn calls_ambiguous(): int\n    return shared()\n\
             fn calls_missing(): int\n    return nowhere()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_eq!(
        with_code(&report, "check.ambiguous_call").len(),
        1,
        "a bare call to a pub fn in two modules stays ambiguous at scale: {:#?}",
        report.diagnostics
    );
    let unresolved = with_code(&report, "check.unresolved_call");
    assert_eq!(
        unresolved.len(),
        1,
        "exactly the one undeclared name is unresolved; the many same-module calls resolve: {:#?}",
        report.diagnostics
    );
    assert!(
        unresolved[0].file.ends_with("caller.mw"),
        "the unresolved call is the undeclared `nowhere()` in caller.mw: {:#?}",
        unresolved[0]
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

#[test]
fn duplicate_declared_name_in_one_module_is_unresolved_not_ambiguous() {
    // `aaa` declares `pub fn greet` twice (a `check.duplicate_declaration`); `zzz`
    // declares no `greet` and bare-calls it. Only one module declares the name, so
    // the bare call is reachable solely as `aaa::greet` and stays a plain
    // `check.unresolved_call` — the diagnostic-enrichment scan counts the declaring
    // module once, never once per duplicate declaration, so the duplicate must not
    // forge a second candidate and flip the call to `check.ambiguous_call`.
    let root = temp_project("resolve-duplicate-decl", |root| {
        write(
            root,
            "src/aaa.mw",
            "module aaa\npub fn greet(): int\n    return 1\npub fn greet(): int\n    return 2\n",
        );
        write(
            root,
            "src/zzz.mw",
            "module zzz\nfn run(): int\n    return greet()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !with_code(&report, "check.duplicate_declaration").is_empty(),
        "the doubly-declared `greet` is reported as a duplicate declaration: {:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "check.unresolved_call").len(),
        1,
        "a bare call to a name declared in one module stays unresolved: {:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.ambiguous_call").is_empty(),
        "a duplicate declaration in one module must not forge a second candidate: {:#?}",
        report.diagnostics
    );
}

#[test]
fn many_stores_resolve_their_own_roots_and_a_shared_root_across_modules() {
    // Saved-root resolution routes through the O(1) store-root index. At scale the
    // semantics must be unchanged: every module reads its own `^rootN` and resolves
    // another module's project-visible `^books`, and the whole project still checks
    // clean — the index never strands a declared root or mis-resolves one module's
    // root for another's.
    const MODULE_COUNT: usize = 64;
    let root = temp_project("resolve-many-stores", |root| {
        write(
            root,
            "src/shared.mw",
            "module shared\n\
             resource Book\n    title: string\n\
             store ^books(id: int): Book\n",
        );
        for index in 0..MODULE_COUNT {
            write(
                root,
                &format!("src/m{index}.mw"),
                &format!(
                    "module m{index}\n\
                     resource Row{index}\n    label: string\n\
                     store ^rows{index}(id: int): Row{index}\n\
                     fn label_of{index}(id: Id(^rows{index})): string\n    return ^rows{index}(id).label ?? \"\"\n\
                     fn shared_title{index}(id: Id(^books)): string\n    return ^books(id).title ?? \"\"\n"
                ),
            );
        }
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}
