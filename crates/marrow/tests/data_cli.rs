use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create dir");
    root
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

const CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;
const SRC: &str = "module app\n\
                   \n\
                   resource Counter at ^counter(id: int)\n\
                   \x20\x20\x20\x20required value: int\n\
                   \n\
                   pub fn seed()\n\
                   \x20\x20\x20\x20var c: Counter\n\
                   \x20\x20\x20\x20c.value = 42\n\
                   \x20\x20\x20\x20transaction\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n";

fn native_project(name: &str) -> PathBuf {
    let root = temp_dir(name);
    write(&root, "marrow.json", CONFIG);
    write(&root, "src/app.mw", SRC);
    root
}

#[test]
fn data_roots_lists_the_saved_roots() {
    let project = native_project("data-roots");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    let output = marrow(&["data", "roots", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("^counter"), "{stdout}");
}

#[test]
fn data_stats_counts_roots_and_records() {
    let project = native_project("data-stats");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    let output = marrow(&["data", "stats", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("roots: 1"), "{stdout}");
    assert!(
        stdout.contains("records: ") && !stdout.contains("records: 0"),
        "{stdout}"
    );
}

#[test]
fn inspecting_an_unseeded_project_reports_no_data_and_creates_nothing() {
    let project = native_project("data-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "roots", &dir]);
    // Inspection is read-only: it must not create the store file.
    let created = project.join(".data").join("marrow.redb").exists();
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("(no saved data)"), "{stdout}");
    assert!(!created, "inspection must not create the store");
}
