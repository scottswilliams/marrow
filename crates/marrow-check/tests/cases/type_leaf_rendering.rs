//! Characterization goldens pinning the exact rendered message bytes for the four
//! nominal `MarrowType` leaves — resource, group entry, identity, and enum. These
//! leaves carry their identity as an interned id rather than a stored string, and
//! their diagnostic prose is recovered by id at render time. The messages here are
//! the diff oracle: any change to how a leaf's name is spelled in a mismatch must
//! move one of these strings, so a leaf reshape that keeps them green is
//! byte-faithful to the behavior it replaced.
//!
//! The corners pinned are the ones where the spelling is non-obvious: a
//! cross-module same-name enum qualifies both sides; a same-module enum stays bare;
//! a script-vs-module enum renders an empty module prefix (`::Status`); an
//! undeclared identity root still spells `Id(^root)`; and duplicate same-name enums
//! in one module alias to the first (first-wins), so a reference to either renders
//! the same bare name.

use crate::support;
use marrow_check::check_project;

use support::{check_module, check_script, config, temp_project, with_code, write};

/// The lone message of the first diagnostic with `code`, panicking if the count is
/// not exactly one so a golden never silently pins the wrong diagnostic.
fn only_message(root: &std::path::Path, code: &str) -> String {
    let (report, _program) = check_project(root, &config()).expect("check");
    let found = with_code(&report, code);
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    found[0].message.clone()
}

#[test]
fn cross_module_same_name_enum_mismatch_qualifies_both_sides() {
    let root = temp_project("leaf-render-cross-module-enum", |root| {
        write(root, "src/a.mw", "module a\npub enum Status\n    active\n");
        write(root, "src/b.mw", "module b\npub enum Status\n    open\n");
        write(
            root,
            "src/m.mw",
            "module m\nuse a\nuse b\n\
             fn want(s: a::Status): b::Status\n    return s\n",
        );
    });
    assert_eq!(
        only_message(&root, "check.return_type"),
        "function returns `b::Status`, but this value is `a::Status`",
    );
}

#[test]
fn same_module_enum_mismatch_uses_bare_names() {
    let found = check_module(
        "leaf-render-same-module-enum",
        "module m\n\
         enum Color\n    red\n    green\n\
         enum Shade\n    light\n    dark\n\
         fn want(c: Color): Shade\n    return c\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].message,
        "function returns `Shade`, but this value is `Color`",
    );
}

#[test]
fn script_vs_module_same_name_enum_renders_empty_module_prefix() {
    let root = temp_project("leaf-render-script-module-enum", |root| {
        write(root, "src/a.mw", "module a\npub enum Status\n    active\n");
        write(
            root,
            "src/app.mw",
            "use a\n\
             enum Status\n    open\n\
             fn want(s: Status): a::Status\n    return s\n",
        );
    });
    assert_eq!(
        only_message(&root, "check.return_type"),
        "function returns `a::Status`, but this value is `::Status`",
    );
}

#[test]
fn undeclared_identity_root_still_spells_id_of_root() {
    let found = check_script(
        "leaf-render-undeclared-identity",
        "fn want(a: Id(^missing)): int\n    return a\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].message,
        "function returns `int`, but this value is `Id(^missing)`",
    );
}

#[test]
fn duplicate_same_name_enums_in_one_module_alias_to_the_first() {
    // Two enums named `Status` declared in the same module. A reference resolves to
    // the first (first-wins), so a mismatch against a third enum spells the bare
    // `Status` for whichever declaration a reference reaches — the duplicate is
    // invisible in the rendered name, matching the pre-interning string behavior.
    let found = check_module(
        "leaf-render-duplicate-enum",
        "module m\n\
         enum Status\n    active\n\
         enum Status\n    open\n\
         enum Other\n    x\n\
         fn want(s: Status): Other\n    return s\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].message,
        "function returns `Other`, but this value is `Status`",
    );
}
