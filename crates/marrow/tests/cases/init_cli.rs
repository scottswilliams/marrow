use crate::support;
use std::fs;
use std::path::Path;
use std::process::Command;
use support::{marrow, marrow_in};

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

const EXPECTED_CLIENT_CONFIG: &str = r#"{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::books::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests"],
  "client": "generated/marrow.ts"
}
"#;

const EXPECTED_CLIENT_BOOKS: &str = r#"module shelf::books

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

surface Books from ^books
    fields title, author, shelf
    collection ^books.byShelf as byShelf
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
    // init prints a next-steps block on stdout so a newcomer learns how to run the scaffold.
    let init_stdout = String::from_utf8(init.stdout.clone()).expect("init stdout utf8");
    assert!(
        init_stdout.contains("next steps:")
            && init_stdout.contains("cd ")
            && init_stdout.contains("marrow run ."),
        "init should print next steps (cd, marrow run): {init_stdout}"
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

    let fmt = marrow(&["fmt", "--check", target_arg]);
    assert_eq!(fmt.status.code(), Some(0), "{fmt:?}");

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
fn init_client_scaffolds_surface_and_declared_client() {
    let parent = support::temp_dir("init-client-shelf");
    let target = parent.join("shelf");
    let target_arg = target.to_str().expect("target path utf8");

    let init = marrow(&["init", "--client", target_arg]);
    assert_eq!(init.status.code(), Some(0), "{init:?}");
    assert!(
        init.stderr.is_empty(),
        "unexpected stderr: {:?}",
        init.stderr
    );

    assert_eq!(
        fs::read_to_string(target.join("marrow.json")).expect("read config"),
        EXPECTED_CLIENT_CONFIG,
        "--client must write the client line into marrow.json"
    );
    assert_eq!(
        fs::read_to_string(target.join("src/shelf/books.mw")).expect("read source"),
        EXPECTED_CLIENT_BOOKS,
        "--client must scaffold a surface over ^books"
    );

    // -c is the short alias and must produce the identical scaffold.
    let short_target = parent.join("shelf2");
    let short_arg = short_target.to_str().expect("short target utf8");
    let short = marrow(&["init", "-c", short_arg]);
    assert_eq!(short.status.code(), Some(0), "{short:?}");
    assert_eq!(
        fs::read_to_string(short_target.join("marrow.json")).expect("read short config"),
        EXPECTED_CLIENT_CONFIG.replace("shelf", "shelf2"),
        "-c must match --client"
    );

    // The --client scaffold checks, runs, and keeps its declared client --locked-clean.
    let check = marrow(&["check", target_arg]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "--client scaffold must check: {check:?}"
    );
    let run = marrow(&["run", target_arg]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert!(
        target.join("generated/marrow.ts").exists(),
        "run must emit the declared client"
    );
    let locked = marrow(&["check", "--locked", target_arg]);
    assert_eq!(
        locked.status.code(),
        Some(0),
        "--client scaffold must be --locked clean after run: {locked:?}"
    );
}

#[test]
fn bare_init_writes_no_client_or_surface() {
    let parent = support::temp_dir("init-bare-no-client");
    let target = parent.join("shelf");
    let target_arg = target.to_str().expect("target path utf8");

    let init = marrow(&["init", target_arg]);
    assert_eq!(init.status.code(), Some(0), "{init:?}");
    let config = fs::read_to_string(target.join("marrow.json")).expect("read config");
    assert!(
        !config.contains("\"client\""),
        "bare init must not write a client line: {config}"
    );
    let source = fs::read_to_string(target.join("src/shelf/books.mw")).expect("read source");
    assert!(
        !source.contains("surface "),
        "bare init must not scaffold a surface: {source}"
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
    // The message must teach the naming rule, not just reject: it names the offending name, the
    // identifier rule, and a concrete valid example a user can copy.
    assert!(
        stderr.contains("bad-name"),
        "the message should name the rejected directory name: {stderr}"
    );
    assert!(
        stderr.contains("letter")
            && stderr.contains("underscore")
            && (stderr.contains("digit") || stderr.contains("number")),
        "the message should state the identifier rule (letter/underscore start, then letters, \
         digits, underscores): {stderr}"
    );
    assert!(
        stderr.contains("example") || stderr.contains("e.g.") || stderr.contains("for example"),
        "the message should give a valid-name example: {stderr}"
    );
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

#[test]
fn init_rejects_a_relative_nested_name_even_when_the_parent_exists() {
    // A relative `bad/name` smuggles a path separator into what must be a single module
    // identifier. The parent component existing must not let the separator slip through and
    // silently create a nested project: it is rejected as config.invalid before any write,
    // independent of whether `bad/` exists.
    let parent = support::temp_dir("init-relative-nested");
    fs::create_dir(parent.join("bad")).expect("create existing parent component");

    let output = marrow_in(parent.path(), &["init", "bad/name"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("config.invalid"),
        "a relative nested name must be a config error: {stderr}"
    );
    assert!(
        stderr.contains("separator"),
        "the message must name the path separator as the invalid character: {stderr}"
    );
    assert!(
        !parent.join("bad").join("name").exists(),
        "a relative nested name must be rejected before creating files"
    );
    assert!(
        !parent.join("bad").join("name").join("marrow.json").exists(),
        "no scaffold may be written under the existing parent component"
    );
}

#[test]
fn init_rejects_a_target_whose_parent_directory_is_missing_with_a_config_error() {
    let parent = support::temp_dir("init-missing-parent");
    // A relative-style target whose parent component does not exist must surface a clear
    // config error naming the missing parent, not a raw OS read/write failure.
    let target = parent.join("absent").join("my_app");
    let output = marrow(&["init", target.to_str().expect("target path utf8")]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("config.invalid"),
        "a missing parent must be a config error, not a raw io failure: {stderr}"
    );
    assert!(
        !stderr.contains("os error"),
        "the message must not leak a raw OS error code: {stderr}"
    );
    assert!(
        !target.exists(),
        "a missing-parent target must be rejected before creating files"
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
    assert!(
        stdout.contains("marrow init [--client] <projectdir>"),
        "{stdout}"
    );
    assert!(stdout.contains("--client, -c"), "{stdout}");

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
