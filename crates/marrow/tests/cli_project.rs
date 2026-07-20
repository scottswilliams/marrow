//! End-to-end tests for the project-capture CLI surface: `marrow init` and
//! `marrow fmt <projectdir>`, driven through the built binary so discovery,
//! manifest parsing, and formatting travel the real production path.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Child, ExitStatus, Stdio};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::{Duration, Instant};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");
const VALID_MANIFEST: &str = "edition = \"2026\"\n";
const FORMATTED_SOURCE: &str = "pub fn main() {\n    return\n}\n";
const UNFORMATTED_SOURCE: &str = "pub fn main() {\n        return\n}\n";
const EMPTY_IDS: &str =
    "marrow ids v0\nmachine-written by marrow; do not edit\nhigh-water 0\nend\n";
const MANIFEST_BYTES_LIMIT: usize = 1 << 20;
const SOURCE_FILE_BYTES_LIMIT: u64 = 1 << 20;
const SOURCE_TOTAL_BYTES_LIMIT: u64 = 64 << 20;
const SOURCE_FILE_COUNT_LIMIT: usize = 4_096;
const VISITED_ENTRY_LIMIT: usize = 65_536;
const SOURCE_DEPTH_LIMIT: usize = 64;

/// A temporary directory removed when dropped, even on a failing assertion.
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
            std::env::temp_dir().join(format!("marrow-b01-{name}-{}-{nanos}", std::process::id()));
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

