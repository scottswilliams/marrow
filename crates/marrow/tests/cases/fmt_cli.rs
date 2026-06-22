use crate::support;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
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

/// Human-rendered `fmt` message fragments with no typed code or JSON surface (`fmt` has
/// no `--format json`). Each is a render-contract golden: the behavior it accompanies is
/// asserted by its typed exit status (a `not formatted` finding exits 1, a flag-usage
/// error exits 2), and these fragments only pin the rendered explanation. Regenerate only
/// on an intentional change to the rendered message.
const NOT_FORMATTED_GOLDEN: &str = "not formatted";
const CHECK_FLAG_GOLDEN: &str = "--check";
const WRITE_FLAG_GOLDEN: &str = "--write";
const STDIN_GOLDEN: &str = "stdin";

/// The diagnostic/error codes `marrow fmt` reports, each read from its structured
/// slot on the stderr line rather than matched anywhere in the rendered prose. `fmt`
/// has no JSON surface, so the dotted code is the typed identifier embedded in the
/// text, selected by the shared [`support::is_code`] oracle. A `not formatted` finding
/// and a plain `file:line:col` location carry no code, so a line may contribute none.
fn fmt_error_codes(stderr: &[u8]) -> Vec<String> {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    text.lines()
        .filter_map(|line| {
            line.split(": ")
                .find(|segment| support::is_code(segment))
                .map(str::to_string)
        })
        .collect()
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
    // The finding must teach the fix: the exact write command for this file.
    assert!(
        stderr.contains(&format!("marrow fmt --write {}", path.display())),
        "the not-formatted finding must point at the fixing command: {stderr}"
    );
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
    let temps = support::temp_artifacts_for(&path);
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
fn fmt_write_preserves_multiline_evolve_trailing_comment_placement() {
    let source = "module app\n\
         evolve\n\
         \x20   default Book.info = save(\n\
         \x20       title: \"x\",\n\
         \x20   ) ; default rationale\n";
    let expected = "module app\n\
         \n\
         evolve\n\
         \x20   default Book.info = save(\n\
         \x20       title: \"x\",\n\
         \x20   ) ; default rationale\n";
    let path = temp_source("write-evolve-multiline-comment", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    let check = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(written, expected);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
}

#[test]
fn fmt_write_preserves_header_trailing_comments() {
    let source = "module app ; module rationale\n\
         const Max:int=5 ; const rationale\n\
         resource Book ; resource rationale\n\
         \x20   details ; group rationale\n\
         \x20       required title: string ; field rationale\n\
         store ^books: Book\n\
         \x20   index byTitle(title) ; index rationale\n";
    let expected = "module app ; module rationale\n\
         \n\
         const Max: int = 5 ; const rationale\n\
         \n\
         resource Book ; resource rationale\n\
         \x20   details ; group rationale\n\
         \x20       required title: string ; field rationale\n\
         \n\
         store ^books: Book\n\
         \x20   index byTitle(title) ; index rationale\n";
    let path = temp_source("write-header-comments", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    let check = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(written, expected);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
}

#[test]
fn fmt_write_preserves_multiline_top_level_header_comments() {
    let source = "module app\n\
         const Info = save(\n\
         \x20   title: \"x\",\n\
         ) ; const rationale\n\
         fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         ) ; function rationale\n\
         \x20   return\n";
    let expected = "module app\n\
         \n\
         const Info = save(\n\
         \x20   title: \"x\",\n\
         ) ; const rationale\n\
         \n\
         fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         ) ; function rationale\n\
         \x20   return\n";
    let path = temp_source("write-multiline-header-comments", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    let check = run_fmt(&["--check", path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(written, expected);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
}

#[test]
fn fmt_write_rejects_a_statement_position_doc_comment() {
    // A `;;` doc comment in a statement position has no attachment target and is
    // a parse error, so the source never reaches the formatter. `fmt` refuses
    // with `parse.syntax` and leaves the file byte-for-byte: a program that
    // passes check and runs is always formattable, and an unattachable doc
    // comment is rejected before it can strand a comment the formatter cannot
    // re-emit.
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
        fmt_reports_code(&output.stderr, "parse.syntax"),
        "{:?}",
        output.stderr
    );
    assert_eq!(
        written, source,
        "fmt --write must leave a malformed source byte-for-byte"
    );
}

#[test]
fn fmt_write_rejects_a_doc_comment_in_an_unexpected_indented_block() {
    // An over-indented block whose only content is a `;;` doc comment must still
    // be a parse error: the doc comment has no attachment target, and the
    // statement parser's indented-block recovery rejects it rather than silently
    // retaining it. Without the rejection the program would pass check yet `fmt`
    // would refuse with `fmt.comment_loss`, breaking the check-then-format round
    // trip. The file is left byte-for-byte.
    let source = "module app\n\
         fn main()\n\
         \x20   print(\"a\")\n\
         \x20       ;; keep doc marker\n";
    let path = temp_source("write-indented-block-doc-comment-guard", source);
    let output = run_fmt(&["--write", path.to_str().unwrap()]);
    let written = fs::read_to_string(&path).expect("read back");
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "parse.syntax"),
        "{:?}",
        output.stderr
    );
    assert!(
        !fmt_reports_code(&output.stderr, "fmt.comment_loss"),
        "a parse-rejected source must not reach the comment-loss guard: {:?}",
        output.stderr
    );
    assert_eq!(
        written, source,
        "fmt --write must leave a malformed source byte-for-byte"
    );
}

/// Default stdout `fmt` must agree with `--check`/`--write` on losslessness: a
/// comment that the formatter cannot re-emit (one stranded on a continuation line
/// inside an open delimiter) makes stdout mode refuse with `fmt.comment_loss` and
/// exit non-zero rather than print the comment-stripped source. Otherwise
/// `marrow fmt file > file` would silently destroy the comment. Every paren
/// continuation position the formatter drops is covered so the guard fires for the
/// whole family, not just one shape.
fn assert_stdout_fmt_refuses_comment_loss(name: &str, source: &str) {
    let path = temp_source(name, source);
    let output = run_fmt(&[path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(
        output.status.code(),
        Some(1),
        "default fmt must refuse lossy output for {name}: {output:?}"
    );
    assert!(
        fmt_reports_code(&output.stderr, "fmt.comment_loss"),
        "{name}: {:?}",
        output.stderr
    );
    assert!(
        output.stdout.is_empty(),
        "{name}: a refused lossy format must print nothing to stdout, got {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn fmt_stdout_refuses_comment_loss_inside_call_arguments() {
    assert_stdout_fmt_refuses_comment_loss(
        "stdout-callarg",
        "module app\n\
         fn f(): string\n\
         \x20   return concat(\n\
         \x20       \"a\",  ; keep call\n\
         \x20       \"b\",\n\
         \x20   )\n",
    );
}

#[test]
fn fmt_stdout_refuses_comment_loss_inside_throw_error_arguments() {
    assert_stdout_fmt_refuses_comment_loss(
        "stdout-throwarg",
        "module app\n\
         fn f()\n\
         \x20   throw Error(\n\
         \x20       ; keep throw\n\
         \x20       code: \"x.y\",\n\
         \x20       message: \"m\",\n\
         \x20   )\n",
    );
}

#[test]
fn fmt_stdout_refuses_comment_loss_inside_if_condition_parens() {
    assert_stdout_fmt_refuses_comment_loss(
        "stdout-ifcond",
        "module app\n\
         fn f(): int\n\
         \x20   if (\n\
         \x20       1 == 1  ; keep if\n\
         \x20   )\n\
         \x20       return 1\n\
         \x20   return 0\n",
    );
}

#[test]
fn fmt_stdout_refuses_comment_loss_inside_bool_parens() {
    assert_stdout_fmt_refuses_comment_loss(
        "stdout-boolparen",
        "module app\n\
         fn f(): bool\n\
         \x20   return (\n\
         \x20       true  ; keep bool\n\
         \x20       and false\n\
         \x20   )\n",
    );
}

#[test]
fn fmt_stdout_refuses_comment_loss_between_arguments() {
    assert_stdout_fmt_refuses_comment_loss(
        "stdout-between",
        "module app\n\
         fn f(): string\n\
         \x20   return concat(\n\
         \x20       \"a\",\n\
         \x20       ; keep between\n\
         \x20       \"b\",\n\
         \x20   )\n",
    );
}

#[test]
fn fmt_stdout_refuses_comment_loss_before_close_paren() {
    assert_stdout_fmt_refuses_comment_loss(
        "stdout-beforeclose",
        "module app\n\
         fn f(): string\n\
         \x20   return concat(\n\
         \x20       \"a\",\n\
         \x20       \"b\",\n\
         \x20       ; keep before close\n\
         \x20   )\n",
    );
}

#[test]
fn fmt_stdout_prints_when_all_comments_are_preserved() {
    // Own-line and trailing comments outside open delimiters round-trip, so the
    // guard does not fire and stdout mode prints the formatted source at exit 0.
    let source = "module app\n\
         \n\
         fn main(): int\n\
         \x20   ; own line comment\n\
         \x20   return 5 ; trailing comment\n";
    let path = temp_source("stdout-preserved", source);
    let output = run_fmt(&[path.to_str().unwrap()]);
    fs::remove_file(&path).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, source);
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
fn fmt_on_a_directory_with_no_config_reports_a_missing_project() {
    let dir = support::temp_dir("fmt-noconfig");
    let output = run_fmt(&["--check", dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        fmt_reports_code(&output.stderr, "config.missing"),
        "{:?}",
        output.stderr
    );
    // A directory with no marrow.json names the missing project and points at marrow init.
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("marrow.json") && stderr.contains("marrow init"),
        "{stderr}"
    );
    assert!(!stderr.contains("os error"), "{stderr}");
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
