use crate::support;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use support::temp_source;

fn run_fmt(args: &[&str]) -> std::process::Output {
    support::marrow_sub("fmt", args)
}

fn run_fmt_with_env(args: &[&str], key: &str, value: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg("fmt")
        .args(args)
        .env(key, value)
        .output()
        .expect("run marrow fmt")
}

fn temp_artifacts_for(path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .expect("source file name")
        .to_string_lossy();
    let prefix = format!(".{file_name}.");
    fs::read_dir(parent)
        .expect("read source parent")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
        .collect()
}

/// Human-rendered `fmt` message fragments with no typed code or JSON surface (`fmt` has
/// no `--format json`). Each is a render-contract golden: the behavior it accompanies is
/// asserted by its typed exit status (a `not formatted` finding exits 1, a flag-usage
/// error exits 2), and these fragments only pin the rendered explanation. Regenerate only
/// on an intentional change to the rendered message. The structured-envelope idea is
/// recorded as a backlog item in this lane's design notes.
const NOT_FORMATTED_GOLDEN: &str = "not formatted";
const CHECK_FLAG_GOLDEN: &str = "--check";
const WRITE_FLAG_GOLDEN: &str = "--write";
const STDIN_GOLDEN: &str = "stdin";

/// The diagnostic/error codes `marrow fmt` reports, each read from its structured
/// slot on the stderr line rather than matched anywhere in the rendered prose. `fmt`
/// has no JSON surface, so the dotted code is the typed identifier embedded in the
/// text. Two line shapes carry one: a simple error renders `code: message`, where the
/// code is the leading `: `-delimited segment; a located parse diagnostic renders
/// `file:line:col: severity: code: message`, where the code is the segment right after
/// the `error`/`warning` severity. A `not formatted` finding carries no code. Asserting
/// against these slots keeps the oracle on the code, reword-proof against changes to
/// the human message that follows it.
fn fmt_error_codes(stderr: &[u8]) -> Vec<String> {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    text.lines().filter_map(line_code).collect()
}

fn line_code(line: &str) -> Option<String> {
    let segments: Vec<&str> = line.split(": ").collect();
    // A located diagnostic places the code right after its `error`/`warning` severity.
    if let Some(severity) = segments
        .iter()
        .position(|segment| *segment == "error" || *segment == "warning")
        && let Some(code) = segments.get(severity + 1)
    {
        return is_code(code).then(|| (*code).to_string());
    }
    // Otherwise a simple error leads with `code: message`.
    let first = segments.first()?;
    is_code(first).then(|| (*first).to_string())
}

/// Whether `token` is a diagnostic code: a dotted lowercase identifier carrying no
/// spaces, so a `not formatted` finding or a path fragment never reads as a code.
fn is_code(token: &str) -> bool {
    token.contains('.')
        && !token.contains(' ')
        && token
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '.' || character == '_')
}

fn fmt_reports_code(stderr: &[u8], code: &str) -> bool {
    fmt_error_codes(stderr).iter().any(|found| found == code)
}

#[test]
fn fmt_prints_canonical_source_to_stdout() {
    // Extra blank lines and tight spacing get normalized.
    let path = temp_source(
        "messy",
        "module app\n\n\nconst Max:int=5\nfn main()\n    return Max\n",
    );
    let output = run_fmt(&[path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(
        stdout,
        "module app\n\nconst Max: int = 5\n\nfn main()\n    return Max\n"
    );
}

#[test]
fn fmt_check_succeeds_on_already_formatted_source() {
    let formatted = "module app\n\nconst Max: int = 5\n";
    let path = temp_source("formatted", formatted);
    let output = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(0), "{:?}", output);
}

#[test]
fn fmt_check_fails_on_unformatted_source() {
    let path = temp_source("unformatted", "module app\nconst Max:int=5\n");
    let output = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();
    // The unformatted finding is the typed exit status 1; the rendered explanation is the
    // golden, since `fmt --check` has no structured finding surface.
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(NOT_FORMATTED_GOLDEN), "{stderr}");
}

#[test]
fn fmt_write_rewrites_the_file_in_place() {
    let path = temp_source("write", "module app\nconst Max:int=5\n");
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();
    assert_eq!(written, "module app\n\nconst Max: int = 5\n");
}

#[test]
fn fmt_write_failure_preserves_original_and_removes_temp_file() {
    let source = "module app\nconst Max:int=5\n";
    let path = temp_source("write-atomic-failure", source);
    let output = run_fmt_with_env(
        &["--write", path.to_str().unwrap()],
        "MARROW_TEST_FMT_FAIL_AFTER_BYTES",
        "8",
    );
    let written = fs::read_to_string(&path).expect("read back");
    let temps = temp_artifacts_for(&path);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "io.write"),
        "{:?}",
        output.stderr
    );
    assert_eq!(
        written, source,
        "failed fmt --write must leave the original source byte-for-byte"
    );
    assert_eq!(
        temps,
        Vec::<PathBuf>::new(),
        "failed fmt --write must remove its adjacent temp file"
    );
}

