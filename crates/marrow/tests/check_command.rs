//! `marrow check`: the minimal check surface and the per-export demand sentence.
//!
//! A project travels the real production path through the built binary — capture,
//! the resilient analysis floor, then compile and verify — and each exported function
//! is described by its verifier-reconstructed durable demand in source spelling. The
//! sentence bytes are frozen here: the demand describes which durable places an export
//! reads and writes, and never grants that access.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-a5b01-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn run_check(dir: &Path) -> Output {
    Command::new(MARROW)
        .args(["check"])
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run marrow binary")
}

/// The identity ledger for the bookstore fixture: the application, the `Book` product,
/// its two stored fields, the `books` root and its one key column, and the `byIsbn`
/// unique index each carry a fixed 128-bit id so the graph resolves without a mint.
const BOOKSTORE_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.isbn 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index books.byIsbn 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     high-water 0\n\
     end\n";

/// A two-export bookstore over a keyed root with a unique index. `lookup` is read-only:
/// it reads the index and then the whole entry it names. `put` is a transactional
/// writer: replacing the entry both reads it (to keep `byIsbn` coherent) and writes it.
const BOOKSTORE_SOURCE: &str = r#"resource Book {
    required title: string
    required isbn: string
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
}

pub fn lookup(isbn: string): string? {
    if const found = ^books.byIsbn[isbn] {
        if const b = ^books[found] {
            return b.title
        }
    }
    return absent
}

pub fn put(id: int, title: string, isbn: string) {
    transaction {
        ^books[id] = Book(title: title, isbn: isbn)
    }
}
"#;

/// The frozen demand report for the bookstore fixture. One line per export, in
/// `module.item` order, each naming its durable places in source spelling: the
/// read-only `lookup` reads the whole entry and the index; the transactional `put`
/// reads the entry (unique-index maintenance) and writes it. An index read renders
/// under `reads`, and a place a writer both reads and writes appears in both clauses.
const BOOKSTORE_REPORT: &str = "bookstore.lookup reads ^books and ^books.byIsbn\n\
     bookstore.put reads ^books; writes ^books\n";

/// The check surface prints the exact per-export demand sentences and exits 0 for a
/// project that checks clean.
#[test]
fn check_describes_each_export_demand_in_source_spelling() {
    let dir = TempDir::new("bookstore");
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(&dir.join("marrow.ids"), BOOKSTORE_IDS);
    write(&dir.join("src").join("bookstore.mw"), BOOKSTORE_SOURCE);

    let output = run_check(&dir);
    assert!(
        output.status.success(),
        "check must succeed on a clean project: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), BOOKSTORE_REPORT);
}

/// An explicit project-directory argument checks the same project as the working
/// directory, so the demand report is identical.
#[test]
fn check_accepts_an_explicit_project_directory() {
    let dir = TempDir::new("bookstore-arg");
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(&dir.join("marrow.ids"), BOOKSTORE_IDS);
    write(&dir.join("src").join("bookstore.mw"), BOOKSTORE_SOURCE);

    let output = Command::new(MARROW)
        .args(["check", "."])
        .current_dir(&*dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run marrow binary");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), BOOKSTORE_REPORT);
}

/// A storeless export touches no durable data, and the sentence says exactly that.
#[test]
fn check_describes_a_storeless_export_as_touching_no_durable_data() {
    let dir = TempDir::new("storeless");
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &dir.join("src").join("main.mw"),
        "pub fn answer(): int {\n    return 42\n}\n",
    );

    let output = run_check(&dir);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "main.answer reads or writes no durable data\n"
    );
}

/// A project with a diagnostic reports it with its span and typed code on standard
/// error, writes no demand line, and exits nonzero — the code and span are the
/// contract, not the message prose.
#[test]
fn check_reports_a_diagnostic_with_its_span_and_exits_nonzero() {
    let dir = TempDir::new("broken");
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &dir.join("src").join("main.mw"),
        "pub fn oops(): int {\n    return \"nope\"\n}\n",
    );

    let output = run_check(&dir);
    assert!(!output.status.success(), "a broken project must fail check");
    assert!(
        output.stdout.is_empty(),
        "a failing check prints no demand report: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("src/main.mw:2:12"), "{stderr}");
    assert!(stderr.contains("check.type"), "{stderr}");
}

/// `check` is a live command, not a refounding stub: it never reports
/// `cli.command_unsupported`.
#[test]
fn check_is_not_a_refounding_command() {
    let dir = TempDir::new("live");
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &dir.join("src").join("main.mw"),
        "pub fn answer(): int {\n    return 1\n}\n",
    );

    let output = run_check(&dir);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("cli.command_unsupported"),
        "check must be live: {combined}"
    );
}
