//! End-to-end tests for the project-capture CLI surface: `marrow init` and
//! `marrow fmt <projectdir>`, driven through the built binary so discovery,
//! manifest parsing, and formatting travel the real production path.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

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

#[test]
fn init_creates_a_manifest_and_src_tree() {
    let temp = TempDir::new("init");
    let project = temp.join("app");

    let output = run(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");

    let manifest = fs::read_to_string(project.join("marrow.toml")).expect("manifest written");
    assert_eq!(manifest, "edition = \"2026\"\n");
    assert!(project.join("src").join("main.mw").is_file());
    assert!(
        !project.join("marrow.lock").exists(),
        "init creates no store artifacts"
    );
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
fn fmt_project_checks_and_writes_every_module() {
    let temp = TempDir::new("fmt-project");
    let project = temp.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    // A deliberately misformatted module (extra blank lines the formatter removes).
    write(
        &project.join("src").join("main.mw"),
        "pub fn main()\n    return\n\n\n",
    );
    write(
        &project.join("src").join("util").join("helper.mw"),
        "pub fn help()\n    return\n",
    );

    // --check reports the unformatted module and fails.
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
        "pub fn main()\n    return\n"
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
        "pub fn main()\n    return\n",
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
        "pub fn x()\n    return\n",
    );
    write(
        &project.join("src").join("a.b.mw"),
        "pub fn y()\n    return\n",
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
    let module = "pub fn main()\n    return\n\n";
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

#[cfg(unix)]
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
        "pub fn main()\n    return\n",
    );

    // An unformatted target the symlink points at, outside the walked tree.
    let outside = temp.join("outside.mw");
    write(&outside, "pub fn stray()\n    return\n\n\n");
    std::os::unix::fs::symlink(&outside, project.join("src").join("linked.mw"))
        .expect("create symlink");

    let output = run(&["fmt", "--check", project.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "the symlinked unformatted file must be skipped: {output:?}"
    );
}