#[test]
fn fmt_write_refuses_to_destroy_retained_comments() {
    let source = "module app\n\
         fn main()\n\
         \x20   throw Error(\n\
         \x20       ; retained rationale\n\
         \x20       code: \"book.absent\",\n\
         \x20       message: \"missing book\",\n\
         \x20   )\n";
    let path = temp_source("write-comment-guard", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "fmt.comment_loss"),
        "{:?}",
        output.stderr
    );
    assert_eq!(
        written, source,
        "fmt --write must leave the original source byte-for-byte"
    );
}

#[test]
fn fmt_write_refuses_to_destroy_evolve_doc_comment_markers() {
    let source = "module app\n\
         evolve\n\
         \x20   ;; keep doc marker\n\
         \x20   rename Book.title -> Book.subtitle\n";
    let path = temp_source("write-evolve-doc-comment-guard", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "fmt.comment_loss"),
        "{:?}",
        output.stderr
    );
    assert_eq!(
        written, source,
        "fmt --write must leave the original source byte-for-byte"
    );
}

#[test]
fn fmt_write_refuses_to_destroy_statement_doc_comment_markers() {
    let source = "module app\n\
         fn main()\n\
         \x20   ;; keep doc marker\n\
         \x20   return\n";
    let path = temp_source("write-statement-doc-comment-guard", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "fmt.comment_loss"),
        "{:?}",
        output.stderr
    );
    assert_eq!(
        written, source,
        "fmt --write must leave the original source byte-for-byte"
    );
}

#[cfg(unix)]
#[test]
fn fmt_write_follows_a_symlinked_source_file() {
    let dir = support::temp_dir("fmt-symlink");
    let target = dir.join("target.mw");
    let link = dir.join("link.mw");
    fs::write(&target, "module app\nconst Max:int=5\n").expect("write target source");
    std::os::unix::fs::symlink("target.mw", &link).expect("create source symlink");

    let output = run_fmt(&["--write", link.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        fs::symlink_metadata(&link)
            .expect("read link metadata")
            .file_type()
            .is_symlink(),
        "fmt --write must preserve the source symlink path"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read formatted target"),
        "module app\n\nconst Max: int = 5\n"
    );
}

#[test]
fn fmt_rejects_check_with_write_without_rewriting() {
    let source = "module app\nconst Max:int=5\n";
    let path = temp_source("check-write", source);
    let output = run_fmt(&["--check", "--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    // Conflicting mode flags are the typed usage error (exit 2); the rendered message
    // names both flags, pinned by the goldens since `fmt` has no structured usage surface.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(written, source);
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(CHECK_FLAG_GOLDEN), "{stderr}");
    assert!(stderr.contains(WRITE_FLAG_GOLDEN), "{stderr}");
}

#[test]
fn fmt_rejects_duplicate_mode_flags() {
    let path = temp_source("dupe-check", "module app\n\nconst Max: int = 5\n");
    let output = run_fmt(&["--check", "--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    // A repeated mode flag is the typed usage error (exit 2); the rendered message names
    // the offending flag, pinned by the golden.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(CHECK_FLAG_GOLDEN), "{stderr}");
}

#[test]
fn fmt_refuses_to_format_source_with_errors() {
    let path = temp_source("broken", "module app\n\tconst Max: int = 5\n");
    let output = run_fmt(&[path.to_str().unwrap()]);
    let unchanged = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        fmt_reports_code(&output.stderr, "parse.syntax"),
        "{:?}",
        output.stderr
    );
    assert_eq!(unchanged, "module app\n\tconst Max: int = 5\n");
}

#[test]
fn fmt_write_refuses_unexpected_indentation_without_rewriting() {
    let source = "module app\nfn main()\n    print(\"kept\")\n        print(\"over-indented\")\n";
    let path = temp_source("over-indented", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let unchanged = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "parse.syntax"),
        "{:?}",
        output.stderr
    );
    assert_eq!(unchanged, source);
}

fn temp_project(name: &str, files: &[(&str, &str)]) -> support::TempProject {
    support::temp_project_uncommitted(name, |root| {
        support::write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        for (relative, source) in files {
            support::write(root, relative, source);
        }
    })
}

#[cfg(unix)]
fn make_readonly(path: impl AsRef<std::path::Path>) {
    let permissions = fs::Permissions::from_mode(0o444);
    fs::set_permissions(path, permissions).expect("make source readonly");
}

#[cfg(unix)]
fn make_writable(path: impl AsRef<std::path::Path>) {
    let permissions = fs::Permissions::from_mode(0o644);
    fs::set_permissions(path, permissions).expect("make source writable");
}

#[test]
fn fmt_check_on_a_project_directory_passes_when_all_files_are_formatted() {
    let project = temp_project(
        "fmt-proj-ok",
        &[
            ("src/app.mw", "module app\n\nconst Max: int = 5\n"),
            ("src/lib.mw", "module lib\n\nconst Limit: int = 10\n"),
        ],
    );
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn fmt_check_on_a_project_directory_fails_when_a_file_is_unformatted() {
    let project = temp_project(
        "fmt-proj-bad",
        &[("src/app.mw", "module app\nconst Max:int=5\n")],
    );
    let output = run_fmt(&["--check", project.to_str().unwrap()]);
    // A directory check fails closed on the first unformatted file: typed exit status 1,
    // with the rendered finding pinned by the golden.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(NOT_FORMATTED_GOLDEN), "{stderr}");
}

#[test]
fn fmt_write_on_a_project_directory_rewrites_each_changed_file() {
    let project = temp_project(
        "fmt-proj-write",
        &[
            ("src/app.mw", "module app\nconst Max:int=5\n"),
            ("src/lib.mw", "module lib\nconst Limit:int=10\n"),
        ],
    );
    let output = run_fmt(&["--write", project.to_str().unwrap()]);
    let app = fs::read_to_string(project.join("src/app.mw")).expect("read app");
    let lib = fs::read_to_string(project.join("src/lib.mw")).expect("read lib");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(app, "module app\n\nconst Max: int = 5\n");
    assert_eq!(lib, "module lib\n\nconst Limit: int = 10\n");
}

#[cfg(unix)]
#[test]
fn fmt_write_reports_file_write_failures_as_io_write() {
    let path = temp_source("write-readonly", "module app\nconst Max:int=5\n");
    make_readonly(&path);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    make_writable(&path);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "io.write"),
        "{:?}",
        output.stderr
    );
    // The failed path is part of the io.write payload; it has no JSON surface here.
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(path.to_str().unwrap()), "{stderr}");
}

