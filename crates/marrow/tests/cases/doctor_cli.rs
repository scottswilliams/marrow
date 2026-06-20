use std::fs;
use std::path::{Path, PathBuf};

use crate::support;
use crate::support_data;
use marrow_store::key::SavedKey;
use support::{json, marrow};
use support_data::{
    checked_place, delete_tree_path, field_path, integrity_problem, marrow as data_marrow,
    seeded_project, write_orphan_cell, write_tree_values,
};

const EXPECTED_INTEGRITY_SAMPLE_LIMIT: u64 = 64;

fn store_path(project: &Path) -> PathBuf {
    project.join(".data").join("marrow.redb")
}

fn corrupt_catalog_digest(project: &Path) {
    fs::write(
        project.join("marrow.catalog.json"),
        r#"{
  "epoch": 1,
  "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
  "entries": []
}"#,
    )
    .expect("write corrupt catalog artifact");
}

fn truncate_store_body(project: &Path) {
    let path = store_path(project);
    let len = fs::metadata(&path).expect("store metadata").len();
    assert!(len > 4096, "a seeded store should exceed one redb page");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open store for truncation");
    file.set_len(4096).expect("truncate store body");
}

fn finding_codes(value: &serde_json::Value) -> Vec<&str> {
    value["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|finding| finding["code"].as_str().expect("finding code"))
        .collect()
}

fn finding<'a>(value: &'a serde_json::Value, code: &str) -> &'a serde_json::Value {
    value["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .find(|finding| finding["code"] == serde_json::json!(code))
        .unwrap_or_else(|| panic!("{code} not found in {value:#?}"))
}

fn doctor_integrity_finding<'a>(value: &'a serde_json::Value, dir: &str) -> &'a serde_json::Value {
    let finding = finding(value, "doctor.integrity_sample_failed");
    assert_eq!(
        finding["next_command"],
        serde_json::json!(format!("marrow data integrity {dir}")),
        "{value:#?}"
    );
    finding
}

#[test]
fn doctor_on_a_missing_directory_reports_the_io_read_failure() {
    let missing = support::unique_temp_path("doctor-missing-dir");

    let output = marrow(&["doctor", "--format", "json", missing.to_str().unwrap()]);

    // A missing directory is not a passed-a-file usage error: doctor probes it
    // like run/test, surfacing the unreadable marrow.json as a finding and
    // exiting 1, not short-circuiting with the bare-file guidance and exit 2.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let config_finding = finding(&value, "doctor.config_invalid");
    assert_eq!(
        config_finding["data"]["underlying_code"],
        serde_json::json!("io.read"),
        "{value:#?}"
    );
    assert!(
        config_finding["data"]["path"]
            .as_str()
            .expect("finding path")
            .ends_with("marrow.json"),
        "{value:#?}"
    );
}

#[test]
fn doctor_rejects_a_bare_file_target_as_a_usage_failure() {
    let path = support::temp_source(
        "doctor-file-target",
        r#"module app
pub fn main()
    print("ok")
"#,
    );

    let output = marrow(&["doctor", path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("accepts a project directory") && stderr.contains("marrow.json"),
        "{stderr}"
    );
}

#[test]
fn doctor_aggregates_locked_store_and_corrupt_catalog() {
    let (project, dir) = seeded_project("doctor-lock-catalog");
    corrupt_catalog_digest(&project);
    let _writer = marrow_store::tree::TreeStore::open(&store_path(&project))
        .expect("hold native writer open");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "doctor report should stay on stdout: {:?}",
        output.stderr
    );
    let value = json(output.stdout);
    let codes = finding_codes(&value);
    assert!(codes.contains(&"doctor.store_locked"), "{value:#?}");
    assert!(codes.contains(&"doctor.catalog_invalid"), "{value:#?}");

    let locked = finding(&value, "doctor.store_locked");
    assert_eq!(
        locked["data"]["underlying_code"], "store.locked",
        "{value:#?}"
    );
    assert_eq!(
        locked["next_command"],
        serde_json::json!(format!("marrow doctor {dir}")),
        "{value:#?}"
    );

    let catalog = finding(&value, "doctor.catalog_invalid");
    assert_eq!(
        catalog["data"]["underlying_code"], "catalog.invalid",
        "{value:#?}"
    );
    assert_eq!(
        catalog["next_command"],
        serde_json::json!(format!("marrow check {dir}")),
        "{value:#?}"
    );
}

#[test]
fn doctor_integrity_sample_reports_incomplete_records() {
    let (project, dir) = seeded_project("doctor-integrity-incomplete");
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    delete_tree_path(&project, "counter", &[SavedKey::Int(1)], &value_path);

    let integrity = data_marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    let integrity_value = json(integrity.stdout);
    integrity_problem(&integrity_value, "data.incomplete");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let finding = doctor_integrity_finding(&value, &dir);
    assert_eq!(finding["data"]["problems"], 1, "{value:#?}");
}

