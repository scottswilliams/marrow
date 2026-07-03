use std::fs;
use std::path::{Path, PathBuf};

use crate::support;
use crate::support_data;
use marrow_store::key::SavedKey;
use marrow_store::{AccessMode, SealedStore};
use support::{json, marrow};
use support_data::{
    checked_place, delete_tree_path, field_path, integrity_problem, marrow as data_marrow,
    native_project, seeded_project, write_orphan_cell, write_tree_values,
};

const EXPECTED_INTEGRITY_SAMPLE_LIMIT: u64 = 64;

fn store_path(project: &Path) -> PathBuf {
    project.join(".data").join("marrow.redb")
}

fn lock_path(project: &Path) -> PathBuf {
    project.join("marrow.lock")
}

fn committed_lock(project: &Path) -> marrow_catalog::CatalogLock {
    marrow_check::read_committed_lock(project)
        .expect("read committed lock")
        .expect("project has a committed lock")
}

/// Overwrite the committed lock with bytes the production lock reader rejects, leaving the
/// stamped store untouched. Doctor reports `doctor.lock_corrupt` rather than treating the
/// corrupt lock as absent.
fn corrupt_lock(project: &Path) {
    fs::write(lock_path(project), "{ this is not a valid lock")
        .expect("write corrupt committed lock");
}