#[cfg(unix)]
#[test]
fn fmt_write_on_a_project_directory_reports_write_failures_as_io_write_and_continues() {
    let project = temp_project(
        "fmt-proj-readonly",
        &[
            ("src/app.mw", "module app\nconst Max:int=5\n"),
            ("src/lib.mw", "module lib\nconst Limit:int=10\n"),
        ],
    );
    let readonly = project.join("src/app.mw");
    let writable = project.join("src/lib.mw");
    make_readonly(&readonly);
    let output = run_fmt(&["--write", project.to_str().unwrap()]);
    make_writable(&readonly);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "io.write"),
        "{:?}",
        output.stderr
    );
    // The failed path is part of the io.write payload; it has no JSON surface here.
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(readonly.to_str().unwrap()), "{stderr}");
    let lib = fs::read_to_string(writable).expect("read lib");
    assert_eq!(lib, "module lib\n\nconst Limit: int = 10\n");
}

#[test]
fn fmt_on_a_directory_with_no_config_reports_io_read_for_config() {
    let dir = support::temp_dir("fmt-noconfig");
    let output = run_fmt(&["--check", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "io.read"),
        "{:?}",
        output.stderr
    );
    // The unreadable config path is part of the io.read payload.
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("marrow.json"), "{stderr}");
}

#[test]
fn fmt_on_a_directory_with_invalid_config_reports_a_config_error() {
    let project = temp_project("fmt-proj-badconfig", &[("src/app.mw", "module app\n")]);
    fs::write(project.join("marrow.json"), r#"{ "sourceRoots": [] }"#).expect("write config");
    let output = run_fmt(&["--check", project.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    // An invalid config is reported under a `config.*` code, read from the code slot
    // rather than matched in the message prose.
    assert!(
        fmt_error_codes(&output.stderr)
            .iter()
            .any(|code| code.starts_with("config.")),
        "{:?}",
        output.stderr
    );
}

#[test]
fn fmt_of_a_bare_directory_requires_check_or_write() {
    let project = temp_project("fmt-proj-bare", &[("src/app.mw", "module app\n")]);
    let output = run_fmt(&[project.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn fmt_rejects_stdin_dash_cleanly() {
    let output = run_fmt(&["-"]);
    // Rejecting a `-` stdin argument is the typed usage error (exit 2); the rendered
    // message names stdin, pinned by the golden.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains(STDIN_GOLDEN), "{stderr}");
}