#[test]
fn doctor_integrity_sample_reports_orphan_cells() {
    let (project, dir) = seeded_project("doctor-integrity-orphan");
    write_orphan_cell(
        &project,
        "cat_000000000000000000000000deadbeef",
        "cat_00000000000000000000000000000001",
    );

    let integrity = data_marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    let integrity_value = json(integrity.stdout);
    integrity_problem(&integrity_value, "data.orphan");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let finding = doctor_integrity_finding(&value, &dir);
    assert_eq!(finding["data"]["problems"], 1, "{value:#?}");
}

#[test]
fn doctor_store_corruption_uses_non_writing_next_command() {
    let (project, dir) = seeded_project("doctor-store-corruption");
    truncate_store_body(&project);

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let finding = finding(&value, "doctor.store_unavailable");
    assert_eq!(
        finding["data"]["underlying_code"], "store.corruption",
        "{value:#?}"
    );
    assert_eq!(
        finding["next_command"],
        serde_json::json!(format!("marrow doctor {dir}")),
        "{value:#?}"
    );
}

#[test]
fn doctor_reports_no_findings_for_healthy_project() {
    let (_project, dir) = seeded_project("doctor-healthy");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        output.stderr
    );
    let value = json(output.stdout);
    assert_eq!(value["status"], "ok", "{value:#?}");
    assert_eq!(value["findings"], serde_json::json!([]), "{value:#?}");
    assert_eq!(
        value["integrity_sample"]["limit"], EXPECTED_INTEGRITY_SAMPLE_LIMIT,
        "{value:#?}"
    );
    assert_eq!(value["integrity_sample"]["truncated"], false, "{value:#?}");
}

#[test]
fn doctor_integrity_sample_is_bounded() {
    let (project, dir) = seeded_project("doctor-integrity-cap");
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    let identities =
        (1..=EXPECTED_INTEGRITY_SAMPLE_LIMIT + 1).map(|id| vec![SavedKey::Int(id as i64)]);
    write_tree_values(&project, &place, identities, &value_path, b"42");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output.stdout);
    assert_eq!(
        value["integrity_sample"]["limit"], EXPECTED_INTEGRITY_SAMPLE_LIMIT,
        "{value:#?}"
    );
    assert_eq!(
        value["integrity_sample"]["items_checked"], EXPECTED_INTEGRITY_SAMPLE_LIMIT,
        "{value:#?}"
    );
    assert_eq!(value["integrity_sample"]["truncated"], true, "{value:#?}");
    assert_eq!(value["findings"], serde_json::json!([]), "{value:#?}");
}

#[test]
fn doctor_integrity_sample_uses_one_shared_budget() {
    let (project, dir) = seeded_project("doctor-integrity-shared-cap");
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    let identities =
        (1..=EXPECTED_INTEGRITY_SAMPLE_LIMIT + 1).map(|id| vec![SavedKey::Int(id as i64)]);
    write_tree_values(&project, &place, identities, &value_path, b"42");
    write_orphan_cell(
        &project,
        "cat_000000000000000000000000deadbeef",
        "cat_00000000000000000000000000000001",
    );

    let integrity = data_marrow(&["data", "integrity", "--format", "json", &dir]);
    assert_eq!(integrity.status.code(), Some(1), "{integrity:?}");
    let integrity_value = json(integrity.stdout);
    integrity_problem(&integrity_value, "data.orphan");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output.stdout);
    assert_eq!(
        value["integrity_sample"]["items_checked"], EXPECTED_INTEGRITY_SAMPLE_LIMIT,
        "{value:#?}"
    );
    assert_eq!(value["integrity_sample"]["truncated"], true, "{value:#?}");
    assert_eq!(value["findings"], serde_json::json!([]), "{value:#?}");
}

#[test]
fn doctor_text_reports_truncated_clean_sample() {
    let (project, dir) = seeded_project("doctor-text-truncated-sample");
    let place = checked_place(&project, "counter");
    let value_path = field_path(&place, "value");
    let identities =
        (1..=EXPECTED_INTEGRITY_SAMPLE_LIMIT + 1).map(|id| vec![SavedKey::Int(id as i64)]);
    write_tree_values(&project, &place, identities, &value_path, b"42");
    write_orphan_cell(
        &project,
        "cat_000000000000000000000000deadbeef",
        "cat_00000000000000000000000000000001",
    );

    let output = marrow(&["doctor", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("doctor stdout utf8");
    assert!(
        stdout.contains(&format!("ok: {dir} doctor found no findings")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "sample truncated after {EXPECTED_INTEGRITY_SAMPLE_LIMIT} items; run marrow data integrity {dir} for the full read-only report"
        )),
        "{stdout}"
    );
    assert!(
        !stdout.contains("doctor.integrity_sample_failed"),
        "{stdout}"
    );
}
