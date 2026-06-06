mod support;

use std::path::Path;

use marrow_check::{CheckDiagnostic, DiagnosticPayload, EnumDiagnostic, check_project};

use support::{assert_clean, check_script, config, temp_project, with_code, write};

fn assert_enum_payload(diagnostic: &CheckDiagnostic, expected: EnumDiagnostic) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::Enum(expected),
        "{diagnostic:#?}"
    );
}

/// A nested-module enum `module a::b` owns `Status` and `Color`. Its module name
/// has *two* segments (`a::b`), so a qualified annotation `a::b::Status` and a
/// qualified literal `a::b::Color::red` are four-segment paths. The module/enum
/// split must keep all-but-the-last segment as the module (`a::b`), not the first
/// (`a`) — otherwise the slot stays `Unknown` and every boundary fails open.
///
/// Write `src/a/b.mw` declaring the nested module `a::b` with the `Status` and `Color`
/// enums followed by `trailing`, so every nested-module enum test shares one owner of
/// the enum declarations and only varies the function that exercises them.
fn nested_module_with(root: &Path, trailing: &str) {
    write(
        root,
        "src/a/b.mw",
        &format!(
            "module a::b\n\
             pub enum Status\n    active\n    archived\n\n\
             pub enum Color\n    red\n    green\n\n\
             {trailing}"
        ),
    );
}

/// The nested module `a::b` whose `take(s: a::b::Status)` function the cross-module
/// argument tests call from `src/app.mw`.
fn nested_module_sources(root: &Path) {
    nested_module_with(
        root,
        "pub fn take(s: a::b::Status): int\n    \
         match s\n        active\n            return 1\n        archived\n            return 2\n",
    );
}