fn run(args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .output()
        .expect("run marrow binary")
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn project(root: &Path, source: &str) {
    write(&root.join("marrow.toml"), VALID_MANIFEST);
    write(&root.join("src").join("main.mw"), source);
}

fn assert_empty_streams(output: &Output) {
    assert!(output.stdout.is_empty(), "command wrote stdout: {output:?}");
    assert!(output.stderr.is_empty(), "command wrote stderr: {output:?}");
}

fn assert_io_read(output: &Output) {
    assert!(!output.status.success(), "capture must fail: {output:?}");
    assert!(output.stdout.is_empty(), "failure wrote stdout: {output:?}");
    let stderr = std::str::from_utf8(&output.stderr).expect("io.read stderr is valid UTF-8");
    assert!(stderr.starts_with("io.read: "), "{stderr}");
    assert!(stderr.ends_with('\n'), "stderr record lacks LF: {stderr:?}");
    assert_eq!(
        stderr.bytes().filter(|byte| *byte == b'\n').count(),
        1,
        "io.read must be exactly one stderr record: {stderr:?}"
    );
}

fn assert_refused_without_writing(path: &Path, before: &str, output: &Output) {
    assert_eq!(
        fs::read_to_string(path).expect("read source after refusal"),
        before,
        "capture refusal must precede every formatter write"
    );
    assert_io_read(output);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct ChildGuard {
    child: Option<Child>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl ChildGuard {
    fn spawn(command: &mut Command) -> Self {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        Self {
            child: Some(command.spawn().expect("spawn guarded child")),
        }
    }

    fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .as_mut()
            .expect("child remains owned")
            .try_wait()
            .expect("poll guarded child")
    }

    fn kill(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if child.try_wait().expect("poll before kill").is_some() {
            return;
        }
        if let Err(error) = child.kill()
            && child
                .try_wait()
                .expect("poll child after raced kill")
                .is_none()
        {
            panic!("kill guarded child: {error}");
        }
    }

    fn finish(mut self) -> Output {
        self.child
            .take()
            .expect("child remains owned")
            .wait_with_output()
            .expect("collect guarded child")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                child.kill().ok();
            }
            child.wait().ok();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct PermissionGuard {
    path: PathBuf,
    original_mode: u32,
    restored: bool,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl PermissionGuard {
    fn make_searchable_only(path: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let original_mode = fs::metadata(path)
            .expect("inspect directory permissions")
            .permissions()
            .mode()
            & 0o7777;
        fs::set_permissions(path, fs::Permissions::from_mode(0o111))
            .expect("make directory searchable only");
        Self {
            path: path.to_path_buf(),
            original_mode,
            restored: false,
        }
    }

    fn restore(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&self.path, fs::Permissions::from_mode(self.original_mode))
            .expect("restore directory permissions");
        self.restored = true;
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for PermissionGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        if !self.restored {
            fs::set_permissions(&self.path, fs::Permissions::from_mode(self.original_mode)).ok();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn create_fifo(path: &Path) {
    let output = Command::new("/usr/bin/mkfifo")
        .arg(path)
        .output()
        .expect("run mkfifo");
    assert!(output.status.success(), "mkfifo failed: {output:?}");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_with_fifo(project: &Path, fifo: &Path, payload: &str, after_open: &Path) -> Output {
    let mut writer_command = Command::new("/bin/sh");
    writer_command.args([
        "-c",
        "exec 3>\"$1\"; : >\"$2\"; printf '%s' \"$3\" >&3; exec 3>&-",
        "marrow-fifo-writer",
        fifo.to_str().expect("FIFO path is UTF-8"),
        after_open.to_str().expect("sentinel path is UTF-8"),
        payload,
    ]);
    let mut writer = ChildGuard::spawn(&mut writer_command);

    let mut cli_command = Command::new(MARROW);
    cli_command.args(["fmt", "--write", project.to_str().unwrap()]);
    let mut cli = ChildGuard::spawn(&mut cli_command);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let cli_status = cli.try_wait();
        let writer_status = writer.try_wait();
        match cli_status {
            Some(status) if !status.success() => {
                writer.kill();
                break;
            }
            Some(_) if writer_status.is_some() => break,
            _ if Instant::now() >= deadline => {
                cli.kill();
                writer.kill();
                panic!(
                    "FIFO journey exceeded its deadline: cli={cli_status:?}, writer={writer_status:?}"
                );
            }
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }

    let writer_output = writer.finish();
    let cli_output = cli.finish();
    if cli_output.status.success() {
        assert!(
            writer_output.status.success(),
            "current successful FIFO journey needs a successful writer: {writer_output:?}"
        );
    }
    cli_output
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_with_deadline(args: &[&str]) -> Output {
    let mut command = Command::new(MARROW);
    command.args(args);
    let mut child = ChildGuard::spawn(&mut command);
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().is_none() {
        if Instant::now() >= deadline {
            child.kill();
            panic!("CLI journey exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.finish()
}

#[test]
fn init_creates_a_manifest_and_src_tree() {
    let temp = TempDir::new("init");
    let project = temp.join("app");

    let output = run(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");

    let manifest = fs::read_to_string(project.join("marrow.toml")).expect("manifest written");
    assert_eq!(manifest, "edition = \"2026\"\n");
    assert_eq!(
        fs::read(project.join("src").join("main.mw")).expect("starter script written"),
        b"pub fn main() {\n    return\n}\n"
    );
    assert!(
        !project.join("marrow.lock").exists(),
        "init creates no store artifacts"
    );
}

#[test]
fn project_help_describes_captured_source_files_and_the_headerless_script() {
    let root_help = run(&["--help"]);
    assert!(root_help.status.success(), "{root_help:?}");
    let root_stdout = String::from_utf8(root_help.stdout).expect("root help is UTF-8");
    assert!(
        root_stdout.contains("every captured source file"),
        "{root_stdout}"
    );

    let init_help = run(&["init", "--help"]);
    assert!(init_help.status.success(), "{init_help:?}");
    let init_stdout = String::from_utf8(init_help.stdout).expect("init help is UTF-8");
    assert!(init_stdout.contains("headerless script"), "{init_stdout}");

    let fmt_help = run(&["fmt", "--help"]);
    assert!(fmt_help.status.success(), "{fmt_help:?}");
    let fmt_stdout = String::from_utf8(fmt_help.stdout).expect("fmt help is UTF-8");
    assert!(
        fmt_stdout.contains("every captured source file"),
        "{fmt_stdout}"
    );
}

#[test]
fn source_owner_wording_does_not_restore_the_stale_module_script_law() {
    const OWNER_SOURCES: &[(&str, &str)] = &[
        (
            "crates/marrow-project/src/identity.rs",
            include_str!("../../marrow-project/src/identity.rs"),
        ),
        (
            "crates/marrow-project/AGENTS.md",
            include_str!("../../marrow-project/AGENTS.md"),
        ),
        ("crates/marrow/src/main.rs", include_str!("../src/main.rs")),
        (
            "crates/marrow/src/cmd_init.rs",
            include_str!("../src/cmd_init.rs"),
        ),
        (
            "crates/marrow/src/cmd_fmt.rs",
            include_str!("../src/cmd_fmt.rs"),
        ),
    ];
    const STALE_PHRASES: &[&str] = &[
        "every module of a project",
        "every module of the project",
        "starter module",
        "unformatted module",
        "no in-source module header",
        "single-file fallback",
    ];

    for (path, source) in OWNER_SOURCES {
        let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
        for stale in STALE_PHRASES {
            assert!(
                !normalized.contains(stale),
                "{path} retains stale source-owner wording: {stale}"
            );
        }
    }
}

#[test]
fn a_fresh_project_is_already_formatted() {
    let temp = TempDir::new("init-fmt");
    let project = temp.join("app");
    assert!(run(&["init", project.to_str().unwrap()]).status.success());

    let output = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "fresh project must pass fmt --check: {output:?}"
    );
}

#[test]
fn init_refuses_an_existing_directory() {
    let temp = TempDir::new("init-existing");
    let project = temp.join("app");
    fs::create_dir(&project).expect("pre-create");

    let output = run(&["init", project.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config.invalid"), "{stderr}");
    assert!(stderr.contains("already exists"), "{stderr}");
}

#[test]
fn a_failed_init_leaves_no_debris_and_a_retry_succeeds() {
    // A failure after the exclusive claim must unwind it: otherwise the partial
    // directory blocks every retry with AlreadyExists. The debug build injects a
    // post-claim scaffold failure through MARROW_TEST_INIT_FAIL_SCAFFOLD.
    let temp = TempDir::new("init-unwind");
    let project = temp.join("app");

    let failed = Command::new(MARROW)
        .args(["init", project.to_str().unwrap()])
        .env("MARROW_TEST_INIT_FAIL_SCAFFOLD", "1")
        .output()
        .expect("run marrow binary");
    assert!(!failed.status.success(), "injected failure must fail init");
    assert!(
        !project.exists(),
        "a failed init must remove its claimed directory"
    );

    let retried = run(&["init", project.to_str().unwrap()]);
    assert!(retried.status.success(), "retry must succeed: {retried:?}");
    assert!(project.join("marrow.toml").is_file());
}

#[test]
fn fmt_project_checks_and_writes_every_captured_source_file() {
    let temp = TempDir::new("fmt-project");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    // A deliberately misformatted source file (over-indented body the formatter normalizes
    // back to the canonical single indent).
    write(
        &project.join("src").join("main.mw"),
        "pub fn main() {\n        return\n}\n",
    );
    write(
        &project.join("src").join("util").join("helper.mw"),
        r#"pub fn help() {
    return
}
"#,
    );

    // --check reports the unformatted source file and fails.
    let checked = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(
        !checked.status.success(),
        "unformatted project must fail --check"
    );

    // --write reformats it.
    let written = run(&["fmt", "--write", project.to_str().unwrap()]);
    assert!(written.status.success(), "{written:?}");
    assert_eq!(
        fs::read_to_string(project.join("src").join("main.mw")).unwrap(),
        r#"pub fn main() {
    return
}
"#
    );

    // --check now passes.
    assert!(
        run(&["fmt", "--check", project.to_str().unwrap()])
            .status
            .success()
    );
}

#[test]
fn fmt_project_reports_an_invalid_manifest() {
    let temp = TempDir::new("fmt-bad-manifest");
    let project = temp.join("app");
    write(
        &project.join("marrow.toml"),
        "edition = \"2026\"\nname = \"app\"\n",
    );
    write(
        &project.join("src").join("main.mw"),
        r#"pub fn main() {
    return
}
"#,
    );

    let output = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config.invalid"), "{stderr}");
}

#[test]
fn fmt_project_reports_a_module_collision() {
    let temp = TempDir::new("fmt-collision");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &project.join("src").join("a").join("b.mw"),
        r#"pub fn x() {
    return
}
"#,
    );
    write(
        &project.join("src").join("a.b.mw"),
        r#"pub fn y() {
    return
}
"#,
    );

    let output = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("project.module_collision"), "{stderr}");
}

#[test]
fn relocation_produces_identical_formatted_bytes() {
    // The same source under two different roots must format identically, because
    // module identity is derived from the root-relative path, not the location.
    let module = r#"pub fn main() {
    return
}
"#;
    let first = TempDir::new("reloc-a");
    let second = TempDir::new("reloc-b");
    for root in [&*first, &*second] {
        write(&root.join("marrow.toml"), "edition = \"2026\"\n");
        write(&root.join("src").join("main.mw"), module);
        assert!(
            run(&["fmt", "--write", root.to_str().unwrap()])
                .status
                .success()
        );
    }
    let a = fs::read(first.join("src").join("main.mw")).unwrap();
    let b = fs::read(second.join("src").join("main.mw")).unwrap();
    assert_eq!(a, b);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_symlinked_src_root_is_refused_and_external_files_stay_untouched() {
    // The containment blocker: if `src` itself is a symlink to an external
    // directory, following it would let capture escape the project tree and let
    // `fmt --write` rewrite external files in place. The adapter must refuse a
    // symlinked source root with a typed code and touch nothing.
    let temp = TempDir::new("symlink-root");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");

    // An external tree with an unformatted source file, reachable only through the
    // symlinked root.
    let external = temp.join("external");
    let stray = r#"pub fn stray() {
    return
}
"#;
    write(&external.join("main.mw"), stray);
    std::os::unix::fs::symlink(&external, project.join("src")).expect("symlink src");

    let checked = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(!checked.status.success(), "symlinked src must be refused");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("project.source_path"), "{stderr}");

    let written = run(&["fmt", "--write", project.to_str().unwrap()]);
    assert!(
        !written.status.success(),
        "symlinked src must refuse --write"
    );
    assert_eq!(
        fs::read_to_string(external.join("main.mw")).unwrap(),
        stray,
        "the external file must remain byte-identical"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_symlinked_source_file_is_not_followed() {
    // A symlink inside src is skipped by the physical adapter, so an unformatted
    // file reached only through a symlink does not fail a project that is
    // otherwise fully formatted.
    let temp = TempDir::new("symlink");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &project.join("src").join("main.mw"),
        r#"pub fn main() {
    return
}
"#,
    );

    // An unformatted target the symlink points at, outside the walked tree.
    let outside = temp.join("outside.mw");
    write(
        &outside,
        r#"pub fn stray() {
    return
}
"#,
    );
    std::os::unix::fs::symlink(&outside, project.join("src").join("linked.mw"))
        .expect("create symlink");

    let output = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "the symlinked unformatted file must be skipped: {output:?}"
    );
}

#[test]
fn a_manifest_over_the_physical_byte_bound_is_refused_before_formatting() {
    let temp = TempDir::new("manifest-bound");
    let source_path = temp.join("src/main.mw");
    project(&temp, UNFORMATTED_SOURCE);

    let target_len = MANIFEST_BYTES_LIMIT + 1;
    let mut manifest = String::with_capacity(target_len);
    manifest.push_str(VALID_MANIFEST);
    manifest.push('#');
    manifest.extend(std::iter::repeat_n(
        'x',
        target_len
            .checked_sub(VALID_MANIFEST.len() + 2)
            .expect("manifest has room for comment framing"),
    ));
    manifest.push('\n');
    assert_eq!(manifest.len(), target_len);
    write(&temp.join("marrow.toml"), &manifest);

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert_refused_without_writing(&source_path, UNFORMATTED_SOURCE, &output);
}

#[test]
fn a_missing_manifest_at_a_nested_root_keeps_its_exact_io_read_record() {
    let temp = TempDir::new("missing-nested-manifest");
    let project_root = temp.join("nested/project");
    let retained = project_root.join("src/main.mw");
    write(&retained, UNFORMATTED_SOURCE);
    let manifest_path = project_root.join("marrow.toml");
    let os_error = fs::read_to_string(&manifest_path).expect_err("manifest remains absent");

    let output = run(&["fmt", "--write", project_root.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(&retained).expect("read source after refusal"),
        UNFORMATTED_SOURCE,
        "a missing manifest must refuse before formatting"
    );
    assert!(output.stdout.is_empty(), "failure wrote stdout: {output:?}");
    assert!(!output.status.success(), "missing manifest must fail");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "io.read: failed to read {}: {os_error}\n",
            manifest_path.display()
        )
    );
}

#[test]
fn a_located_malformed_manifest_keeps_its_exact_cli_record() {
    const MALFORMED_MANIFEST: &str = "edition = [\n";

    let temp = TempDir::new("located-malformed-manifest");
    let retained = temp.join("src/main.mw");
    project(&temp, UNFORMATTED_SOURCE);
    let manifest_path = temp.join("marrow.toml");
    write(&manifest_path, MALFORMED_MANIFEST);
    let error = marrow_project::Manifest::parse(MALFORMED_MANIFEST)
        .expect_err("fixture must be malformed TOML");
    let position = error.position().expect("malformed TOML has a location");

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(&retained).expect("read source after refusal"),
        UNFORMATTED_SOURCE,
        "manifest failure must precede formatter writes"
    );
    assert!(output.stdout.is_empty(), "failure wrote stdout: {output:?}");
    assert!(!output.status.success(), "malformed manifest must fail");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "{}:{}:{}: {}: {}\n",
            manifest_path.display(),
            position.line,
            position.column,
            error.code().as_str(),
            error.message()
        )
    );
}

