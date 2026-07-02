//! Thin boundary checks for enum identity at the CLI: the run JSON envelope,
//! check diagnostics, and the multi-invocation evolve lifecycle. Single-process
//! enum semantics live in the conformance corpus
//! (`fixtures/v01/conformance/enum_semantics`).

use std::path::Path;

use crate::support;
use serde_json::Value;
use support::{marrow_sub, temp_project, write};

/// Check a project directory through the structured JSONL surface, returning the
/// diagnostic records (every record except the trailing summary). Enum-identity
/// mismatches are asserted on typed codes; rendered prose is pinned by goldens.
fn check_diagnostics(dir: &str) -> (std::process::Output, Vec<Value>) {
    let output = marrow_sub("check", &["--format", "jsonl", dir]);
    let records = support::diagnostic_records(output.stdout.clone());
    (output, records)
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

/// A deeply nested module `a::b::c` imported under its short alias `c` via
/// `use a::b::c`. The alias names the *module*, so `c::Status` must expand to
/// `a::b::c` before enum resolution.
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
fn renaming_an_enum_category_carries_its_descendant_leaves_stored_identity_forward() {
    // A category member rename (`Pet::mammal` -> `Pet::beast`) is a member rename, so it
    // is identity-preserving: a value stored under a leaf below the category
    // (`Pet::mammal::dog`) must read back as the same leaf under the new category path
    // (`Pet::beast::dog`) without per-leaf renames. The rename must cascade to every
    // descendant leaf's full-path saved identity, so check does not treat the descendants
    // as new, and a single `marrow run` reads the stored value back and exits 0.
    let root = temp_project("run-enum-category-rename", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             enum Pet\n    category mammal\n        dog\n        cat\n    fish\n\n\
             resource Owner\n    required favorite: Pet\n\
             store ^owners(id: int): Owner\n\n\
             pub fn seed()\n    \
             var o: Owner\n    o.favorite = Pet::dog\n    \
             transaction\n        ^owners(1) = o\n\n\
             pub fn show()\n    \
             if const f = ^owners(1).favorite\n        \
             match f\n            mammal\n                print(\"mammal\")\n            \
             fish\n                print(\"fish\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let seed = marrow_sub("run", &["--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // Rename the category and add the single `evolve rename` of the category member.
    write(
        &root,
        "src/app.mw",
        "module app\n\
         enum Pet\n    category beast\n        dog\n        cat\n    fish\n\n\
         resource Owner\n    required favorite: Pet\n\
         store ^owners(id: int): Owner\n\n\
         evolve\n    rename Pet::mammal -> Pet::beast\n\n\
         pub fn seed()\n    \
         var o: Owner\n    o.favorite = Pet::dog\n    \
         transaction\n        ^owners(1) = o\n\n\
         pub fn show()\n    \
         if const f = ^owners(1).favorite\n        \
         match f\n            beast\n                print(\"beast\")\n            \
         fish\n                print(\"fish\")\n",
    );

    // The category rename carries the descendant leaves' stored identity forward, so
    // check reports nothing about them: the lone diagnostic is the stale-lock advisory
    // the source rewrite itself caused.
    let (check, records) = check_diagnostics(&dir);
    assert_eq!(check.status.code(), Some(0), "check: {check:?}");
    assert_eq!(
        support::codes(&records),
        vec!["check.stale_lock"],
        "the descendant leaves must not be reported as new: {records:#?}"
    );

    // The rename re-addresses the populated stored value, a non-additive identity change,
    // so a bare run fences rather than auto-applying it.
    let fenced = marrow_sub("run", &["--entry", "app::show", &dir]);
    assert_eq!(fenced.status.code(), Some(1), "fenced run: {fenced:?}");
    let fault = support::parse_result_line(&support::last_fault(&fenced.stderr));
    assert_eq!(
        fault.code, "run.schema_drift",
        "the populated category rename fences the run"
    );

    // The explicit `evolve apply` discharges the rename, carrying the descendant leaf's
    // stored identity forward.
    let apply = marrow_sub("evolve", &["apply", &dir]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");

    // The stored value now reads back as `Pet::beast::dog` and dispatches the `beast` arm.
    let show = marrow_sub("run", &["--entry", "app::show", &dir]);
    assert_eq!(show.status.code(), Some(0), "show: {show:?}");
    let stdout = String::from_utf8(show.stdout).expect("stdout utf8");
    assert_eq!(stdout, "beast\n");
}

#[test]
fn a_cross_module_same_named_enum_mismatch_names_both_modules() {
    // Two modules each declare `Status`; passing `a::Status::open` to a parameter
    // typed `b::Status` is a nominal mismatch. Both short names are `Status`, so
    // an unqualified message ("expects `Status`, but found `Status`") is useless.
    // The golden pins the render contract: each side qualified with its owning module.
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
    support::assert_matches_golden(
        mismatch["message"].as_str().expect("diagnostic message"),
        "enum_mismatch_names_both_modules.txt",
    );
}
