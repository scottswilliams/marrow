use crate::support;
use std::fs;
use std::path::Path;
use std::process::Command;
use support::marrow;

const QUICKSTART: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/quickstart.md"
));

const EXPECTED_CONFIG: &str = r#"{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::books::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests"]
}
"#;

const EXPECTED_BOOKS: &str = r#"module shelf::books

resource Book
    required title: string
    required author: string
    required shelf: string
    loanedTo: string

store ^books(id: int): Book
    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string): Id(^books)
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf

    const id: Id(^books) = nextId(^books)
    ^books(id) = book
    return id

pub fn listShelf(shelf: string)
    for id, book in ^books.byShelf(shelf)
        print($"{id}: {book.title} by {book.author}")

pub fn main()
    add(title: "Small Gods", author: "Terry Pratchett", shelf: "fiction")
    add(title: "Sourcery", author: "Terry Pratchett", shelf: "fiction")
    listShelf("fiction")
"#;

const EXPECTED_TEST: &str = r#"module tests::books_test

use shelf::books

pub fn addThenFind()
    const id = books::add(title: "Mort", author: "Terry Pratchett", shelf: "fiction")
    std::assert::isTrue(exists(^books(id)))
    if const title = ^books(id).title
        std::assert::isTrue(title == "Mort")
    else
        std::assert::isTrue(false)
"#;

#[test]
fn init_scaffold_checks_runs_and_tests() {
    assert!(
        QUICKSTART.contains("marrow init shelf"),
        "quickstart should lead with marrow init"
    );
    assert!(
        QUICKSTART.contains(EXPECTED_CONFIG),
        "quickstart config drifted from init scaffold"
    );
    assert!(
        QUICKSTART.contains(EXPECTED_BOOKS),
        "quickstart source drifted from init scaffold"
    );
    assert!(
        QUICKSTART.contains(EXPECTED_TEST),
        "quickstart test drifted from init scaffold"
    );

    let parent = support::temp_dir("init-shelf");
    let target = parent.join("shelf");
    let target_arg = target.to_str().expect("target path utf8");

    let init = marrow(&["init", target_arg]);
    assert_eq!(init.status.code(), Some(0), "{init:?}");
    assert!(
        init.stderr.is_empty(),
        "unexpected stderr: {:?}",
        init.stderr
    );

    assert_eq!(
        fs::read_to_string(target.join("marrow.json")).expect("read config"),
        EXPECTED_CONFIG
    );
    assert_eq!(
        fs::read_to_string(target.join("src/shelf/books.mw")).expect("read source"),
        EXPECTED_BOOKS
    );
    assert_eq!(
        fs::read_to_string(target.join("tests/books_test.mw")).expect("read test"),
        EXPECTED_TEST
    );
    assert_root_entries(&target, &["marrow.json", "src", "tests"]);
    assert!(
        !target.join(".gitignore").exists(),
        "init should not write .gitignore"
    );

    let check = marrow(&["check", target_arg]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    let run = marrow(&["run", target_arg]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    let stdout = String::from_utf8(run.stdout).expect("run stdout utf8");
    assert_eq!(
        stdout,
        "1: Small Gods by Terry Pratchett\n2: Sourcery by Terry Pratchett\n"
    );

    let test = marrow(&["test", target_arg]);
    assert_eq!(test.status.code(), Some(0), "{test:?}");
    let stdout = String::from_utf8(test.stdout).expect("test stdout utf8");
    assert!(
        stdout.contains("ok    tests::books_test::addThenFind"),
        "{stdout}"
    );
}

#[test]
fn init_rejects_invalid_target_module_name_without_writing() {
    let parent = support::temp_dir("init-invalid");
    let target = parent.join("bad-name");
    let output = marrow(&["init", target.to_str().expect("target path utf8")]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config.invalid"), "{stderr}");
    assert!(
        !target.exists(),
        "invalid target name should be rejected before creating files"
    );
}

#[test]
fn init_rejects_qualified_target_module_name_without_writing() {
    let parent = support::temp_dir("init-qualified");
    let target = parent.join("a::b");
    let output = marrow(&["init", target.to_str().expect("target path utf8")]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config.invalid"), "{stderr}");
    assert!(
        !target.exists(),
        "qualified target name should be rejected before creating files"
    );
}

#[cfg(unix)]
#[test]
fn init_rejects_non_utf8_target_without_panicking_or_writing() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let parent = support::temp_dir("init-non-utf8");
    let target_name = OsString::from_vec(b"bad-\xFF".to_vec());
    let target = parent.join(target_name);
    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("init")
        .arg(&target)
        .output()
        .expect("run marrow init");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config.invalid"), "{stderr}");
    assert!(
        !target.exists(),
        "non-UTF8 target should be rejected before creating files"
    );
}

#[test]
fn init_help_and_usage_are_command_line_tier() {
    let help = marrow(&["init", "--help"]);
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    assert!(
        help.stderr.is_empty(),
        "unexpected stderr: {:?}",
        help.stderr
    );
    let stdout = String::from_utf8(help.stdout).expect("help stdout utf8");
    assert!(stdout.contains("marrow init <projectdir>"), "{stdout}");

    let missing = marrow(&["init"]);
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");

    let parent = support::temp_dir("init-usage");
    let first = parent.join("one");
    let second = parent.join("two");
    let duplicate = marrow(&[
        "init",
        first.to_str().expect("first path utf8"),
        second.to_str().expect("second path utf8"),
    ]);
    assert_eq!(duplicate.status.code(), Some(2), "{duplicate:?}");
    assert!(
        !first.exists(),
        "duplicate target should not create first dir"
    );
    assert!(
        !second.exists(),
        "duplicate target should not create second dir"
    );

    let target = parent.join("flags");
    let flag = marrow(&[
        "init",
        "--bogus",
        target.to_str().expect("target path utf8"),
    ]);
    assert_eq!(flag.status.code(), Some(2), "{flag:?}");
    assert!(
        !target.exists(),
        "unknown flag should not create target dir"
    );
}

fn assert_root_entries(root: &Path, expected: &[&str]) {
    let mut entries = fs::read_dir(root)
        .expect("read scaffold root")
        .map(|entry| {
            entry
                .expect("root entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    entries.sort();

    let mut expected = expected
        .iter()
        .map(|entry| (*entry).to_string())
        .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(entries, expected);
}
