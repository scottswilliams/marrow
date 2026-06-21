use std::path::Path;

use crate::support;
use serde_json::Value;
use support::{marrow_sub, temp_project, write};

/// Check a project directory through the structured JSONL surface, returning the
/// diagnostic records (every record except the trailing summary). Enum-identity
/// mismatches are asserted on typed codes and structured `message` payloads, not
/// on a rendered stderr blob.
fn check_diagnostics(dir: &str) -> (std::process::Output, Vec<Value>) {
    let output = marrow_sub("check", &["--format", "jsonl", dir]);
    let records = support::diagnostic_records(output.stdout.clone());
    (output, records)
}

#[test]
fn a_same_named_enum_in_another_module_does_not_alias() {
    // Two modules each declare an enum `Status`, with members in opposite order:
    // module `b` stores its own `Status::active` to a saved `state: Status`
    // field. Enum identity is module-qualified, so binding the field back through
    // `b` must match `b::Status::active`, not the same-named enum in `a`.
    let root = temp_project("run-enum-same-name", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "b::seed" } }"#,
        );
        write(
            root,
            "src/a.mw",
            "module a\nenum Status\n    active\n    archived\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             enum Status\n    archived\n    active\n\n\
             resource Order\n    required state: Status\n\
             store ^orders(id: int): Order\n\n\
             pub fn seed()\n    \
             var o: Order\n    o.state = Status::active\n    \
             transaction\n        ^orders(1) = o\n\n\
             pub fn show()\n    \
             if const state = ^orders(1).state\n        \
             match state\n            active\n                print(\"active\")\n            archived\n                print(\"archived\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let run = marrow_sub("run", &[&dir]);
    assert_eq!(run.status.code(), Some(0), "seed: {run:?}");

    let got = marrow_sub("run", &["--entry", "b::show", &dir]);
    assert_eq!(got.status.code(), Some(0), "show: {got:?}");
    let stdout = String::from_utf8(got.stdout).expect("stdout utf8");
    assert_eq!(stdout, "active\n");
}

