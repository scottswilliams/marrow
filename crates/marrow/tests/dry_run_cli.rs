use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    support::commit_catalog_if_clean(&root);
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

/// A native-store project whose entry writes one field inside a `transaction`,
/// plus a reader that dumps the store and a second writer for the dry-vs-real
/// comparison.
const SRC: &str = "module app\n\n\
                   resource Book at ^books(id: int)\n\
                   \x20\x20\x20\x20required title: string\n\
                   \x20\x20\x20\x20pages: int\n\n\
                   pub fn add()\n\
                   \x20\x20\x20\x20transaction\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^books(1).pages = 272\n";

fn native_project(name: &str) -> PathBuf {
    temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::add" } }"#,
        );
        write(root, "src/app.mw", SRC);
    })
}

#[test]
fn dry_run_leaves_the_store_byte_for_byte_unchanged() {
    let project = native_project("dryrun-untouched");
    let dir = project.to_str().unwrap().to_string();

    // The store starts empty. Dump it before the dry run.
    let before = marrow(&["data", "dump", &dir]);
    assert_eq!(before.status.code(), Some(0), "before: {before:?}");

    // A dry run reports the writes it would commit, then rolls them back.
    let dry = marrow(&["run", "--dry-run", &dir]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    let dry_err = String::from_utf8(dry.stderr).expect("utf8");
    // It listed the planned writes to the title and pages fields.
    assert!(dry_err.contains("would write"), "{dry_err}");
    assert!(dry_err.contains("^books(1).title"), "{dry_err}");
    assert!(dry_err.contains("^books(1).pages"), "{dry_err}");

    // The store is unchanged: the dump after matches the dump before.
    let after = marrow(&["data", "dump", &dir]);
    fs::remove_dir_all(&project).ok();
    assert_eq!(after.status.code(), Some(0), "after: {after:?}");
    assert_eq!(
        before.stdout, after.stdout,
        "dry run must leave saved data byte-for-byte unchanged"
    );
}

#[test]
fn dry_run_plan_matches_a_real_run() {
    // The dry run's planned writes are exactly the records a real run commits. Run
    // the entry for real, dump the store, and assert each expected path appears in
    // the real store's dump.
    let dry_project = native_project("dryrun-plan-dry");
    let real_project = native_project("dryrun-plan-real");
    let dry_dir = dry_project.to_str().unwrap().to_string();
    let real_dir = real_project.to_str().unwrap().to_string();

    let dry = marrow(&["run", "--dry-run", "--format", "json", &dry_dir]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    let dry_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8(dry.stdout).expect("utf8")).expect("json");
    assert_eq!(dry_json["committed"], false);
    let planned_paths: Vec<String> = dry_json["planned"]
        .as_array()
        .expect("planned array")
        .iter()
        .filter_map(|step| step["path"].as_str().map(str::to_string))
        .collect();

    // A real run commits the writes; its dump holds those exact field paths.
    assert_eq!(
        marrow(&["run", &real_dir]).status.code(),
        Some(0),
        "real run"
    );
    let real_dump = marrow(&["data", "dump", &real_dir]);
    let real_out = String::from_utf8(real_dump.stdout).expect("utf8");
    fs::remove_dir_all(&dry_project).ok();
    fs::remove_dir_all(&real_project).ok();

    // Every planned field write the dry run reported is present in the real store.
    for path in &planned_paths {
        assert!(
            real_out.contains(path),
            "real run is missing a path the dry run planned: {path}\n{real_out}"
        );
    }
    assert!(
        planned_paths.iter().any(|p| p.contains("title"))
            && planned_paths.iter().any(|p| p.contains("pages")),
        "the plan must cover both field writes: {planned_paths:?}"
    );
}

#[test]
fn dry_run_renders_a_bool_write_as_its_typed_value() {
    // The dry-run text report renders a `bool` leaf write as `true`, not the codec
    // byte `1`, through the same typed-value path the trace uses.
    let project = temp_project("dryrun-bool", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Flag at ^flags(id: int)\n\
             \x20\x20\x20\x20on: bool\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^flags(1).on = true\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(
        stderr.contains("would write ^flags(1).on\ttrue"),
        "a bool must dry-run as `true`, not `1`: {stderr}"
    );
}

#[test]
fn dry_run_reports_maintenance_whole_root_deletes() {
    let project = temp_project("dryrun-root-delete", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20title: string\n\
             \x20\x20\x20\x20shelf: string\n\n\
             \x20\x20\x20\x20index byShelf(shelf, id)\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20^books(1).shelf = \"fiction\"\n\n\
             pub fn dropRoot()\n\
             \x20\x20\x20\x20delete ^books\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let dry = marrow(&[
        "run",
        "--dry-run",
        "--maintenance",
        "--format",
        "json",
        "--entry",
        "app::dropRoot",
        &dir,
    ]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    let dry_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8(dry.stdout).expect("utf8")).expect("json");

    assert_eq!(dry_json["committed"], false);
    assert_eq!(dry_json["writes"], 0);
    assert_eq!(dry_json["deletes"], 2);
    let planned = dry_json["planned"].as_array().expect("planned array");
    assert!(
        planned.iter().any(|step| {
            step["op"] == "delete"
                && step["target"]["kind"] == "data"
                && step["target"]["store"] == "books"
                && step["target"]["identity"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                && step["target"]["path"].as_array().is_some_and(Vec::is_empty)
        }),
        "dry-run report must include the data root delete: {dry_json}"
    );
    assert!(
        planned.iter().any(|step| {
            step["op"] == "delete"
                && step["target"]["kind"] == "index"
                && step["target"]["index"] == "^books.byShelf"
                && step["target"]["keys"].as_array().is_some_and(Vec::is_empty)
                && step["target"]["identity"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
        }),
        "dry-run report must include the index root delete: {dry_json}"
    );

    let dump = marrow(&["data", "dump", &dir]);
    fs::remove_dir_all(&project).ok();
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_out = String::from_utf8(dump.stdout).expect("utf8");
    assert!(
        dump_out.contains("^books(1).title"),
        "dry run must leave the seeded record in place: {dump_out}"
    );
}

#[test]
fn dry_run_reports_non_root_deletes() {
    let project = temp_project("dryrun-nonroot-delete", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20details\n\
             \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\n\
             pub fn seed()\n\
             \x20\x20\x20\x20^books(1).details.note = \"kept\"\n\n\
             pub fn dropDetails()\n\
             \x20\x20\x20\x20delete ^books(1).details\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();

    let seed = marrow(&["run", "--entry", "app::seed", &dir]);
    assert_eq!(seed.status.code(), Some(0), "seed: {seed:?}");

    let dry = marrow(&[
        "run",
        "--dry-run",
        "--format",
        "json",
        "--entry",
        "app::dropDetails",
        &dir,
    ]);
    assert_eq!(dry.status.code(), Some(0), "dry: {dry:?}");
    let dry_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8(dry.stdout).expect("utf8")).expect("json");

    assert_eq!(dry_json["committed"], false);
    assert_eq!(dry_json["writes"], 0);
    assert_eq!(dry_json["deletes"], 1);
    let planned = dry_json["planned"].as_array().expect("planned array");
    assert!(
        planned.iter().any(|step| {
            step["op"] == "delete"
                && step["target"]["kind"] == "data"
                && step["target"]["store"] == "books"
                && step["target"]["identity"]
                    .as_array()
                    .is_some_and(|keys| keys.len() == 1 && keys[0] == "1")
                && step["target"]["path"]
                    .as_array()
                    .is_some_and(|path| path.len() == 1 && path[0]["member"] == "details")
        }),
        "dry-run report must include the group delete: {dry_json}"
    );

    let dump = marrow(&["data", "dump", &dir]);
    fs::remove_dir_all(&project).ok();
    assert_eq!(dump.status.code(), Some(0), "dump: {dump:?}");
    let dump_out = String::from_utf8(dump.stdout).expect("utf8");
    assert!(
        dump_out.contains("^books(1).details.note"),
        "dry run must leave the seeded group data in place: {dump_out}"
    );
}

#[test]
fn dry_run_keeps_the_program_output_on_stdout() {
    // The program's own `print` output still lands on stdout; the dry-run report is
    // separate (stderr under text).
    let project = temp_project("dryrun-stdout", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\n\
             resource Book at ^books(id: int)\n\
             \x20\x20\x20\x20title: string\n\n\
             pub fn main()\n\
             \x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20print(\"ran\")\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", &dir]);
    fs::remove_dir_all(&project).ok();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert_eq!(stdout, "ran\n", "program output must stay on stdout");
    assert!(stderr.contains("rolled back"), "{stderr}");
}

#[test]
fn dry_run_composes_with_trace() {
    // `--dry-run --trace` traces the run and still discards its writes. The trace
    // names the write; the dry-run report says it was rolled back.
    let project = native_project("dryrun-trace");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["run", "--dry-run", "--trace", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("utf8");
    assert!(stderr.contains("^books(1).title"), "trace: {stderr}");
    assert!(stderr.contains("rolled back"), "report: {stderr}");

    // The store is still empty after the composed dry run.
    let dump = marrow(&["data", "dump", &dir]);
    fs::remove_dir_all(&project).ok();
    let dump_out = String::from_utf8(dump.stdout).expect("utf8");
    assert!(
        dump_out.contains("(no saved data)"),
        "store must be empty: {dump_out}"
    );
}