#[test]
fn passing_a_nested_module_wrong_enum_to_a_qualified_parameter_is_a_check_error() {
    // `a::b::take(s: a::b::Status)` called with `a::b::Color::red`: enum `Color`
    // is not enum `Status`, a nominal mismatch. A first-separator split would make
    // the parameter `Unknown` (module "a", enum "b::Status" matches nothing), so
    // the wrong enum would pass with zero diagnostics.
    let root = temp_project("enum-nested-arg-cross", |root| {
        nested_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    return a::b::take(a::b::Color::red)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn passing_a_raw_scalar_to_a_nested_module_enum_parameter_is_a_check_error() {
    // The same `a::b::take(s: a::b::Status)` slot, called with a raw `int`. The
    // nested-module parameter must carry its real enum identity so the scalar is a
    // concrete mismatch, not silently accepted.
    let root = temp_project("enum-nested-arg-scalar", |root| {
        nested_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    return a::b::take(1)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn returning_a_wrong_enum_from_a_nested_module_function_is_a_check_error() {
    // A function declared `: a::b::Status` returns `a::b::Color::red`. The return
    // slot must resolve to `a::b::Status` (nested module kept whole), so returning
    // a `Color` is a nominal mismatch rather than an unresolved slot accepting any
    // value.
    let root = temp_project("enum-nested-return-cross", |root| {
        nested_module_with(root, "fn f(): a::b::Status\n    return a::b::Color::red\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.return_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn assigning_a_wrong_enum_into_a_nested_module_enum_local_is_a_check_error() {
    // A `var s: a::b::Status` local is assigned `a::b::Color::red`. The annotation
    // must resolve to `a::b::Status` so the cross-enum assignment is caught.
    let root = temp_project("enum-nested-assign-cross", |root| {
        nested_module_with(
            root,
            "fn f()\n    var s: a::b::Status = a::b::Status::active\n    s = a::b::Color::red\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.assignment_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_nonexhaustive_match_over_a_nested_module_enum_scrutinee_is_a_check_error() {
    // `s: a::b::Status` is a nested-module qualified annotation. The match over it
    // must resolve to `a::b::Status` and enforce exhaustiveness; missing `archived`
    // is a check error, not a runtime crash from a scrutinee that passed open.
    let root = temp_project("enum-nested-nonexhaustive", |root| {
        write(
            root,
            "src/a/b.mw",
            "module a::b\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn classify(s: a::b::Status): int\n    \
             match s\n        active\n            return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.nonexhaustive_match");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_enum_payload(
        found[0],
        EnumDiagnostic::NonexhaustiveMatch {
            enum_name: "Status".into(),
            missing: vec!["archived".into()],
        },
    );
}

#[test]
fn an_unknown_member_of_a_nested_module_enum_literal_is_a_check_error() {
    // `a::b::Status::bogus` names a real nested-module enum but an unknown member.
    // The four-segment literal must resolve enum=Status in module=a::b, then report
    // the missing member — not type `Unknown` and pass silently.
    let root = temp_project("enum-nested-unknown-member", |root| {
        write(
            root,
            "src/a/b.mw",
            "module a::b\npub enum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    const s: a::b::Status = a::b::Status::bogus\n    return 1\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.unknown_enum_member");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_enum_payload(
        found[0],
        EnumDiagnostic::UnknownMember {
            enum_name: "Status".into(),
            member: "bogus".into(),
        },
    );
}

#[test]
fn passing_the_matching_nested_module_enum_checks_clean() {
    // The clean counterpart: `a::b::take(s: a::b::Status)` called with the matching
    // `a::b::Status::active`. A like-for-like nested-module enum argument checks clean.
    let root = temp_project("enum-nested-arg-clean", |root| {
        nested_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             fn run(): int\n    return a::b::take(a::b::Status::active)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !report.has_errors(),
        "a matching nested-module enum argument must check clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_module_less_script_string_into_an_int_field_is_a_check_error() {
    // A file with no `module` line is a single-file script. Its own `^orders`
    // resource must still be nominally checked: storing a `string` into the
    // `int` field `count` is a type mismatch, not a silently-accepted write.
    let found = check_script(
        "script-string-into-int",
        "resource Order at ^orders(id: int)\n    required count: int\n\n\
         pub fn main()\n    var o: Order\n    o.count = \"alsobad\"\n    ^orders(1) = o\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_less_script_string_into_an_enum_field_is_a_check_error() {
    // The enum counterpart: a script's enum-typed field `state: Status` written a
    // raw `string`. The field type resolves to the script's own `Status`, so the
    // mismatch is caught rather than dropping to `Unknown` and passing.
    let found = check_script(
        "script-string-into-enum",
        "enum Status\n    active\n    archived\n\n\
         resource Order at ^orders(id: int)\n    required state: Status\n\n\
         pub fn main()\n    var o: Order\n    o.state = \"notamember\"\n    ^orders(1) = o\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_less_script_self_reference_checks_clean() {
    // The over-rejection guard: once a script's own types become visible, a
    // correct script must still check clean. Its resource, its enum-typed field,
    // and a same-enum comparison all resolve to the script's own declarations.
    let root = temp_project("script-self-reference-clean", |root| {
        write(
            root,
            "src/app.mw",
            "enum Status\n    active\n    archived\n\n\
             resource Order at ^orders(id: int)\n    required state: Status\n\n\
             pub fn main()\n    var o: Order\n    o.state = Status::active\n    \
             ^orders(1) = o\n\n\
             pub fn isActive(): bool\n    return ^orders(1).state == Status::active\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !report.has_errors(),
        "a correct module-less script must check clean: {:#?}",
        report.diagnostics
    );
}

#[test]
fn another_module_cannot_use_a_module_less_script() {
    // The import-safety invariant: a script is self-resolvable but un-importable.
    // A sibling `module other` that does `use app` against a module-less `app.mw`
    // must still fail with `check.unresolved_import` — the empty-named script is
    // never bound to a name a `use` can spell.
    let root = temp_project("script-not-importable", |root| {
        write(
            root,
            "src/app.mw",
            "resource Order at ^orders(id: int)\n    required count: int\n\n\
             pub fn main()\n    print(\"hi\")\n",
        );
        write(
            root,
            "src/other.mw",
            "module other\nuse app\n\npub fn run()\n    print(\"ok\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.unresolved_import");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::UnresolvedImport("app".into()),
        "{:#?}",
        found[0]
    );
}

#[test]
fn a_module_less_script_joins_the_program_under_the_empty_name() {
    // Pins the construction: a parse-clean script enters `program.modules` under
    // the empty module name, carrying its own resources, so the nominal resolvers
    // (which scan `program.modules`) can see `Order`. This is what turns the
    // script's field types from `Unknown` into its real types.
    let root = temp_project("script-empty-named-module", |root| {
        write(
            root,
            "src/app.mw",
            "resource Order at ^orders(id: int)\n    required count: int\n\n\
             pub fn main()\n    print(\"hi\")\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
    let script = program
        .modules
        .iter()
        .find(|module| module.name.is_empty())
        .expect("the module-less script joins the program under the empty name");
    assert!(
        script.resources.iter().any(|r| r.name == "Order"),
        "the script's own resource is present for nominal resolution"
    );
}

#[test]
fn two_module_less_scripts_are_a_check_error() {
    // The soundness fix: a project may hold at most one module-less file (its
    // single entrypoint script). Two scripts share the empty module name, so a
    // bare reference in one could resolve against the other's declarations. Rather
    // than alias them, the checker rejects every module-less file past the first —
    // a project's library files must declare a `module`.
    let root = temp_project("two-scripts-rejected", |root| {
        write(
            root,
            "src/one.mw",
            "resource Order at ^orders(id: int)\n    required count: int\n\n\
             pub fn main()\n    print(\"one\")\n",
        );
        write(
            root,
            "src/two.mw",
            "resource Ticket at ^tickets(id: int)\n    required note: string\n\n\
             pub fn other()\n    print(\"two\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.multiple_scripts");
    // Each offending file is named; neither is privileged over the other.
    assert_eq!(found.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        found.iter().any(|d| d.file.ends_with("one.mw"))
            && found.iter().any(|d| d.file.ends_with("two.mw")),
        "{found:#?}"
    );
}

#[test]
fn two_scripts_with_clashing_resources_never_silently_bind_to_the_wrong_shape() {
    // The wrong-shape-binding repro: each script declares its own `Order` with a
    // different shape (`one.mw`'s has `count`, `two.mw`'s has `priority`). Under the
    // empty-name alias, `two.mw`'s `var o: Order` could bind to `one.mw`'s `Order`,
    // and assigning a field only `two.mw`'s `Order` has would either silently accept
    // against the wrong shape or corrupt at run time. Rejecting the second script
    // makes that impossible: the binding never happens because the file is an error.
    let root = temp_project("two-scripts-wrong-shape", |root| {
        write(
            root,
            "src/one.mw",
            "resource Order at ^orders_a(id: int)\n    required count: int\n\n\
             pub fn main()\n    var o: Order\n    o.count = 1\n    ^orders_a(1) = o\n",
        );
        write(
            root,
            "src/two.mw",
            "resource Order at ^orders_b(id: int)\n    required priority: int\n\n\
             pub fn other()\n    var o: Order\n    o.priority = 9\n    ^orders_b(1) = o\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !with_code(&report, "check.multiple_scripts").is_empty(),
        "two scripts with clashing resources must be rejected, never silently bound: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_script_cannot_see_another_scripts_functions() {
    // The cross-script call repro: `b.mw` calls `helper`, declared only in `a.mw`.
    // Under the empty-name alias `b` could resolve `helper` against `a`'s module —
    // false cross-script visibility. With the scripts rejected, the call cannot
    // resolve across the file boundary.
    let root = temp_project("cross-script-call", |root| {
        write(
            root,
            "src/a.mw",
            "pub fn helper(): int\n    return 1\n\npub fn main()\n    print(\"a\")\n",
        );
        write(root, "src/b.mw", "pub fn other()\n    var x = helper()\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        !with_code(&report, "check.multiple_scripts").is_empty(),
        "b cannot see a's functions; the two-script project is rejected: {:#?}",
        report.diagnostics
    );
}
