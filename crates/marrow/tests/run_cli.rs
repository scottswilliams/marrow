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
    // A managed-write fault that escapes the entry stays fatal and exits non-zero,
    // and its `write.unique_conflict` dotted code still reaches stderr — the
    // uncaught surface is unchanged now that the fault is also catchable.
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
