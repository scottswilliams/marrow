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

fn run_test(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("test")
        .arg(dir)
        .output()
        .expect("run marrow test")
}

const CONFIG: &str = r#"{ "sourceRoots": ["src"], "tests": ["tests/**/*.mw"] }"#;

#[test]
fn runs_passing_tests_and_reports_a_summary() {
    let root = temp_project("test-pass", |root| {
        write(root, "marrow.json", CONFIG);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn add(a: int, b: int): int\n    return a + b\n",
        );
        write(
            root,
            "tests/app_test.mw",
            "pub fn adds_numbers()\n    std::assert::isTrue(app::add(2, 3) = 5)\n",
        );
    });
    let output = run_test(&root);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("ok    tests::app_test::adds_numbers"),
        "{stdout}"
    );
    assert!(
        stdout.contains("1 test: 1 passed, 0 failed, 0 errored"),
        "{stdout}"
    );
}

#[test]
fn a_failed_assertion_is_a_located_failure() {
    let root = temp_project("test-fail", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn wrong()\n    std::assert::isTrue(1 = 2)\n",
        );
    });
    let output = run_test(&root);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("FAIL  tests::app_test::wrong"), "{stdout}");
    assert!(stdout.contains("run.assertion"), "{stdout}");
    // The failure is located in the test file.
    assert!(stdout.contains("app_test.mw:2:"), "{stdout}");
    assert!(stdout.contains("0 passed, 1 failed"), "{stdout}");
}

#[test]
fn a_runtime_fault_is_reported_as_an_error() {
    let root = temp_project("test-error", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        // `/` yields `decimal`, so a `decimal` dividend keeps the assignment
        // well-typed at check time; the fault is purely a runtime
        // divide-by-zero.
        write(
            root,
            "tests/app_test.mw",
            "pub fn divides_by_zero()\n    var x: decimal = 1.0\n    x = x / 0.0\n",
        );
    });
    let output = run_test(&root);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("ERROR tests::app_test::divides_by_zero"),
        "{stdout}"
    );
    assert!(stdout.contains("run.divide_by_zero"), "{stdout}");
    assert!(stdout.contains("0 passed, 0 failed, 1 errored"), "{stdout}");
}

#[test]
fn reports_when_no_tests_are_found() {
    let root = temp_project("test-none", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        // No `tests/` directory exists.
    });
    let output = run_test(&root);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("test.none"), "{stderr}");
}

#[test]
fn refuses_to_run_tests_when_the_project_does_not_check() {
    let root = temp_project("test-badcheck", |root| {
        write(root, "marrow.json", CONFIG);
        // The path implies module `shelf::books`, but the file declares another.
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_test(&root);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.module_path"), "{stderr}");
}

#[test]
fn each_test_runs_against_a_fresh_store() {
    let root = temp_project("test-isolation", |root| {
        write(root, "marrow.json", CONFIG);
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Box at ^box(id: int)\n    required value: int\n",
        );
        // The first test writes the box; the second asserts it is absent. Both
        // pass only if each test gets its own fresh store.
        write(
            root,
            "tests/iso_test.mw",
            "pub fn a_writes()\n    ^box(1).value = 1\n\npub fn b_sees_a_fresh_store()\n    std::assert::absent(^box(1))\n",
        );
    });
    let output = run_test(&root);
    fs::remove_dir_all(&root).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("2 tests: 2 passed"), "{stdout}");
}
