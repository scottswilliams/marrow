//! `marrow data roots|stats|dump`: the saved-data inventory commands. Structured
//! formats are asserted as parsed JSON/JSONL fields; the text format is a render
//! contract pinned by explicitly-marked prose.

use marrow_store::key::SavedKey;

mod support;
mod support_data;

use support_data::{
    checked_place, field_path, json, marrow, native_project, seeded_project, write_tree_value,
};

#[test]
fn data_roots_lists_stored_roots() {
    // Render contract: the text format prints one `^root` per saved root. The typed
    // root list is asserted by `data_roots_format_json_emits_a_structured_envelope`.
    let (_project, dir) = seeded_project("data-roots");
    let output = marrow(&["data", "roots", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    // Render contract: the text format prints exactly one `^root` line per saved root.
    assert_eq!(stdout, "^counter\n", "{stdout}");
}

#[test]
fn data_stats_counts_roots_and_records() {
    // Render contract: the text format prints `roots:`/`records:` count lines. The
    // typed counts are asserted by `data_stats_format_json_emits_counts`.
    let (_project, dir) = seeded_project("data-stats");
    let output = marrow(&["data", "stats", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    // Render contract: the two count lines, pinned exactly.
    assert_eq!(stdout, "roots: 1\nrecords: 1\n", "{stdout}");
}

#[test]
fn inspecting_an_unseeded_project_reports_no_data_and_creates_nothing() {
    let project = native_project("data-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "roots", &dir]);
    // Inspection is read-only: it must not create the store file.
    let created = project.join(".data").join("marrow.redb").exists();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Render contract: an empty store prints a human placeholder, not a bare line.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "(no saved data)\n", "{stdout}");
    assert!(!created, "inspection must not create the store");
}

#[test]
fn data_dump_prints_each_record_as_path_and_value() {
    // Render contract: the text format prints `<path>\t<value>` for the one record.
    // The typed record is asserted by `data_dump_format_jsonl_emits_a_record_then_a_summary`.
    let (_project, dir) = seeded_project("data-dump");
    let output = marrow(&["data", "dump", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    // Render contract: one `<path>\t<value>` line for the single record, pinned exactly.
    assert_eq!(stdout, "^counter(1).value\t42\n", "{stdout}");
}

#[test]
fn data_dump_of_an_unseeded_project_prints_empty_and_creates_nothing() {
    let project = native_project("data-dump-empty");
    let dir = project.to_str().unwrap().to_string();
    let output = marrow(&["data", "dump", &dir]);
    let created = project.join(".data").join("marrow.redb").exists();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Render contract: an empty store dump prints a human placeholder.
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(stdout, "(no saved data)\n", "{stdout}");
    assert!(!created, "dump must not create the store");
}

#[test]
fn data_roots_format_json_emits_a_structured_envelope() {
    let (_project, dir) = seeded_project("data-roots-json");
    let output = marrow(&["data", "roots", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output);
    assert_eq!(value["project"], serde_json::json!(dir));
    assert_eq!(value["roots"], serde_json::json!(["counter"]));
}

#[test]
fn data_stats_format_json_emits_counts() {
    let (_project, dir) = seeded_project("data-stats-json");
    let output = marrow(&["data", "stats", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output);
    assert_eq!(value["roots"], serde_json::json!(1));
    assert_eq!(value["records"], serde_json::json!(1));
}

#[test]
fn data_dump_format_jsonl_emits_a_record_then_a_summary() {
    let (_project, dir) = seeded_project("data-dump-jsonl");
    let output = marrow(&["data", "dump", "--format", "jsonl", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("record json");
    assert_eq!(record["path"], serde_json::json!("^counter(1).value"));
    assert!(record["value_b64"].is_string(), "{record}");
    let summary: serde_json::Value = serde_json::from_str(lines[1]).expect("summary json");
    assert_eq!(summary["kind"], serde_json::json!("summary"));
    assert_eq!(summary["records"], serde_json::json!(1));
}

#[test]
fn data_commands_page_through_large_native_store() {
    const RECORDS: usize = 150;

    let project = native_project("data-paged");
    let dir = project.to_str().unwrap().to_string();
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    for id in 1..=RECORDS {
        write_tree_value(
            &project,
            "counter",
            &[SavedKey::Int(id as i64)],
            &value_path,
            b"7".to_vec(),
        );
    }

    let stats = marrow(&["data", "stats", "--format", "json", &dir]);
    let dump = marrow(&["data", "dump", "--format", "jsonl", &dir]);
    let integrity = marrow(&["data", "integrity", "--format", "json", &dir]);

    assert_eq!(stats.status.code(), Some(0), "{stats:?}");
    let stats_json: serde_json::Value = serde_json::from_slice(&stats.stdout).expect("stats json");
    assert_eq!(stats_json["records"], serde_json::json!(RECORDS));

    assert_eq!(dump.status.code(), Some(0), "{dump:?}");
    let dump_stdout = String::from_utf8(dump.stdout).expect("dump utf8");
    let lines = dump_stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), RECORDS + 1);
    let summary: serde_json::Value = serde_json::from_str(lines[RECORDS]).expect("summary json");
    assert_eq!(summary["records"], serde_json::json!(RECORDS));

    // Paging through every record finds no integrity problems, asserted as the typed
    // empty problem list rather than the rendered record-count text.
    assert_eq!(integrity.status.code(), Some(0), "{integrity:?}");
    let integrity_json: serde_json::Value =
        serde_json::from_slice(&integrity.stdout).expect("integrity json");
    assert_eq!(integrity_json["records"], serde_json::json!(RECORDS));
    assert_eq!(integrity_json["problems"], serde_json::json!([]));
}