#[test]
fn a_visited_entry_over_the_physical_bound_is_refused_before_retention() {
    let temp = TempDir::new("visited-bound");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let source_root = temp.join("src");
    fs::create_dir(&source_root).expect("create source root");
    for index in 0..VISITED_ENTRY_LIMIT {
        fs::File::create(source_root.join(format!("ignored-{index:05}")))
            .expect("create ignored source-tree entry");
    }
    // The selected source is the 65,537th total visited entry. Keep the
    // ignored-entry count at exactly the 65,536-entry production limit.
    let retained = source_root.join("main.mw");
    write(&retained, UNFORMATTED_SOURCE);

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert_refused_without_writing(&retained, UNFORMATTED_SOURCE, &output);
}

#[test]
fn a_directory_beyond_the_physical_depth_bound_is_refused_before_descent() {
    let temp = TempDir::new("depth-bound");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let retained = temp.join("src/00-retained.mw");
    write(&retained, UNFORMATTED_SOURCE);
    let mut deepest = temp.join("src");
    deepest.push("deep");
    fs::create_dir(&deepest).expect("create first nested source directory");
    for _ in 1..=SOURCE_DEPTH_LIMIT {
        deepest.push("d");
        fs::create_dir(&deepest).expect("create nested source directory");
    }

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert_refused_without_writing(&retained, UNFORMATTED_SOURCE, &output);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_symlinked_manifest_is_refused_before_formatting() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new("manifest-symlink");
    let project_root = temp.join("app");
    let source_path = project_root.join("src/main.mw");
    write(&source_path, UNFORMATTED_SOURCE);
    let outside = temp.join("outside.toml");
    write(&outside, VALID_MANIFEST);
    symlink(&outside, project_root.join("marrow.toml")).expect("symlink manifest");

    let output = run(&["fmt", "--write", project_root.to_str().unwrap()]);
    assert_refused_without_writing(&source_path, UNFORMATTED_SOURCE, &output);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_hardlinked_manifest_is_refused_before_formatting() {
    let temp = TempDir::new("manifest-hardlink");
    let project_root = temp.join("app");
    let source_path = project_root.join("src/main.mw");
    write(&source_path, UNFORMATTED_SOURCE);
    let outside = temp.join("outside.toml");
    write(&outside, VALID_MANIFEST);
    fs::hard_link(&outside, project_root.join("marrow.toml")).expect("hardlink manifest");

    let output = run(&["fmt", "--write", project_root.to_str().unwrap()]);
    assert_refused_without_writing(&source_path, UNFORMATTED_SOURCE, &output);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_hardlinked_identity_ledger_is_refused_before_formatting() {
    let temp = TempDir::new("ids-hardlink");
    let source_path = temp.join("src/main.mw");
    project(&temp, UNFORMATTED_SOURCE);
    let outside = temp.join("outside.ids");
    write(&outside, EMPTY_IDS);
    fs::hard_link(&outside, temp.join("marrow.ids")).expect("hardlink identity ledger");

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert_refused_without_writing(&source_path, UNFORMATTED_SOURCE, &output);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_hardlinked_selected_source_is_refused_before_formatting() {
    let temp = TempDir::new("source-hardlink");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let outside = temp.join("outside.mw");
    write(&outside, UNFORMATTED_SOURCE);
    let source_path = temp.join("src/main.mw");
    fs::create_dir_all(source_path.parent().unwrap()).expect("create source root");
    fs::hard_link(&outside, &source_path).expect("hardlink source");

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert_refused_without_writing(&outside, UNFORMATTED_SOURCE, &output);
    assert_eq!(
        fs::read_to_string(&source_path).expect("read selected source"),
        UNFORMATTED_SOURCE
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_manifest_fifo_is_refused_without_waiting_for_its_body() {
    let temp = TempDir::new("manifest-fifo");
    let retained = temp.join("src/main.mw");
    write(&retained, UNFORMATTED_SOURCE);
    let fifo = temp.join("marrow.toml");
    create_fifo(&fifo);
    let after_open = temp.join("manifest.after-open");

    let output = run_with_fifo(&temp, &fifo, VALID_MANIFEST, &after_open);
    let without_writer = run_with_deadline(&["fmt", "--write", temp.to_str().unwrap()]);
    assert!(
        !after_open.exists(),
        "refusing the manifest FIFO must not let the writer open it"
    );
    assert_refused_without_writing(&retained, UNFORMATTED_SOURCE, &output);
    assert_refused_without_writing(&retained, UNFORMATTED_SOURCE, &without_writer);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn an_identity_ledger_fifo_is_refused_without_waiting_for_its_body() {
    let temp = TempDir::new("ids-fifo");
    let retained = temp.join("src/main.mw");
    project(&temp, UNFORMATTED_SOURCE);
    let fifo = temp.join("marrow.ids");
    create_fifo(&fifo);
    let after_open = temp.join("ids.after-open");

    let output = run_with_fifo(&temp, &fifo, EMPTY_IDS, &after_open);
    let without_writer = run_with_deadline(&["fmt", "--write", temp.to_str().unwrap()]);
    assert!(
        !after_open.exists(),
        "refusing the identity-ledger FIFO must not let the writer open it"
    );
    assert_refused_without_writing(&retained, UNFORMATTED_SOURCE, &output);
    assert_refused_without_writing(&retained, UNFORMATTED_SOURCE, &without_writer);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_searchable_but_unreadable_root_is_refused_at_physical_admission() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new("search-only-root");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    assert!(
        !temp.join("src").exists(),
        "fixture must have no source role"
    );
    assert!(
        !temp.join("marrow.ids").exists(),
        "fixture must have no identity-ledger role"
    );
    let mut permissions = PermissionGuard::make_searchable_only(&temp);

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    permissions.restore();
    assert_eq!(
        fs::metadata(&*temp)
            .expect("inspect restored directory permissions")
            .permissions()
            .mode()
            & 0o7777,
        permissions.original_mode,
        "the normal path must restore permissions before fixture cleanup"
    );
    drop(permissions);
    if output.status.success() {
        assert_empty_streams(&output);
        panic!("the shared adapter must refuse a root it cannot retain: {output:?}");
    }
    assert_io_read(&output);
}

#[test]
fn a_manifest_only_project_with_no_source_root_is_a_silent_noop() {
    let temp = TempDir::new("manifest-only");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    assert_eq!(
        fs::read_dir(&*temp).expect("read project root").count(),
        1,
        "fixture must begin with only the manifest"
    );

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "a missing optional source root must succeed: {output:?}"
    );
    assert_empty_streams(&output);
    assert_eq!(
        fs::read_dir(&*temp).expect("read project root").count(),
        1,
        "fmt --check must not create a source root or identity ledger"
    );
    assert!(!temp.join("src").exists());
    assert!(!temp.join("marrow.ids").exists());
}

#[test]
fn an_identity_ledger_over_its_byte_bound_keeps_its_exact_refusal() {
    let temp = TempDir::new("ids-byte-bound");
    let source_path = temp.join("src/main.mw");
    project(&temp, UNFORMATTED_SOURCE);
    let ids_path = temp.join("marrow.ids");
    let oversized_bytes = marrow_project::MAX_IDS_BYTES + 1;
    let ids = fs::File::create(&ids_path).expect("create oversized identity ledger");
    ids.set_len(u64::try_from(oversized_bytes).expect("identity bound fits u64"))
        .expect("size oversized identity ledger");

    let output = run(&["fmt", "--write", temp.to_str().unwrap()]);
    assert!(!output.status.success(), "oversized ledger must fail");
    assert!(output.stdout.is_empty(), "failure wrote stdout: {output:?}");
    assert_eq!(
        fs::read_to_string(&source_path).expect("read source after refusal"),
        UNFORMATTED_SOURCE,
        "identity-ledger refusal must precede formatter writes"
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "project.ids_corrupt: {} is {oversized_bytes} bytes, over the {}-byte identity-artifact bound\n",
            ids_path.display(),
            marrow_project::MAX_IDS_BYTES
        )
    );
}

#[test]
fn an_under_bound_non_source_entry_is_ignored_with_exact_silent_output() {
    const IGNORED_BYTES: &str = "ordinary file; not Marrow source\n";

    let temp = TempDir::new("ignored-regular");
    let source_path = temp.join("src/main.mw");
    let ignored_path = temp.join("src/ignored.txt");
    project(&temp, FORMATTED_SOURCE);
    write(&ignored_path, IGNORED_BYTES);

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "an ordinary non-source entry must remain ignored: {output:?}"
    );
    assert_empty_streams(&output);
    assert_eq!(
        fs::read_to_string(source_path).expect("read selected source"),
        FORMATTED_SOURCE
    );
    assert_eq!(
        fs::read_to_string(ignored_path).expect("read ignored entry"),
        IGNORED_BYTES
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_symlinked_identity_ledger_retains_its_existing_typed_refusal() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new("ids-symlink");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let outside = temp.join("outside.ids");
    write(&outside, EMPTY_IDS);
    symlink(&outside, temp.join("marrow.ids")).expect("symlink identity ledger");

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "linked ledger must fail: {output:?}"
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "project.ids_corrupt: {} is a symlink; the identity artifact must be a real file inside the project\n",
            temp.join("marrow.ids").display()
        )
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_symlinked_source_directory_remains_ignored() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new("source-dir-symlink");
    project(&temp, FORMATTED_SOURCE);
    let outside = temp.join("outside");
    write(&outside.join("unformatted.mw"), UNFORMATTED_SOURCE);
    symlink(&outside, temp.join("src/linked")).expect("symlink nested source directory");

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "a linked directory below src remains ignored: {output:?}"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_symlinked_project_root_alias_remains_accepted() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new("root-alias");
    let project_root = temp.join("project");
    project(&project_root, FORMATTED_SOURCE);
    let alias = temp.join("alias");
    symlink(&project_root, &alias).expect("symlink project root alias");

    let output = run(&["fmt", "--check", alias.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "root alias must remain accepted: {output:?}"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_fifo_below_src_remains_an_ignored_special_entry() {
    let temp = TempDir::new("ignored-source-fifo");
    project(&temp, FORMATTED_SOURCE);
    create_fifo(&temp.join("src/ignored.mw"));

    let output = run_with_deadline(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "an ignored special entry must never be opened: {output:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_non_utf8_source_path_retains_its_existing_exact_refusal() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = TempDir::new("non-utf8-source");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let path = temp
        .join("src")
        .join(OsString::from_vec(b"bad\xff.mw".to_vec()));
    fs::create_dir_all(path.parent().unwrap()).expect("create source root");
    fs::write(&path, FORMATTED_SOURCE).expect("write non-UTF-8 source path");

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(!output.status.success(), "non-UTF-8 source path must fail");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "project.source_path: source path {} is not valid UTF-8\n",
            path.display()
        )
    );
}

#[test]
fn existing_source_file_byte_bound_keeps_its_exact_cli_rendering() {
    let temp = TempDir::new("source-file-bound");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let path = temp.join("src/main.mw");
    fs::create_dir_all(path.parent().unwrap()).expect("create source root");
    let file = fs::File::create(&path).expect("create oversized source");
    file.set_len(SOURCE_FILE_BYTES_LIMIT + 1)
        .expect("size oversized source");

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(!output.status.success(), "oversized source must fail");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "project.capture_limit: `src/main.mw` capture is {}, over the per-file byte limit ({SOURCE_FILE_BYTES_LIMIT})\n",
            SOURCE_FILE_BYTES_LIMIT + 1
        )
    );
}

#[test]
fn existing_source_file_count_bound_keeps_its_exact_cli_rendering() {
    let temp = TempDir::new("source-file-count");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let source_root = temp.join("src");
    fs::create_dir(&source_root).expect("create source root");
    for index in 0..=SOURCE_FILE_COUNT_LIMIT {
        fs::File::create(source_root.join(format!("{index:04}.mw")))
            .expect("create bounded source");
    }
    let offender = source_root.join(format!("{SOURCE_FILE_COUNT_LIMIT:04}.mw"));

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(!output.status.success(), "too many sources must fail");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "project.capture_limit: `{}` capture is {}, over the source-file limit ({SOURCE_FILE_COUNT_LIMIT})\n",
            offender.display(),
            SOURCE_FILE_COUNT_LIMIT + 1
        )
    );
}

#[test]
fn existing_source_total_byte_bound_keeps_its_exact_cli_rendering() {
    let temp = TempDir::new("source-total-bound");
    write(&temp.join("marrow.toml"), VALID_MANIFEST);
    let source_root = temp.join("src");
    fs::create_dir(&source_root).expect("create source root");
    let full_files = SOURCE_TOTAL_BYTES_LIMIT / SOURCE_FILE_BYTES_LIMIT;
    for index in 0..full_files {
        let path = source_root.join(format!("{index:04}.mw"));
        let file = fs::File::create(path).expect("create full bounded source");
        file.set_len(SOURCE_FILE_BYTES_LIMIT)
            .expect("size full bounded source");
    }
    let offender = source_root.join(format!("{full_files:04}.mw"));
    fs::write(&offender, [0]).expect("write limit-plus-one byte");

    let output = run(&["fmt", "--check", temp.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "source total over bound must fail"
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!(
            "project.capture_limit: `src/{full_files:04}.mw` capture is {}, over the project byte limit ({SOURCE_TOTAL_BYTES_LIMIT})\n",
            SOURCE_TOTAL_BYTES_LIMIT + 1
        )
    );
}

/// Run the built binary with `dir` as its working directory, so `run`, `test`, and
/// `client` capture the project there (each captures the current directory).
fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run marrow binary")
}

/// The unlocated `io.read` message a missing-manifest capture renders. The binary
/// reads `./marrow.toml` relative to its working directory, so the path is the
/// exact working-directory-relative spelling regardless of where `dir` lives.
fn missing_manifest_io_read_message(dir: &Path) -> String {
    let os_error = fs::read_to_string(dir.join("marrow.toml")).expect_err("manifest is absent");
    format!("io.read: failed to read ./marrow.toml: {os_error}")
}

const RUN_ERROR_JSONL: &str = "{\"code\":\"io.read\",\"kind\":\"run\",\"outcome\":\"error\"}\n";

#[test]
fn client_reports_an_unlocated_capture_failure_on_styled_stderr() {
    let temp = TempDir::new("client-capture");
    let expected = missing_manifest_io_read_message(&temp);
    let output = run_in(&temp, &["client", "typescript"]);
    assert!(
        output.stdout.is_empty(),
        "client capture wrote stdout: {output:?}"
    );
    assert!(!output.status.success(), "a missing manifest must fail");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        format!("{expected}\n")
    );
}