/// Rewrite the committed lock so it shares the store's epoch high-water but records a different
/// shape: one entry's fingerprint is flipped while every stable id and the epoch stay fixed. The
/// store stamp and rows are left exactly as the seed wrote them, so doctor sees the same epoch on
/// both sides and a divergent shape — the collision doctor must report against the live store.
fn drift_lock_shape_same_epoch(project: &Path) {
    let committed = committed_lock(project);
    let mut entries = committed.entries.clone();
    let first = entries
        .first_mut()
        .expect("a committed lock entry to drift");
    first.shape_fingerprint = "sha256:".to_string() + &"f".repeat(64);
    let drifted = marrow_catalog::CatalogLock::new(
        entries,
        committed.ledger.clone(),
        committed.epoch_high_water,
        committed.source_digest.clone(),
    )
    .expect("drifted lock builds");
    fs::write(
        lock_path(project),
        drifted.to_lock_json_pretty().expect("lock renders"),
    )
    .expect("write shape-drifted committed lock");
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
fn doctor_on_a_missing_directory_reports_a_missing_project() {
    let missing = support::unique_temp_path("doctor-missing-dir");

    let output = marrow(&["doctor", "--format", "json", missing.to_str().unwrap()]);

    // A missing directory is not a passed-a-file usage error: doctor probes it
    // like run/test, surfacing the missing project as a finding and exiting 1,
    // not short-circuiting with the bare-file guidance and exit 2.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let config_finding = finding(&value, "doctor.config_invalid");
    assert_eq!(
        config_finding["data"]["underlying_code"],
        serde_json::json!("config.missing"),
        "{value:#?}"
    );
    let message = config_finding["data"]["message"]
        .as_str()
        .expect("finding message");
    assert!(
        message.contains("marrow init") && !message.contains("os error"),
        "the missing-project finding must point at marrow init with no raw OS error: {value:#?}"
    );
    // The remedy and next command are derived from the typed fault: a missing project is created
    // by `marrow init`, never the self-defeating loop back through `marrow doctor`.
    let remedy = config_finding["remedy"].as_str().expect("remedy string");
    assert!(
        remedy.contains("marrow init"),
        "the missing-project remedy names the working init action: {remedy}"
    );
    assert_eq!(
        config_finding["next_command"],
        serde_json::json!(format!("marrow init {}", missing.to_str().unwrap())),
        "the next command creates the project rather than looping doctor: {value:#?}"
    );
}

#[test]
fn doctor_reports_a_bare_file_target_as_not_a_project() {
    let path = support::temp_source(
        "doctor-file-target",
        r#"module app
pub fn main()
    print("ok")
"#,
    );

    let output = marrow(&["doctor", "--format", "json", path.to_str().unwrap()]);

    fs::remove_file(&path).ok();
    // A bare file is the same not-a-project condition run/check surface: doctor probes it like a
    // missing directory, reporting the `config.not_a_project` finding and exiting 1, never a
    // command-local usage failure and never a raw `os error` leaked through the lock read.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let config_finding = finding(&value, "doctor.config_invalid");
    assert_eq!(
        config_finding["data"]["underlying_code"],
        serde_json::json!("config.not_a_project"),
        "{value:#?}"
    );
    let message = config_finding["data"]["message"]
        .as_str()
        .expect("finding message");
    assert!(
        message.contains("bare file")
            && message.contains("marrow.json")
            && !message.contains("os error")
            && !message.contains("marrow init"),
        "the not-a-project finding names the bare file with no errno and no init remedy: {value:#?}"
    );
    assert!(
        !value.to_string().contains("os error"),
        "no doctor finding may leak a raw OS errno for a bare-file path: {value:#?}"
    );
    // A bare file cannot be turned into a project in place, so the remedy names that mistake and
    // the next command never loops doctor back at the same bare-file path.
    let remedy = config_finding["remedy"].as_str().expect("remedy string");
    assert!(
        remedy.contains("project directory") && !remedy.contains("marrow init"),
        "the not-a-project remedy names the bare-file mistake without an init remedy: {remedy}"
    );
    let next = config_finding["next_command"]
        .as_str()
        .expect("next_command string");
    assert!(
        !next.contains(path.to_str().unwrap()),
        "the next command must not loop back at the bare-file path: {next}"
    );
    // The remedy is an actionable pointer at a real project directory, not a self-referential
    // `marrow doctor` re-run that only re-reports the not-a-project fault.
    assert!(
        next.contains("marrow check") && !next.contains("marrow doctor"),
        "the not-a-project next command points check at a real project directory, never loops doctor: {next}"
    );
}

#[test]
fn doctor_on_an_invalid_config_field_names_the_field_fix_not_a_loop() {
    // A marrow.json with a field that fails validation (a `dataDir` escaping the project root)
    // loads as `config.invalid`. The remedy must point at fixing the reported field in place, and
    // the next command re-validates that fix through `marrow check` — never a self-referential
    // `marrow doctor` on the same directory, which only re-reports the same fault.
    let project = support::temp_project_uncommitted("doctor-invalid-config", |root| {
        support::write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": "../escape" } }"#,
        );
        support::write(root, "src/app.mw", "module app\n\nfn main()\n    return\n");
    });
    let dir = project.path().to_str().expect("project path utf8");

    let output = marrow(&["doctor", "--format", "json", dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let config_finding = finding(&value, "doctor.config_invalid");
    assert_eq!(
        config_finding["data"]["underlying_code"],
        serde_json::json!("config.invalid"),
        "{value:#?}"
    );
    let remedy = config_finding["remedy"].as_str().expect("remedy string");
    assert!(
        remedy.contains("field") && !remedy.contains("marrow init"),
        "the invalid-config remedy names the field fix, not an init: {remedy}"
    );
    assert_eq!(
        config_finding["next_command"],
        serde_json::json!(format!("marrow check {dir}")),
        "the next command re-validates the field fix through check: {value:#?}"
    );
    let next = config_finding["next_command"]
        .as_str()
        .expect("next_command string");
    assert!(
        !next.contains("marrow doctor"),
        "the next command must not loop doctor over the same directory: {next}"
    );
}

#[test]
fn doctor_aggregates_locked_store_and_corrupt_lock() {
    let (project, dir) = seeded_project("doctor-lock-corrupt");
    corrupt_lock(&project);
    let _writer = SealedStore::open(&store_path(&project), AccessMode::Create)
        .expect("hold native writer open")
        .into_store();

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
    assert!(
        codes.contains(&"doctor.lock_corrupt"),
        "a corrupt committed lock aggregates with the locked store: {value:#?}"
    );

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

    let corrupt = finding(&value, "doctor.lock_corrupt");
    assert_eq!(
        corrupt["data"]["underlying_code"],
        marrow_catalog::LOCK_CORRUPT,
        "{value:#?}"
    );
    // A run over a corrupt lock fails closed and does NOT regenerate it, so the remedy must
    // advise deleting the corrupt lock — the only action that unblocks the next run to re-project
    // it from the authoritative store. It must not loop the operator back through a run that
    // cannot succeed against the corrupt lock.
    let remedy = corrupt["remedy"].as_str().expect("remedy is a string");
    assert!(
        remedy.contains("delete") && remedy.contains("marrow.lock"),
        "the lock_corrupt remedy must advise deleting the corrupt lock: {remedy}"
    );
    assert!(
        !remedy.contains("regenerate marrow.lock with a run"),
        "the remedy must not advise a run that fails closed over the corrupt lock: {remedy}"
    );
}

#[test]
fn doctor_reports_same_epoch_different_digest_collision_against_lock_and_store_without_advising_file_repair()
 {
    let (project, dir) = seeded_project("doctor-catalog-collision");

    // The committed lock and the stamped store agree on epoch but disagree on shape after the
    // lock's shape fingerprint is flipped. Capture the store stamp and rows, then the drifted lock
    // bytes, so the repairs-nothing invariant can be checked byte-for-byte after doctor runs.
    let store_before = fs::read(store_path(&project)).expect("read store before doctor");
    let store_snapshot = {
        let store = SealedStore::open(&store_path(&project), AccessMode::Read)
            .expect("open store read-only")
            .into_store();
        store
            .read_catalog_snapshot()
            .expect("read store catalog snapshot")
            .expect("a stamped store carries an accepted catalog")
    };
    drift_lock_shape_same_epoch(&project);
    let committed = committed_lock(&project);
    let lock_before = fs::read(lock_path(&project)).expect("read drifted committed lock");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    let value = json(output.stdout);
    let codes = finding_codes(&value);
    assert!(
        codes.contains(&"doctor.catalog_collision"),
        "doctor reports the same-epoch-different-digest collision: {value:#?}"
    );
    assert!(
        !codes.contains(&"doctor.catalog_drift"),
        "the old file-over-store drift finding is gone: {value:#?}"
    );

    // No finding advises a file restore — asserted structurally over the code set, never over
    // prose. The retired drift remedy was the only file-restore remedy doctor emitted.
    assert!(
        !codes.contains(&"doctor.catalog_invalid") && !codes.contains(&"doctor.catalog_unreadable"),
        "doctor advises no file-restore remedy: {value:#?}"
    );

    let collision = finding(&value, "doctor.catalog_collision");
    assert_eq!(
        collision["data"]["lock_epoch"],
        serde_json::json!(committed.epoch_high_water),
        "{value:#?}"
    );
    assert_eq!(
        collision["data"]["store_epoch"],
        serde_json::json!(store_snapshot.epoch),
        "{value:#?}"
    );
    assert_eq!(
        collision["data"]["lock_epoch"], collision["data"]["store_epoch"],
        "a collision is same-epoch by definition: {value:#?}"
    );
    assert_ne!(
        collision["data"]["lock_digest"], collision["data"]["store_digest"],
        "a collision is different-digest by definition: {value:#?}"
    );
    // The next command regenerates the stale lock from the authoritative store, never loops the
    // read-only diagnostic back through `marrow doctor`.
    let next = collision["next_command"]
        .as_str()
        .expect("next_command string");
    assert_eq!(
        next,
        format!("marrow run {dir}"),
        "the collision next command must regenerate the lock, not re-run doctor: {next}"
    );

    // The store report block names the live store as the binding authority.
    assert_eq!(value["store"]["stamp"], "stamped", "{value:#?}");
    assert!(
        value["store"]["store_uid"].is_string(),
        "the store block reports the live store uid: {value:#?}"
    );

    // Doctor writes nothing: the store stamp, rows, and the committed lock bytes are byte-identical
    // before and after the probe.
    assert_eq!(
        fs::read(store_path(&project)).expect("read store after doctor"),
        store_before,
        "doctor must not rewrite the store"
    );
    assert_eq!(
        fs::read(lock_path(&project)).expect("read committed lock after doctor"),
        lock_before,
        "doctor must not rewrite the committed lock"
    );
}

#[test]
fn doctor_reports_a_stale_lock_against_the_live_store_without_writing() {
    let (project, dir) = seeded_project("doctor-stale-lock");

    // Mutate the committed lock's recorded source digest so it no longer matches the checked
    // source while leaving its identity and epoch intact: a stale lock, not a collision.
    let committed = committed_lock(&project);
    let stale = marrow_catalog::CatalogLock::new(
        committed.entries.clone(),
        committed.ledger.clone(),
        committed.epoch_high_water,
        "sha256:".to_string() + &"a".repeat(64),
    )
    .expect("stale lock builds");
    fs::write(
        lock_path(&project),
        stale.to_lock_json_pretty().expect("lock renders"),
    )
    .expect("write stale committed lock");
    let store_before = fs::read(store_path(&project)).expect("read store before doctor");
    let lock_before = fs::read(lock_path(&project)).expect("read stale committed lock");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let codes = finding_codes(&value);
    assert!(
        codes.contains(&"doctor.stale_lock"),
        "doctor reports the stale lock: {value:#?}"
    );
    assert!(
        !codes.contains(&"doctor.catalog_collision"),
        "a stale lock is not a same-epoch shape collision: {value:#?}"
    );
    let stale = finding(&value, "doctor.stale_lock");
    assert_eq!(
        stale["data"]["lock_source_digest"],
        serde_json::json!("sha256:".to_string() + &"a".repeat(64)),
        "{value:#?}"
    );
    // The next: breadcrumb must point at the fixing command, not a re-report.
    let next = stale["next_command"]
        .as_str()
        .expect("next_command is a string");
    assert!(
        next.contains("marrow run"),
        "stale_lock next must point at the regenerating run: {next}"
    );
    assert!(
        !next.contains("marrow check"),
        "stale_lock next must not loop back to a no-op check: {next}"
    );

    assert_eq!(
        fs::read(store_path(&project)).expect("read store after doctor"),
        store_before,
        "doctor must not rewrite the store"
    );
    assert_eq!(
        fs::read(lock_path(&project)).expect("read committed lock after doctor"),
        lock_before,
        "doctor must not rewrite the committed lock"
    );
}

#[test]
fn doctor_reports_a_missing_lock_over_a_stamped_store() {
    let (project, dir) = seeded_project("doctor-missing-lock");

    // Delete the committed lock while leaving the stamped store untouched: the store still carries
    // durable shape that a committed lock must project, so an absent lock is missing, not a first
    // run. This mirrors `check --locked`'s `check.lock_missing` gate.
    fs::remove_file(lock_path(&project)).expect("remove committed lock");
    let store_before = fs::read(store_path(&project)).expect("read store before doctor");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    assert_eq!(value["status"], "findings", "{value:#?}");
    let codes = finding_codes(&value);
    assert!(
        codes.contains(&"doctor.lock_missing"),
        "doctor reports the missing lock over a stamped store: {value:#?}"
    );
    let missing = finding(&value, "doctor.lock_missing");
    let next = missing["next_command"]
        .as_str()
        .expect("next_command is a string");
    assert!(
        next.contains("marrow run"),
        "lock_missing next must point at the regenerating run: {next}"
    );

    assert!(
        !lock_path(&project).exists(),
        "doctor must not write a marrow.lock"
    );
    assert_eq!(
        fs::read(store_path(&project)).expect("read store after doctor"),
        store_before,
        "doctor must not rewrite the store"
    );
}

#[test]
fn doctor_jsonl_emits_each_finding_then_exactly_one_final_summary_record() {
    let (project, dir) = seeded_project("doctor-jsonl-summary");
    // One deterministic finding (the missing committed lock) so the stream is a single finding line
    // followed by the summary line, letting the test pin the count and the summary's terminal position.
    fs::remove_file(lock_path(&project)).expect("remove committed lock");

    let output = marrow(&["doctor", "--format", "jsonl", &dir]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");

    let stdout = String::from_utf8(output.stdout).expect("doctor jsonl stdout utf8");
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each jsonl line parses as one object"))
        .collect();
    assert_eq!(
        records.len(),
        2,
        "one finding line then one summary line: {records:#?}"
    );

    let (summary, findings) = records.split_last().expect("at least the summary record");
    for finding in findings {
        assert_eq!(finding["code"], "doctor.lock_missing", "{finding:#?}");
        // A finding carries its own diagnostic `kind`, which is never the summary discriminator, so a
        // consumer filtering on `kind == "summary"` keeps every finding line.
        assert_ne!(finding["kind"], "summary", "{finding:#?}");
    }

    // The summary is the single terminal record consumers filter out on `kind`; it reports the finding
    // count rather than repeating the finding bodies, alongside the same probe objects the JSON envelope
    // carries.
    assert_eq!(summary["kind"], "summary", "{summary:#?}");
    assert_eq!(summary["status"], "findings", "{summary:#?}");
    assert_eq!(summary["findings"], 1, "{summary:#?}");
    assert!(summary["project"].is_string(), "{summary:#?}");
    for key in ["store", "fence", "integrity_sample"] {
        assert!(
            summary.get(key).is_some(),
            "summary carries the {key} probe object: {summary:#?}"
        );
    }
    assert!(
        summary.get("code").is_none() && summary.get("message").is_none(),
        "the summary is not a finding: {summary:#?}"
    );
}

#[test]
fn doctor_reports_only_lock_corrupt_for_a_corrupt_lock_over_a_stamped_store() {
    let (project, dir) = seeded_project("doctor-corrupt-lock-stamped");

    // A present-but-corrupt lock over a stamped store is a present file the operator must delete
    // and regenerate, not a missing committed projection. Doctor must report `doctor.lock_corrupt`
    // alone, never the contradictory `doctor.lock_missing` that would tell the operator the file is
    // both present-and-hostile and absent at once. This mirrors `check --locked`, which reports
    // only the corrupt condition.
    corrupt_lock(&project);

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let codes = finding_codes(&value);
    assert!(
        codes.contains(&"doctor.lock_corrupt"),
        "a corrupt lock over a stamped store reports lock_corrupt: {value:#?}"
    );
    assert!(
        !codes.contains(&"doctor.lock_missing"),
        "a corrupt lock is present, not missing: doctor must not double-report lock_missing: {value:#?}"
    );
}

#[test]
fn doctor_reports_no_findings_for_a_true_first_run() {
    // A native project with no run yet: no store file and no committed lock. There is no durable
    // shape to project, so an absent lock is a legitimate first run, not a missing commit.
    let project = native_project("doctor-first-run");
    let dir = project.to_str().expect("project path utf8").to_string();
    assert!(!lock_path(&project).exists(), "first run has no lock");
    assert!(!store_path(&project).exists(), "first run has no store");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let value = json(output.stdout);
    assert_eq!(value["status"], "ok", "{value:#?}");
    assert_eq!(value["findings"], serde_json::json!([]), "{value:#?}");
}

#[test]
fn doctor_reports_a_store_vs_lock_epoch_mismatch_distinct_from_a_collision() {
    let (project, dir) = seeded_project("doctor-store-lock-epoch");

    // Advance the committed lock's epoch high-water past the store's accepted epoch while keeping
    // its identity and source digest intact: the lock and the live store now record different
    // accepted epochs, the gate CI must fail on. This is not a same-epoch shape collision.
    let committed = committed_lock(&project);
    let ahead = marrow_catalog::CatalogLock::new(
        committed.entries.clone(),
        committed.ledger.clone(),
        committed.epoch_high_water + 1,
        committed.source_digest.clone(),
    )
    .expect("epoch-ahead lock builds");
    let store_epoch = {
        let store = SealedStore::open(&store_path(&project), AccessMode::Read)
            .expect("open store read-only")
            .into_store();
        store
            .read_catalog_snapshot()
            .expect("read store catalog snapshot")
            .expect("a stamped store carries an accepted catalog")
            .epoch
    };
    fs::write(
        lock_path(&project),
        ahead.to_lock_json_pretty().expect("lock renders"),
    )
    .expect("write epoch-ahead committed lock");
    let store_before = fs::read(store_path(&project)).expect("read store before doctor");
    let lock_before = fs::read(lock_path(&project)).expect("read epoch-ahead committed lock");

    let output = marrow(&["doctor", "--format", "json", &dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let codes = finding_codes(&value);
    assert!(
        codes.contains(&"doctor.store_lock_epoch_mismatch"),
        "doctor reports the store-vs-lock epoch mismatch: {value:#?}"
    );
    assert!(
        !codes.contains(&"doctor.catalog_collision"),
        "an epoch mismatch is not a same-epoch shape collision: {value:#?}"
    );
    let mismatch = finding(&value, "doctor.store_lock_epoch_mismatch");
    assert_eq!(
        mismatch["data"]["lock_epoch"],
        serde_json::json!(committed.epoch_high_water + 1),
        "{value:#?}"
    );
    assert_eq!(
        mismatch["data"]["store_epoch"],
        serde_json::json!(store_epoch),
        "{value:#?}"
    );
    // The store is BEHIND the committed lock — the R36-01 whole-body-rollback residual state, a
    // store restored to an older epoch being locally indistinguishable from a never-advanced
    // checkout. The advisory must be accurate for a behind store: advise advancing or restoring,
    // and explicitly refuse to declare the behind store authoritative or regenerate the lock from
    // it (which would discard the committed activation).
    let remedy = mismatch["remedy"].as_str().expect("remedy string");
    assert!(
        remedy.contains("behind the committed lock")
            && remedy.contains("do not regenerate")
            && !remedy.contains("authoritative"),
        "the behind-store advisory must not declare the rolled-back store authoritative: {remedy}"
    );
    // The next command must make progress — advance the behind store — never loop the read-only
    // diagnostic back through `marrow doctor`.
    let next = mismatch["next_command"]
        .as_str()
        .expect("next_command string");
    assert_eq!(
        next,
        format!("marrow evolve apply {dir}"),
        "the behind-store next command must advance, not re-run doctor: {next}"
    );

    assert_eq!(
        fs::read(store_path(&project)).expect("read store after doctor"),
        store_before,
        "doctor must not rewrite the store"
    );
    assert_eq!(
        fs::read(lock_path(&project)).expect("read committed lock after doctor"),
        lock_before,
        "doctor must not rewrite the committed lock"
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
fn doctor_surfaces_a_private_default_entry_check_failure() {
    // The default-entry check runs in `analyze_project`, so `doctor` inherits it:
    // a private `run.defaultEntry` surfaces as `check.default_entry` among the
    // failed check's underlying codes.
    let project = support::temp_project_uncommitted("doctor-default-entry", |root| {
        support::write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        support::write(root, "src/app.mw", "module app\n\nfn main()\n    return\n");
    });
    let dir = project.path().to_str().expect("project path utf8");

    let output = marrow(&["doctor", "--format", "json", dir]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = json(output.stdout);
    let check_finding = finding(&value, "doctor.check_failed");
    let codes = check_finding["data"]["underlying_codes"]
        .as_array()
        .expect("underlying codes array");
    assert!(
        codes.contains(&serde_json::json!("check.default_entry")),
        "{value:#?}"
    );
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
        stdout.contains(&format!("ok: {dir} is healthy (no issues found)")),
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
