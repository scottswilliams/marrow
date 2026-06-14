use crate::support;
use support::{marrow_sub, temp_project, write};

#[test]
fn native_store_persists_writes_across_runs() {
    let root = temp_project("run-native", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Counter\n\
             \x20\x20\x20\x20required value: int\n\
             store ^counter(id: int): Counter\n\
             \n\
             pub fn bump()\n\
             \x20\x20\x20\x20var c: Counter\n\
             \x20\x20\x20\x20c.value = 1\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
             \n\
             pub fn show()\n\
             \x20\x20\x20\x20if not exists(^counter(1))\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print(\"absent\")\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return\n\
             \x20\x20\x20\x20if const value = ^counter(1).value\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print($\"value={value}\")\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    // One process writes the counter; a second process reads it back. Only a
    // persistent store carries the write across the two runs.
    let first = marrow_sub("run", &["--entry", "shelf::bump", &dir]);
    let second = marrow_sub("run", &["--entry", "shelf::show", &dir]);

    assert_eq!(first.status.code(), Some(0), "bump: {first:?}");
    assert_eq!(second.status.code(), Some(0), "show: {second:?}");
    let stdout = String::from_utf8(second.stdout).expect("stdout utf8");
    assert_eq!(stdout, "value=1\n");
}

#[test]
fn refuses_to_run_a_project_that_does_not_check() {
    let root = temp_project("run-badcheck", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" } }"#,
        );
        // The path implies module `shelf::books`, but the file declares another.
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("check.module_path"), "{stderr}");
}

#[test]
fn native_store_requires_a_data_dir() {
    let root = temp_project("run-nodatadir", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" }, "store": { "backend": "native" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn main()\n    print(\"hi\")\n",
        );
    });
    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("config.invalid"), "{stderr}");
}