#[test]
fn an_enum_return_renders_by_member_name_in_the_run_json_envelope() {
    // The run JSON envelope must name an enum return by its stable `Enum::member` spelling, the
    // same form `print`/`string(enum)`/interpolation render, not by positional internal
    // `enum_id`/`member_id` indices. Reordering members changes those indices but not the name, so
    // the name is the reorder-invariant identifier a client can rely on.
    let root = temp_project("run-enum-return-json", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             pub enum Status\n    active\n    archived\n    banned\n\n\
             pub fn pick(): Status\n    return Status::archived\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();

    let got = marrow_sub("run", &["--entry", "app::pick", "--format", "json", &dir]);
    assert_eq!(got.status.code(), Some(0), "pick: {got:?}");
    let envelope: Value =
        serde_json::from_slice(&got.stdout).expect("stdout is one JSON run envelope");
    assert_eq!(
        envelope["result"],
        serde_json::json!({
            "kind": "value",
            "value": { "kind": "enum", "member": "Status::archived" }
        }),
        "an enum return names its member, not positional ids: {envelope:#?}"
    );
}

#[test]
fn a_match_over_a_saved_enum_field_dispatches_through_the_real_pipeline() {
    // A `match` whose scrutinee is bound from a saved enum-field read must type as
    // `Status` so the checker records the scrutinee's enum on the match and the
    // runtime dispatches through `Status`'s traversal table. Before the field read
    // was typed it was `Unknown`: the checker recorded no enum, and the match
    // faulted at runtime instead of dispatching. Seeding `Status::archived` then
    // matching must take the `archived` arm and print its marker.
    let root = temp_project("run-enum-field-match", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             enum Status\n    active\n    archived\n    banned\n\n\
             resource Order\n    required state: Status\n\
             store ^orders(id: int): Order\n\n\
             pub fn seed()\n    \
             var o: Order\n    o.state = Status::archived\n    \
             transaction\n        ^orders(1) = o\n\n\
             pub fn label()\n    \
             if const state = ^orders(1).state\n        \
             match state\n            \
             active\n                print(\"A\")\n            \
             archived\n                print(\"R\")\n            \
             banned\n                print(\"B\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let seed = marrow_sub("run", &["--entry", "app::seed", &dir]);
    let label = marrow_sub("run", &["--entry", "app::label", &dir]);

    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert_eq!(label.status.code(), Some(0), "label: {label:?}");
    let stdout = String::from_utf8(label.stdout).expect("stdout utf8");
    assert_eq!(stdout, "R\n");
}

#[test]
fn equality_on_a_saved_enum_field_dispatches_through_the_real_pipeline() {
    // A nominal `==` whose left side is bound from a saved enum-field read must type
    // as `Status` so the comparison checks clean and runs. Seeding
    // `Status::archived` then comparing the read field against `Status::archived` is
    // true; against a different member, false.
    let root = temp_project("run-enum-field-eq", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             enum Status\n    active\n    archived\n    banned\n\n\
             resource Order\n    required state: Status\n\
             store ^orders(id: int): Order\n\n\
             pub fn seed()\n    \
             var o: Order\n    o.state = Status::archived\n    \
             transaction\n        ^orders(1) = o\n\n\
             pub fn check()\n    \
             if const state = ^orders(1).state\n        \
             if state == Status::archived\n            print(\"yes\")\n        \
             if state == Status::active\n            print(\"no\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let seed = marrow_sub("run", &["--entry", "app::seed", &dir]);
    let check = marrow_sub("run", &["--entry", "app::check", &dir]);

    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert_eq!(check.status.code(), Some(0), "check: {check:?}");
    let stdout = String::from_utf8(check.stdout).expect("stdout utf8");
    assert_eq!(stdout, "yes\n");
}

#[test]
fn a_qualified_enum_member_literal_resolves_to_the_owning_enum() {
    // A `mod::Enum::member` value must evaluate as that module's enum. Module
    // `a` passes both qualified members to `b::rank`, whose match dispatch proves
    // they resolved as `b::Status` rather than an unsupported qualified name.
    let root = temp_project("run-enum-qualified-member", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n\n\
             pub fn rank(s: b::Status): int\n    \
             match s\n        open\n            return 0\n        closed\n            return 1\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             pub fn show()\n    \
             print($\"{b::rank(b::Status::open)}\")\n    print($\"{b::rank(b::Status::closed)}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let show = marrow_sub("run", &["--entry", "a::show", &dir]);

    assert_eq!(show.status.code(), Some(0), "show: {show:?}");
    let stdout = String::from_utf8(show.stdout).expect("stdout utf8");
    assert_eq!(stdout, "0\n1\n");
}

#[test]
fn a_nested_module_qualified_enum_program_checks_and_runs() {
    // End-to-end: a nested module `a::b` (two-segment module name) exposes
    // `take(s: a::b::Status)` and is called with `a::b::Status::active` — a
    // four-segment qualified literal. The checker must resolve the parameter and
    // argument to the same `a::b::Status`, and the runtime must evaluate the
    // four-segment literal as that enum's `active` value so `take` returns 1. A
    // first-separator split would leave the parameter `Unknown` (silent pass) and
    // the literal would fault as an unsupported qualified name at runtime.
    let root = temp_project("run-enum-nested-module", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/a/b.mw",
            "module a::b\n\
             pub enum Status\n    active\n    archived\n\n\
             pub fn take(s: a::b::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\n\
             pub fn main()\n    print($\"{a::b::take(a::b::Status::active)}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let run = marrow_sub("run", &[&dir]);

    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert_eq!(stdout, "1\n");
}

/// A deeply nested module `a::b::c` imported under its short alias `c` via
/// `use a::b::c`. The alias names the *module*, so `c::Status` must expand to
/// `a::b::c` before enum resolution — at the annotation, the literal, and the
/// runtime. The enum lives in `a::b::c`; the aliased spellings live in the
/// importing file. These tests pin that an aliased enum spelling resolves to the
/// imported module rather than failing open, faulting, or binding wrong.
fn alias_module_sources(root: &Path) {
    write(
        root,
        "src/a/b/c.mw",
        "module a::b::c\npub enum Status\n    active\n    archived\n\n\
         pub fn marker(s: a::b::c::Status): int\n    \
         match s\n        active\n            return 0\n        archived\n            return 9\n",
    );
}

#[test]
fn an_aliased_annotation_rejects_a_foreign_enum_argument() {
    // `use a::b::c` aliases module `a::b::c` to `c`, so the parameter spelling
    // `s: c::Status` names `a::b::c`'s `Status`. A foreign `a::b::Status::open`
    // (a different module's same-named enum) is a nominal mismatch. Before the
    // alias was expanded the annotation resolved to `Unknown` and the foreign
    // value passed open with exit 0.
    let root = temp_project("run-alias-annotation-foreign", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/a/b.mw",
            "module a::b\npub enum Status\n    open\n    closed\n",
        );
        alias_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b\nuse a::b::c\n\
             pub fn classify(s: c::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n\n\
             pub fn run(): int\n    return classify(a::b::Status::open)\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let (check, records) = check_diagnostics(&dir);
    assert_eq!(check.status.code(), Some(1), "check: {check:?}");
    assert!(
        support::codes(&records).contains(&"check.call_argument"),
        "expected a call_argument mismatch, got: {records:#?}"
    );
}

#[test]
fn an_aliased_enum_literal_checks_and_runs() {
    // `var v: c::Status = c::Status::active` under `use a::b::c`. Both the
    // annotation and the literal must expand `c` to `a::b::c`, so the program
    // checks clean and the match dispatches `active` to return 1. Before
    // expansion the annotation was `Unknown` and the literal faulted at runtime
    // as an unsupported qualified name.
    let root = temp_project("run-alias-literal", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        alias_module_sources(root);
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b::c\n\
             pub fn classify(s: c::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 2\n\n\
             pub fn main()\n    \
             var v: c::Status = c::Status::active\n    \
             print($\"{classify(v)}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let run = marrow_sub("run", &[&dir]);
    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert_eq!(stdout, "1\n");
}

#[test]
fn an_aliased_enum_literal_binds_to_the_imported_module_not_a_top_level_homonym() {
    // A real top-level `module c` also declares `Status`, with members in the
    // opposite member meaning. Under `use a::b::c`, both `c::marker` and
    // `c::Status::active` must bind to imported `a::b::c`, not the homonymous
    // top-level `c`. Without alias expansion both bind to top-level `c` and print
    // 1 instead of 0.
    let root = temp_project("run-alias-literal-homonym", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        alias_module_sources(root);
        write(
            root,
            "src/c.mw",
            "module c\npub enum Status\n    archived\n    active\n\n\
             pub fn marker(s: c::Status): int\n    \
             match s\n        active\n            return 1\n        archived\n            return 8\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b::c\n\
             pub fn main()\n    print($\"{c::marker(c::Status::active)}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let run = marrow_sub("run", &[&dir]);
    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    // The imported module returns 0; the top-level homonym returns 1.
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert_eq!(stdout, "0\n", "literal bound to the wrong module");
}

#[test]
fn a_cross_module_same_named_enum_mismatch_names_both_modules() {
    // Two modules each declare `Status`; passing `a::Status::open` to a parameter
    // typed `b::Status` is a nominal mismatch. Both short names are `Status`, so
    // an unqualified message ("expects `Status`, but found `Status`") is useless.
    // The diagnostic must qualify each side with its owning module.
    let root = temp_project("run-enum-mismatch-display", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/a.mw",
            "module a\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    on\n    off\n\n\
             pub fn take(s: b::Status): int\n    return 0\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a\nuse b\n\
             pub fn run(): int\n    return b::take(a::Status::open)\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let (check, records) = check_diagnostics(&dir);
    assert_eq!(check.status.code(), Some(1), "check: {check:?}");
    let mismatch = records
        .iter()
        .find(|record| record["code"] == "check.call_argument")
        .expect("a call_argument mismatch");
    // Both short names are `Status`, so the rendered message must qualify each
    // side with its owning module; this disambiguation is the render contract.
    let message = mismatch["message"].as_str().expect("diagnostic message");
    assert!(
        message.contains("a::Status") && message.contains("b::Status"),
        "expected both modules named, got: {mismatch}"
    );
}