#[test]
fn run_reports_an_unlocated_capture_failure_on_stdout_text() {
    let temp = TempDir::new("run-capture-text");
    let expected = missing_manifest_io_read_message(&temp);
    let output = run_in(&temp, &["run", "main"]);
    assert!(
        output.stderr.is_empty(),
        "run capture wrote stderr: {output:?}"
    );
    assert!(!output.status.success(), "a missing manifest must fail");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        format!("{expected}\n")
    );
}

#[test]
fn run_reports_a_capture_failure_as_one_jsonl_record() {
    let temp = TempDir::new("run-capture-jsonl");
    let output = run_in(&temp, &["run", "main", "--format", "jsonl"]);
    assert!(
        output.stderr.is_empty(),
        "run jsonl wrote stderr: {output:?}"
    );
    assert!(!output.status.success(), "a missing manifest must fail");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        RUN_ERROR_JSONL
    );
}

#[test]
fn test_reports_an_unlocated_capture_failure_on_stdout_text() {
    let temp = TempDir::new("test-capture-text");
    let expected = missing_manifest_io_read_message(&temp);
    let output = run_in(&temp, &["test"]);
    assert!(
        output.stderr.is_empty(),
        "test capture wrote stderr: {output:?}"
    );
    assert!(!output.status.success(), "a missing manifest must fail");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        format!("{expected}\n")
    );
}

#[test]
fn test_reports_a_capture_failure_as_one_jsonl_record_with_run_kind() {
    let temp = TempDir::new("test-capture-jsonl");
    let output = run_in(&temp, &["test", "--format", "jsonl"]);
    assert!(
        output.stderr.is_empty(),
        "test jsonl wrote stderr: {output:?}"
    );
    assert!(!output.status.success(), "a missing manifest must fail");
    // `test` intentionally retains the current `kind:"run"`.
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        RUN_ERROR_JSONL
    );
}

#[test]
fn run_renders_a_located_manifest_fault_as_an_unlocated_record() {
    // `run`, `test`, and `client` render only the code and message; the manifest
    // location a located fault carries is dropped, unlike `fmt`.
    let temp = TempDir::new("run-located-manifest");
    write(&temp.join("marrow.toml"), "edition = [\n");
    let error = marrow_project::Manifest::parse("edition = [\n").expect_err("malformed");
    let output = run_in(&temp, &["run", "main"]);
    assert!(
        output.stderr.is_empty(),
        "run located manifest wrote stderr: {output:?}"
    );
    assert!(!output.status.success(), "a malformed manifest must fail");
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        format!("{}: {}\n", error.code().as_str(), error.message())
    );
}
