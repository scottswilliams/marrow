//! Composite-identity saved paths in the data CLI. A store keyed by more than
//! one identity column accepts the comma form `^enrolls("s1","c9")`, still
//! accepts the older paren-per-key spelling, and emits the comma form.

use crate::support::{self, TempProject};
use crate::support_data::{json, marrow};

/// A two-column-identity store with one seeded record, `^enrolls("s1","c9").grade = "A"`.
fn composite_project(name: &str) -> (TempProject, String) {
    let project = support::temp_project_uncommitted(name, |root| {
        support::write(root, "marrow.json", support::native_config());
        support::write(
            root,
            "src/app.mw",
            "module app\n\
             \n\
             resource Grade\n\
             \x20\x20\x20\x20required grade: string\n\
             store ^enrolls(student: string, course: string): Grade\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20var g: Grade\n\
             \x20\x20\x20\x20g.grade = \"A\"\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^enrolls(\"s1\", \"c9\") = g\n",
        );
    });
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );
    (project, dir)
}

#[test]
fn data_get_reads_a_composite_record_through_the_comma_form() {
    let (_project, dir) = composite_project("data-composite-get");
    let comma = marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        "^enrolls(\"s1\",\"c9\").grade",
    ]);

    assert_eq!(comma.status.code(), Some(0), "{comma:?}");
    let comma = json(comma);
    assert_eq!(comma["presence"], serde_json::json!("value_only"));
    // The path echoes back as the comma form, so the resolved path round-trips.
    assert_eq!(
        comma["path"],
        serde_json::json!("^enrolls(\"s1\",\"c9\").grade")
    );
    let value =
        marrow_run::base64::decode(comma["value_b64"].as_str().expect("b64")).expect("decode");
    assert_eq!(value, b"A");
}

#[test]
fn data_get_still_accepts_the_old_paren_per_key_composite_form() {
    let (_project, dir) = composite_project("data-composite-get-legacy");
    let legacy = marrow(&[
        "data",
        "get",
        "--format",
        "json",
        &dir,
        "^enrolls(\"s1\")(\"c9\").grade",
    ]);

    assert_eq!(legacy.status.code(), Some(0), "{legacy:?}");
    let legacy = json(legacy);
    assert_eq!(legacy["presence"], serde_json::json!("value_only"));
    // The accepted legacy spelling resolves to the same path, emitted as the comma form.
    assert_eq!(
        legacy["path"],
        serde_json::json!("^enrolls(\"s1\",\"c9\").grade")
    );
}

#[test]
fn data_dump_emits_a_composite_record_in_the_comma_form() {
    let (_project, dir) = composite_project("data-composite-dump");
    let dump = marrow(&["data", "dump", &dir]);

    assert_eq!(dump.status.code(), Some(0), "{dump:?}");
    let stdout = String::from_utf8(dump.stdout).expect("utf8");
    assert_eq!(stdout, "^enrolls(\"s1\",\"c9\").grade\t\"A\"\n", "{stdout}");
}
