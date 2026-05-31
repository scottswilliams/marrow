use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
                   \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
                   \n\
                   pub fn show()\n\
                   \x20\x20\x20\x20if not exists(^counter(1))\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20print(\"absent\")\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20return\n\
                   \x20\x20\x20\x20print($\"value={^counter(1).value}\")\n";

fn native_project(name: &str) -> PathBuf {
    native_project_with_source(name, SRC)
}

fn native_project_with_source(name: &str, source: &str) -> PathBuf {
    let root = temp_dir(name);
    write(&root, "marrow.json", CONFIG);
    write(&root, "src/app.mw", source);
    root
}

fn many_counter_records_source(count: usize) -> String {
    let mut source = String::from(
        "module app\n\
         \n\
         resource Counter at ^counter(id: int)\n\
         \x20\x20\x20\x20required value: int\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var c: Counter\n",
    );
    for id in 0..count {
        source.push_str(&format!(
            "\x20\x20\x20\x20c.value = {id}\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^counter({id}) = c\n"
        ));
    }
    source
}

#[test]
fn backup_then_restore_round_trips_saved_data() {
    let source = native_project("backup-src");
    let archive = source.join("data.archive");
    let src = source.to_str().unwrap();
    let arc = archive.to_str().unwrap();

    // Seed the source store, then back it up.
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", src]).status.code(),
        Some(0)
    );
    let backed = marrow(&["backup", src, arc]);
    assert_eq!(backed.status.code(), Some(0), "{backed:?}");

    // Restore the archive into a separate, empty project.
    let target = native_project("backup-dst");
    let dst = target.to_str().unwrap().to_string();
    let restored = marrow(&["restore", &dst, arc]);
    assert_eq!(restored.status.code(), Some(0), "{restored:?}");

    // The restored store carries the data the source had.
    let shown = marrow(&["run", "--entry", "app::show", &dst]);
    fs::remove_dir_all(&source).ok();
    fs::remove_dir_all(&target).ok();
    assert_eq!(shown.status.code(), Some(0), "{shown:?}");
    assert_eq!(String::from_utf8(shown.stdout).expect("utf8"), "value=42\n");
}

#[test]
fn restore_refuses_a_non_empty_target() {
    let project = native_project("backup-nonempty");
    let archive = project.join("data.archive");
    let dir = project.to_str().unwrap().to_string();
    let arc = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    assert_eq!(marrow(&["backup", &dir, &arc]).status.code(), Some(0));

    // The project already holds data, so a normal restore refuses it.
    let output = marrow(&["restore", &dir, &arc]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("restore.not_empty"), "{stderr}");
}

#[test]
fn backup_reports_missing_archive_parent_as_write_error() {
    let project = native_project("backup-missing-parent");
    let dir = project.to_str().unwrap().to_string();
    let archive = project
        .join("missing")
        .join("parent")
        .join("data.archive")
        .to_str()
        .unwrap()
        .to_string();

    let output = marrow(&["backup", &dir, &archive]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("io.write"), "{stderr}");
    assert!(stderr.contains("failed to write"), "{stderr}");
    assert!(!stderr.contains("io.read"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn backup_reports_archive_body_write_failure_as_write_error() {
    let source = many_counter_records_source(3000);
    let project = native_project_with_source("backup-body-write-failure", &source);
    let dir = project.to_str().unwrap().to_string();
    let archive = project.join("archive.fifo");
    let arc = archive.to_str().unwrap().to_string();
    let seeded = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seeded.status.code(), Some(0), "{seeded:?}");
    let mkfifo = Command::new("mkfifo")
        .arg(&archive)
        .output()
        .expect("run mkfifo");
    assert!(mkfifo.status.success(), "{mkfifo:?}");
    let mut reader = Command::new("dd")
        .arg(format!("if={arc}"))
        .arg("bs=20")
        .arg("count=1")
        .arg("of=/dev/null")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fifo reader");

    let output = marrow(&["backup", &dir, &arc]);
    let reader_status = reader.wait().expect("wait for fifo reader");
    fs::remove_dir_all(&project).ok();

    assert!(
        reader_status.success(),
        "fifo reader should consume exactly the archive header"
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("io.write"), "{stderr}");
    assert!(stderr.contains("failed to write"), "{stderr}");
    assert!(!stderr.contains("store.io"), "{stderr}");
    assert!(!stderr.contains("io.read"), "{stderr}");
}

#[test]
fn restore_rejects_a_file_that_is_not_an_archive() {
    let target = native_project("backup-badfile");
    write(&target, "bogus.bin", "not an archive at all");
    let dir = target.to_str().unwrap().to_string();
    let bogus = target.join("bogus.bin").to_str().unwrap().to_string();

    let output = marrow(&["restore", &dir, &bogus]);
    fs::remove_dir_all(&target).ok();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("store.corruption"), "{stderr}");
}
