use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIBRARY_SOURCE: &str = include_str!("../../../fixtures/v01/library.mw");

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    TempProject { root }
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

#[test]
fn v01_library_fixture_checks_and_runs_through_cli() {
    let root = temp_project("v01-library-cli", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(root, "src/v01/library.mw", LIBRARY_SOURCE);
    });
    let dir = root.path().to_str().unwrap().to_string();

    let check = marrow(&["check", &dir]);
    let seed = marrow(&["run", "--entry", "v01::library::seed", &dir]);
    let print_author = marrow(&["run", "--entry", "v01::library::printSeededAuthor", &dir]);
    let print_stdout = std::str::from_utf8(&print_author.stdout).expect("stdout utf8");

    assert_eq!(check.status.code(), Some(0), "check: {check:?}");
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");
    assert_eq!(
        print_author.status.code(),
        Some(0),
        "print author: {print_author:?}"
    );
    assert_eq!(print_stdout, "Ursula K. Le Guin\n");
}
