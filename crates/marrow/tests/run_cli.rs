use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn run_run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("run")
        .args(args)
        .output()
        .expect("run marrow run")
}

#[test]
fn runs_the_default_entry_and_prints_its_output() {
    let root = temp_project("run-default", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hello from marrow\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "hello from marrow\n");
}

#[test]
fn entry_flag_overrides_the_default_entry() {
    let root = temp_project("run-entry", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"main\")\n\npub fn greet()\n    print(\"greet\")\n",
        );
    });
    let output = run_run(&["--entry", "app::greet", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "greet\n");
}

#[test]
fn bare_entry_flag_resolves_the_first_matching_function() {
    let root = temp_project("run-bare-entry", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/util.mw",
            "module util\n\npub fn helper(): int\n    print(\"helper ran\")\n    return 1\n",
        );
    });
    let output = run_run(&["--entry", "helper", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "helper ran\n");
}

#[test]
fn run_rejects_duplicate_format_flag() {
    let output = run_run(&["--format", "json", "--format", "text", "missing-project"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}

#[test]
fn module_constants_are_bound_at_runtime() {
    let root = temp_project("run-module-const", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             const Base: int = 40\n\
             const Offset = 2\n\
             const Label = \"answer\"\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20print($\"{Label}={Base + Offset}\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "answer=42\n");
}

#[test]
fn native_store_persists_writes_across_runs() {
    let root = temp_project("run-native", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Counter at ^counter(id: int)\n\
             \x20\x20\x20\x20required value: int\n\
             \n\
             pub fn bump()\n\
             \x20\x20\x20\x20var c: Counter\n\
             \x20\x20\x20\x20c.value = 1\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
             \n\
             pub fn show()\n\
             \x20\x20\x20\x20if not exists(^counter(1))\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print(\"absent\")\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return\n\
             \x20\x20\x20\x20print($\"value={^counter(1).value}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    // One process writes the counter; a second process reads it back. Only a
    // persistent store carries the write across the two runs.
    let first = run_run(&["--entry", "shelf::bump", &dir]);
    let second = run_run(&["--entry", "shelf::show", &dir]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(first.status.code(), Some(0), "bump: {first:?}");
    assert_eq!(second.status.code(), Some(0), "show: {second:?}");
    let stdout = String::from_utf8(second.stdout).expect("stdout utf8");
    assert_eq!(stdout, "value=1\n");
}

#[test]
fn reports_a_missing_entry() {
    let root = temp_project("run-noentry", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("run.no_entry"), "{stderr}");
}

#[test]
fn refuses_to_run_a_project_that_does_not_check() {
    let root = temp_project("run-badcheck", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        // The path implies module `shelf::books`, but the file declares another.
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.module_path"), "{stderr}");
}

#[test]
fn native_store_requires_a_data_dir() {
    let root = temp_project("run-nodatadir", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" }, "store": { "backend": "native" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config.invalid"), "{stderr}");
}

#[test]
fn an_uncaught_throw_exits_one_with_the_thrown_code_on_stderr() {
    // The headline runtime failure surface: a throw that propagates out of the
    // entry surfaces as run.uncaught_error with the thrown dotted code embedded.
    let root = temp_project("run-throw", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("book.absent"), "{stderr}");
}

#[test]
fn an_uncaught_unique_conflict_exits_one_with_its_write_code_on_stderr() {
    // A managed-write fault that escapes the entry is fatal: it exits non-zero and
    // its `write.unique_conflict` dotted code reaches stderr, even though the fault
    // is also catchable from within the program.
    let root = temp_project("run-conflict", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n    required title: string\n    isbn: string\n\n    index byIsbn(isbn) unique\n\n\
             pub fn main()\n    ^books(1).title = \"Mort\"\n    ^books(1).isbn = \"978-0\"\n    ^books(2).title = \"Pyramids\"\n    ^books(2).isbn = \"978-0\"\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("write.unique_conflict"), "{stderr}");
}

#[test]
fn maps_an_unknown_entry_to_a_runtime_code() {
    let root = temp_project("run-unknown", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = run_run(&["--entry", "app::nope", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("run.unknown_function"), "{stderr}");
}

#[test]
fn maintenance_flag_gates_a_whole_root_drop() {
    // A whole managed-root drop (`delete ^books`) is maintenance work. The
    // ordinary `marrow run` cannot reach it (rejected with the maintenance code);
    // `marrow run --maintenance` opts in explicitly and performs the drop. A
    // native store carries the seed across the separate runs.
    let root = temp_project("run-maintenance", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20required title: string\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\n\
             pub fn drop_root()\n\
             \x20\x20\x20\x20delete ^books\n\n\
             pub fn count()\n\
             \x20\x20\x20\x20var c = 0\n\
             \x20\x20\x20\x20for id in ^books\n\
             \x20\x20\x20\x20\x20\x20\x20\x20c = c + 1\n\
             \x20\x20\x20\x20print($\"count={c}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();

    let seed = run_run(&["--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    // Default run cannot drop the whole root.
    let denied = run_run(&["--entry", "app::drop_root", &dir]);
    assert_eq!(denied.status.code(), Some(1), "denied: {denied:?}");
    let denied_err = String::from_utf8(denied.stderr).expect("stderr utf8");
    assert!(
        denied_err.contains("write.requires_maintenance"),
        "{denied_err}"
    );

    // Explicit maintenance opt-in performs the drop.
    let allowed = run_run(&["--maintenance", "--entry", "app::drop_root", &dir]);
    assert_eq!(allowed.status.code(), Some(0), "allowed: {allowed:?}");

    // After the drop, no records remain.
    let after = run_run(&["--entry", "app::count", &dir]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(after.status.code(), Some(0), "count: {after:?}");
    let after_out = String::from_utf8(after.stdout).expect("stdout utf8");
    assert_eq!(after_out, "count=0\n");
}

#[test]
fn maintenance_flag_appears_in_help() {
    let output = run_run(&["--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("--maintenance"), "{stdout}");
}

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

#[test]
fn a_same_named_enum_in_another_module_does_not_alias() {
    // Two modules each declare an enum `Status`, with members in opposite order:
    // discovery is path-sorted so `a::Status` ({active, archived}, active=0) is
    // seen before `b::Status` ({archived, active}, active=1). Module `b` stores
    // its own `Status::active` to a saved `state: Status` field. Enum identity is
    // module-qualified, so `Status::active` in `b` is `b`'s ordinal 1, and the
    // raw stored byte is `1`. Resolving the bare name `Status` project-wide would
    // first-match `a::Status`, making `Status::active` ordinal 0 — the value `a`
    // and `b` would alias to.
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
             resource Order at ^orders(id: int)\n    required state: Status\n\n\
             pub fn seed()\n    \
             var o: Order\n    o.state = Status::active\n    \
             transaction\n        ^orders(1) = o\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let run = run_run(&[&dir]);
    assert_eq!(run.status.code(), Some(0), "seed: {run:?}");

    // The raw stored byte is the ordinal of `b`'s `Status::active`, which is 1.
    // Aliasing to `a::Status` would store 0.
    let got = marrow(&["data", "get", &dir, "^orders(1).state"]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(got.status.code(), Some(0), "get: {got:?}");
    let stdout = String::from_utf8(got.stdout).expect("stdout utf8");
    assert!(
        stdout.contains('1'),
        "expected stored ordinal 1, got: {stdout}"
    );
    assert!(
        !stdout.contains('0'),
        "stored ordinal aliased to a::Status: {stdout}"
    );
}

#[test]
fn a_match_over_a_saved_enum_field_dispatches_through_the_real_pipeline() {
    // A `match` whose scrutinee is a saved enum-field read `^orders(1).state` must
    // type as `Status` so the checker records the scrutinee's enum on the match and
    // the runtime dispatches by `Status`'s ordinals. Before the field read was
    // typed it was `Unknown`: the checker recorded no enum, and the match faulted
    // at runtime instead of dispatching. Seeding `Status::archived` (ordinal 1)
    // then matching must take the `archived` arm and print its marker.
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
             resource Order at ^orders(id: int)\n    required state: Status\n\n\
             pub fn seed()\n    \
             var o: Order\n    o.state = Status::archived\n    \
             transaction\n        ^orders(1) = o\n\n\
             pub fn label()\n    \
             match ^orders(1).state\n        \
             active\n            print(\"A\")\n        \
             archived\n            print(\"R\")\n        \
             banned\n            print(\"B\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let seed = run_run(&["--entry", "app::seed", &dir]);
    let label = run_run(&["--entry", "app::label", &dir]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert_eq!(label.status.code(), Some(0), "label: {label:?}");
    let stdout = String::from_utf8(label.stdout).expect("stdout utf8");
    assert_eq!(stdout, "R\n");
}

#[test]
fn equality_on_a_saved_enum_field_dispatches_through_the_real_pipeline() {
    // A nominal `==` whose left side is a saved enum-field read must type as
    // `Status` so the comparison checks clean and runs. Seeding `Status::archived`
    // then comparing the read field against `Status::archived` is true; against a
    // different member, false.
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
             resource Order at ^orders(id: int)\n    required state: Status\n\n\
             pub fn seed()\n    \
             var o: Order\n    o.state = Status::archived\n    \
             transaction\n        ^orders(1) = o\n\n\
             pub fn check()\n    \
             if ^orders(1).state == Status::archived\n        print(\"yes\")\n    \
             if ^orders(1).state == Status::active\n        print(\"no\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let seed = run_run(&["--entry", "app::seed", &dir]);
    let check = run_run(&["--entry", "app::check", &dir]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert_eq!(check.status.code(), Some(0), "check: {check:?}");
    let stdout = String::from_utf8(check.stdout).expect("stdout utf8");
    assert_eq!(stdout, "yes\n");
}

#[test]
fn a_qualified_enum_member_literal_evaluates_to_its_ordinal() {
    // A `mod::Enum::member` value evaluates to that enum's ordinal at runtime: in
    // module `b`, `Status::open` is ordinal 0 and `Status::closed` is ordinal 1.
    // Module `a` reads them by their qualified spelling and prints both ordinals.
    let root = temp_project("run-enum-qualified-member", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/b.mw",
            "module b\npub enum Status\n    open\n    closed\n",
        );
        write(
            root,
            "src/a.mw",
            "module a\nuse b\n\
             pub fn show()\n    \
             print($\"{b::Status::open}\")\n    print($\"{b::Status::closed}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let show = run_run(&["--entry", "a::show", &dir]);
    fs::remove_dir_all(&root).ok();

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
    // four-segment literal to its ordinal (active = 0) so `take` returns 1. A
    // first-separator split would leave the parameter `Unknown` (silent pass) and
    // the literal would fault as an unsupported qualified name at runtime.
    let root = temp_project("run-enum-nested-module", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
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
    let run = run_run(&[&dir]);
    fs::remove_dir_all(&root).ok();

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
        "module a::b::c\npub enum Status\n    active\n    archived\n",
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
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
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
    let check = marrow(&["check", &dir]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(check.status.code(), Some(1), "check: {check:?}");
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("check.call_argument"),
        "expected a call_argument mismatch, got: {stderr}"
    );
}

#[test]
fn an_aliased_enum_literal_checks_and_runs() {
    // `var v: c::Status = c::Status::active` under `use a::b::c`. Both the
    // annotation and the literal must expand `c` to `a::b::c`, so the program
    // checks clean and the match dispatches `active` (ordinal 0) to return 1.
    // Before expansion the annotation was `Unknown` and the literal faulted at
    // runtime as an unsupported qualified name.
    let root = temp_project("run-alias-literal", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
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
    let run = run_run(&[&dir]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    let stdout = String::from_utf8(run.stdout).expect("stdout utf8");
    assert_eq!(stdout, "1\n");
}

#[test]
fn an_aliased_enum_literal_binds_to_the_imported_module_not_a_top_level_homonym() {
    // A real top-level `module c` also declares `Status`, with members in the
    // opposite order so the ordinals differ: `a::b::c`'s `active` is 0, top-level
    // `c`'s `active` is 1. Under `use a::b::c`, `c::Status::active` must bind to
    // the imported `a::b::c` (ordinal 0), not the homonymous top-level `c`.
    // Without alias expansion the runtime literal binds to the top-level `c` and
    // evaluates to 1. Printing the literal's ordinal isolates which module bound.
    let root = temp_project("run-alias-literal-homonym", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        alias_module_sources(root);
        write(
            root,
            "src/c.mw",
            "module c\npub enum Status\n    archived\n    active\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse a::b::c\n\
             pub fn main()\n    print($\"{c::Status::active}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let run = run_run(&[&dir]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    // `a::b::c`'s `active` is ordinal 0; the top-level `c`'s is 1. The imported
    // module must win.
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
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
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
    let check = marrow(&["check", &dir]);
    fs::remove_dir_all(&root).ok();
    assert_eq!(check.status.code(), Some(1), "check: {check:?}");
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("a::Status") && stderr.contains("b::Status"),
        "expected both modules named, got: {stderr}"
    );
}

#[test]
fn an_uncaught_fault_is_located_on_stderr() {
    // A divide-by-zero that escapes the entry prints located on stderr —
    // `file:line:col: code: message`, the same shape `check` and `test` use —
    // not the bare `code: message` it printed before.
    let root = temp_project("run-located", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(): int\n    var n: int = 1\n    return n % 0\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:5:")
            && stderr.contains("run.divide_by_zero:")
            && !stderr.starts_with("run.divide_by_zero"),
        "fault must be located at its file:line:col, got: {stderr}"
    );
}

#[test]
fn a_cross_module_fault_names_the_callee_file() {
    // The entry in `app` calls into `lib`, which divides by zero. The located
    // render must name `lib`'s file — the file the fault was raised in — not the
    // entry's `app`.
    let root = temp_project("run-located-cross", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse lib\n\npub fn main(): int\n    return lib::boom()\n",
        );
        write(
            root,
            "src/lib.mw",
            "module lib\n\npub fn boom(): int\n    var n: int = 1\n    return n % 0\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/lib.mw:5:") && !stderr.contains("src/app.mw"),
        "a cross-module fault must name the callee's file, got: {stderr}"
    );
    assert!(stderr.contains("run.divide_by_zero:"), "{stderr}");
}

#[test]
fn an_overflow_fault_is_located() {
    let root = temp_project("run-located-overflow", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main(): int\n    var n: int = 9223372036854775807\n    return n + 1\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:5:") && stderr.contains("run.overflow:"),
        "overflow must be located, got: {stderr}"
    );
}

#[test]
fn an_absent_element_fault_is_located() {
    let root = temp_project("run-located-absent", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Book at ^books(id: int)\n    title: string\n\n\
             pub fn main(): string\n    return ^books(99).title\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:7:") && stderr.contains("run.absent_element:"),
        "absent_element must be located, got: {stderr}"
    );
}

#[test]
fn an_uncaught_throw_is_located() {
    let root = temp_project("run-located-throw", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("src/app.mw:3:") && stderr.contains("run.uncaught_error:"),
        "an uncaught throw must be located, got: {stderr}"
    );
}

#[test]
fn a_fault_with_no_origin_keeps_the_bare_fallback() {
    // A missing entry never reaches a project file, so its fault carries no
    // origin and must keep the bare `code: message` form — no spurious `:0:0:`.
    let root = temp_project("run-located-noorigin", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": ["src"] }"#);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = run_run(&["--entry", "app::nope", root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.unknown_function") && !stderr.contains(":0:0:"),
        "a no-origin fault must stay bare, got: {stderr}"
    );
}

#[test]
fn runs_a_module_less_script_bare_entry() {
    // A module-less script is self-resolvable: its `pub fn main` lives in the
    // empty module, so the bare entry `main` resolves to it and runs. This is the
    // legitimate path the construction restores — no `run.no_entry`.
    let root = temp_project("run-module-less-script", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "pub fn main()\n    print(\"from a script\")\n",
        );
    });
    let output = run_run(&[root.to_str().unwrap()]);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "from a script\n");
}
