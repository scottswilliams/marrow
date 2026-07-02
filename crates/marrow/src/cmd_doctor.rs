//! `marrow doctor`: read-only operator triage over existing project facts.

use marrow_codes::Code;
use std::path::Path;
use std::process::ExitCode;

use marrow_catalog::{CatalogLifecycle, CatalogLock, CatalogMetadata, LockEntry};
use marrow_check::ProjectIoError;
use marrow_check::tooling::{IntegritySample, sample_integrity_problems};
use marrow_run::evolution::{FenceError, current_engine_profile, fence};
use marrow_store::StoreError;
use marrow_store::tree::{CommitMetadata, TreeStore};
use serde_json::{Value, json};

use crate::term_style::{self, Stream, Style};
use crate::{CheckFormat, store_path_is_absent, write_json};

pub(crate) const DOCTOR_INTEGRITY_SAMPLE_LIMIT: usize = 64;

pub(crate) fn doctor(args: &[String]) -> ExitCode {
    let DoctorArgs { dir, format } = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    run_doctor(&dir, format)
}

struct DoctorArgs {
    dir: String,
    format: CheckFormat,
}

fn parse_args(args: &[String]) -> Result<DoctorArgs, ExitCode> {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)?;
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow doctor [--format text|json|jsonl] <projectdir>

Inspect project, catalog, and store facts without repairing or writing anything.
"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => return Err(crate::unknown_option("doctor", value)),
            value => {
                crate::take_single_target(&mut dir, value, "doctor", "project directory")?;
            }
        }
        index += 1;
    }
    let dir = dir.ok_or_else(|| {
        eprintln!("missing project directory");
        ExitCode::from(2)
    })?;
    Ok(DoctorArgs { dir, format })
}

fn run_doctor(dir: &str, format: CheckFormat) -> ExitCode {
    let root = Path::new(dir);
    let mut findings = Vec::new();

    let config = probe_config(root, dir, &mut findings);
    let lock_probe = probe_lock(root, dir, &mut findings);
    let lock = lock_probe.lock();
    let store = config
        .as_ref()
        .and_then(|config| probe_store_open(root, dir, config, &mut findings));
    // The live store is the sole accepted authority; the committed lock only seeds first-run
    // adoption when no store snapshot is present, mirroring the surface check path. Binding the
    // store snapshot lets a healthy stamped project check and fence cleanly.
    let accepted = store
        .as_ref()
        .and_then(|store| store.read_catalog_snapshot().ok().flatten());
    let program = match &config {
        Some(config) => probe_check(root, dir, config, accepted.as_ref(), lock, &mut findings),
        None => None,
    };
    // The lock-root witness runs over a present store: one rolled back below its committed roots
    // has lost durable identity. An absent store body is the disposable-store case, not a loss, so
    // it is not charged. An unreadable store (locked, recovery-required, or hard corrupt) already
    // carries its own finding and is not also charged this loss.
    if let Some(store) = store.as_ref() {
        probe_lock_root_witness(dir, store, lock, &mut findings);
    }
    let store_report = store.as_ref().map(|store| {
        probe_store_facts(
            dir,
            store,
            accepted.as_ref(),
            &lock_probe,
            program.as_ref(),
            &mut findings,
        )
    });
    let fence_report = match (&store, &program) {
        (Some(store), Some(program)) => Some(probe_fence(dir, store, program, &mut findings)),
        _ => None,
    };
    let integrity_sample = match (&store, &program) {
        (Some(store), Some(program)) => {
            Some(probe_integrity_sample(dir, store, program, &mut findings))
        }
        (None, _) => Some(IntegritySample {
            items_checked: 0,
            problems: 0,
            truncated: false,
        }),
        _ => None,
    };

    render_report(
        dir,
        format,
        &findings,
        store_report.as_ref(),
        fence_report.as_ref(),
        integrity_sample,
    );
    if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn probe_config(
    root: &Path,
    dir: &str,
    findings: &mut Vec<Finding>,
) -> Option<marrow_project::ProjectConfig> {
    match marrow_check::load_config(root) {
        Ok(config) => Some(config),
        Err(error) => {
            let (remedy, next_command) = config_load_remedy(&error, dir);
            findings.push(project_error_finding(
                Code::DoctorConfigInvalid.as_str(),
                "project configuration could not be loaded",
                remedy,
                next_command,
                error,
            ));
            None
        }
    }
}

/// The remedy and next command for a config-load failure, derived from the typed fault so each
/// names an action that resolves it rather than looping `marrow doctor` back at the same fault. A
/// directory with no `marrow.json` is created by `marrow init`; a bare-file path is the wrong
/// target, so it points `marrow check` at a real project directory; an unreadable or invalid
/// `marrow.json` is fixed in place, after which `marrow check` confirms the fix. Doctor never names
/// itself as the next step for a fault a re-run of doctor cannot change.
fn config_load_remedy(error: &ProjectIoError, dir: &str) -> (&'static str, String) {
    match error {
        ProjectIoError::ConfigMissing { .. } => (
            "create the project with marrow init",
            format!("marrow init {dir}"),
        ),
        ProjectIoError::NotAProject { .. } => (
            "pass a project directory containing marrow.json, not a bare file",
            check_command("<projectdir>"),
        ),
        ProjectIoError::Io { .. } => (
            "make the reported marrow.json readable, then recheck the project",
            check_command(dir),
        ),
        _ => (
            "fix the reported marrow.json field, then recheck the project",
            check_command(dir),
        ),
    }
}

/// The doctor-relevant state of the committed `marrow.lock`. The distinction between an absent
/// lock and a present-but-corrupt one is load-bearing: a stamped store with no lock at all is a
/// missing committed projection, while a stamped store with a corrupt lock has a present file that
/// must be deleted and regenerated, not reported as missing.
enum LockProbe {
    Present(CatalogLock),
    Absent,
    Corrupt,
}

impl LockProbe {
    fn lock(&self) -> Option<&CatalogLock> {
        match self {
            LockProbe::Present(lock) => Some(lock),
            LockProbe::Absent | LockProbe::Corrupt => None,
        }
    }
}

/// Read the committed `marrow.lock` through its canonical reader. A present-but-corrupt lock
/// fails closed with the typed `doctor.lock_corrupt` finding so a hostile lock is never treated
/// as absent; an absent lock is a true first run. The live store, not the lock, remains the
/// binding authority, so a corrupt lock does not block the store-fact probes.
fn probe_lock(root: &Path, dir: &str, findings: &mut Vec<Finding>) -> LockProbe {
    match marrow_check::read_committed_lock(root) {
        Ok(Some(lock)) => LockProbe::Present(lock),
        Ok(None) => LockProbe::Absent,
        Err(error) => {
            findings.push(project_error_finding(
                Code::DoctorLockCorrupt.as_str(),
                "committed marrow.lock could not be read",
                "delete the corrupt marrow.lock so the next run or evolve apply re-projects it from the authoritative store",
                check_command(dir),
                error,
            ));
            LockProbe::Corrupt
        }
    }
}

fn probe_check(
    root: &Path,
    dir: &str,
    config: &marrow_project::ProjectConfig,
    accepted: Option<&CatalogMetadata>,
    lock: Option<&CatalogLock>,
    findings: &mut Vec<Finding>,
) -> Option<marrow_check::CheckedProgram> {
    let snapshot = match marrow_check::analyze_project(
        root,
        config,
        &marrow_check::ProjectSources::new(),
        accepted,
        lock,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut data = serde_json::Map::new();
            data.insert("underlying_code".into(), json!(error.code));
            data.insert("path".into(), json!(error.path.display().to_string()));
            findings.push(Finding::new(
                Code::DoctorCheckFailed.as_str(),
                "project sources could not be loaded for checking",
                "fix unreadable source paths, then rerun the next command",
                check_command(dir),
                data,
            ));
            return None;
        }
    };
    if snapshot.report.has_errors() {
        let mut data = serde_json::Map::new();
        let diagnostic_codes = snapshot
            .report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        data.insert("diagnostics".into(), json!(diagnostic_codes.len()));
        data.insert("underlying_codes".into(), json!(diagnostic_codes));
        findings.push(Finding::new(
            Code::DoctorCheckFailed.as_str(),
            "project check reports diagnostics",
            "fix check diagnostics, then rerun the next command",
            check_command(dir),
            data,
        ));
        None
    } else {
        Some(snapshot.program)
    }
}

/// Doctor diagnoses stores that would fail admission, so it holds the stage-1
/// [`SealedStore`](marrow_store::SealedStore) and never an admitted handle.
fn probe_store_open(
    root: &Path,
    dir: &str,
    config: &marrow_project::ProjectConfig,
    findings: &mut Vec<Finding>,
) -> Option<marrow_store::SealedStore> {
    // A `dataDir` occupied by a non-directory is a configuration fault, the same one
    // `run` raises; classifying it here keeps doctor from leaking the store open's raw
    // `ENOTDIR` as a `store.io` finding.
    if let Err(error) = marrow_check::guard_data_dir(root, config) {
        findings.push(project_error_finding(
            Code::DoctorConfigInvalid.as_str(),
            "native store dataDir is not a directory",
            "point dataDir at a writable directory or remove the file occupying it, then recheck the project",
            check_command(dir),
            error,
        ));
        return None;
    }
    let path = match marrow_check::native_store_path(root, config) {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(error) => {
            findings.push(project_error_finding(
                Code::DoctorConfigInvalid.as_str(),
                "project configuration is invalid",
                "fix the reported marrow.json field, then recheck the project",
                check_command(dir),
                error,
            ));
            return None;
        }
    };
    if store_path_is_absent(&path) {
        return None;
    }
    // Prove the store is fully traversable, the same walk `data recover` runs, so doctor
    // never reports a store healthy that the read-only inspections and the write path
    // classify as corrupt below the table roots.
    match marrow_run::admission::open_read(&path)
        .and_then(|store| store.verify_readable().map(|()| store))
    {
        Ok(store) => Some(store),
        Err(error @ StoreError::Locked { .. }) => {
            findings.push(store_error_finding(
                Code::DoctorStoreLocked.as_str(),
                "native store is locked",
                "close the process holding the native store, then rerun the next command",
                doctor_command(dir),
                &path,
                error,
            ));
            None
        }
        Err(error @ StoreError::RecoveryRequired) => {
            findings.push(store_error_finding(
                Code::DoctorStoreRecoveryRequired.as_str(),
                "native store needs recovery before read-only inspection",
                "open the store through the recovery command",
                format!("marrow data recover {dir}"),
                &path,
                error,
            ));
            None
        }
        Err(error) => {
            findings.push(store_error_finding(
                Code::DoctorStoreUnavailable.as_str(),
                "native store could not be opened read-only",
                "inspect the store problem, then rerun the next read-only command",
                store_error_next_command(dir, &error),
                &path,
                error,
            ));
            None
        }
    }
}

/// Run the committed-lock root witness over the present store snapshot doctor sees. A store
/// presenting fewer committed roots than the lock recorded has lost durable identity to a
/// rollback or torn baseline; doctor surfaces it as `doctor.store_unavailable` carrying the
/// underlying `store.corruption` code, the same verdict `backup` and `data recover` reach, so a
/// CI doctor never blesses a store the write paths reject. An absent store body never reaches
/// here: it is the disposable-store case the write paths seed, not a loss.
fn probe_lock_root_witness(
    dir: &str,
    store: &TreeStore,
    lock: Option<&CatalogLock>,
    findings: &mut Vec<Finding>,
) {
    match crate::verify_lock_roots(Some(store), lock) {
        crate::LockRootVerdict::Clean => {}
        crate::LockRootVerdict::Lost(error) => {
            findings.push(store_fact_error(dir, "committed roots", error));
        }
    }
}

fn probe_store_facts(
    dir: &str,
    store: &TreeStore,
    accepted: Option<&CatalogMetadata>,
    lock_probe: &LockProbe,
    program: Option<&marrow_check::CheckedProgram>,
    findings: &mut Vec<Finding>,
) -> StoreReport {
    let lock = lock_probe.lock();
    let current_profile = current_engine_profile();
    let commit = match store.read_commit_metadata() {
        Ok(commit) => commit,
        Err(error) => {
            findings.push(store_fact_error(dir, "commit metadata", error));
            None
        }
    };
    let store_uid = match store.read_store_uid() {
        Ok(uid) => uid.map(|uid| uid.as_str().to_string()),
        Err(error) => {
            findings.push(store_fact_error(dir, "store UID", error));
            None
        }
    };

    if commit.is_none() && probe_populated(store, dir, findings) == Some(true) {
        findings.push(populated_unstamped_finding(dir));
    }
    // Only a physically absent lock over an accepted store is a missing committed projection. A
    // corrupt lock is a present file already reported as `doctor.lock_corrupt`; reporting it as
    // missing too would contradictorily tell the operator both to delete and to regenerate it.
    if matches!(lock_probe, LockProbe::Absent) && accepted.is_some() {
        findings.push(missing_lock_finding(dir));
    }
    if let (Some(lock), Some(accepted)) = (lock, accepted) {
        probe_lock_against_store(dir, lock, accepted, findings);
    }
    if let (Some(lock), Some(program)) = (lock, program) {
        probe_stale_lock(dir, lock, program, findings);
    }

    StoreReport {
        stamp: if commit.is_some() {
            "stamped"
        } else {
            "unstamped"
        },
        store_uid,
        commit,
        current_engine: EngineReport {
            layout_epoch: current_profile.layout_epoch(),
            key_profile_version: current_profile.key_profile_version(),
            profile_digest: current_profile.digest_hex(),
        },
    }
}

/// Whether the live store holds any durable records. A read error reports its own store-fact
/// finding and suppresses the populated check rather than misclassifying an unreadable store as
/// empty.
fn probe_populated(store: &TreeStore, dir: &str, findings: &mut Vec<Finding>) -> Option<bool> {
    match store.is_empty() {
        Ok(empty) => Some(!empty),
        Err(error) => {
            findings.push(store_fact_error(dir, "store contents", error));
            None
        }
    }
}

/// A populated store with no commit stamp: records exist but no accepted identity admits them.
/// The remedy mirrors the run path's fail-closed verdict for this condition — run the owning
/// program or remove the store — so doctor reports the same resolution the next write path enforces.
fn populated_unstamped_finding(dir: &str) -> Finding {
    Finding::new(
        Code::DoctorPopulatedUnstamped.as_str(),
        "the store holds saved data but carries no accepted commit stamp",
        "run the program that owns this store, or remove the store to start fresh",
        doctor_command(dir),
        serde_json::Map::new(),
    )
}

/// A stamped store carries durable shape a committed lock must project, but no `marrow.lock` is
/// present: the committed projection is absent, not merely stale. A CI gate that passed this would
/// give a false green to a developer who forgot to commit or deleted the lock, so doctor surfaces
/// it as a finding — mirroring `check`'s `check.lock_missing` gate. A uid-only store with no
/// accepted catalog, like an absent store, has nothing to lock and stays a healthy first run.
fn missing_lock_finding(dir: &str) -> Finding {
    Finding::new(
        Code::DoctorLockMissing.as_str(),
        "marrow.lock is missing but the live store carries saved shape",
        "regenerate marrow.lock with a run or evolve apply, then commit it",
        run_command(dir),
        serde_json::Map::new(),
    )
}

/// Compare the committed lock against the live store's accepted snapshot. The lock is the
/// subordinate source-tree projection; the live store is the binding authority, so neither verdict
/// ever advises overwriting the store from the lock. Disagreement falls into exactly one of two
/// typed findings: at the same epoch a shape divergence is a `catalog_collision`; at different
/// epochs the lock has drifted from the store's accepted epoch, the gate CI fails on.
fn probe_lock_against_store(
    dir: &str,
    lock: &CatalogLock,
    accepted: &CatalogMetadata,
    findings: &mut Vec<Finding>,
) {
    if lock.epoch_high_water != accepted.epoch {
        let mut data = serde_json::Map::new();
        data.insert("lock_epoch".into(), json!(lock.epoch_high_water));
        data.insert("store_epoch".into(), json!(accepted.epoch));
        findings.push(Finding::new(
            Code::DoctorStoreLockEpochMismatch.as_str(),
            "the committed lock and the live store record different accepted epochs",
            "the live store is authoritative; regenerate marrow.lock with a run or evolve apply",
            doctor_command(dir),
            data,
        ));
        return;
    }
    let committed_fingerprints = sorted_fingerprints(lock.entries.clone());
    let store_fingerprints = store_fingerprints(accepted);
    if committed_fingerprints == store_fingerprints {
        return;
    }
    let mut data = serde_json::Map::new();
    data.insert("lock_epoch".into(), json!(lock.epoch_high_water));
    data.insert("store_epoch".into(), json!(accepted.epoch));
    data.insert(
        "lock_digest".into(),
        json!(lock_shape_digest(&committed_fingerprints)),
    );
    data.insert(
        "store_digest".into(),
        json!(lock_shape_digest(&store_fingerprints)),
    );
    findings.push(Finding::new(
        Code::DoctorCatalogCollision.as_str(),
        "the committed lock and the live store record the same accepted epoch with different shapes",
        "the live store is authoritative; regenerate marrow.lock with a run or evolve apply",
        doctor_command(dir),
        data,
    ));
}

/// A committed lock whose recorded source digest no longer matches the checked source. The lock is
/// inert and subordinate to the live store, so this is reported, never repaired.
fn probe_stale_lock(
    dir: &str,
    lock: &CatalogLock,
    program: &marrow_check::CheckedProgram,
    findings: &mut Vec<Finding>,
) {
    let source_digest = program.source_digest();
    if lock.source_digest == source_digest {
        return;
    }
    let mut data = serde_json::Map::new();
    data.insert(
        "lock_source_digest".into(),
        json!(lock.source_digest.clone()),
    );
    data.insert("source_digest".into(), json!(source_digest));
    findings.push(Finding::new(
        Code::DoctorStaleLock.as_str(),
        "the committed lock is behind the current source",
        "regenerate marrow.lock with a run or evolve apply, then commit it",
        run_command(dir),
        data,
    ));
}

/// The active accepted entries of a store snapshot, fingerprinted into lock entries through the
/// canonical [`LockEntry`] owner and sorted by stable id. This is the same projection the committed
/// lock records, so the two compare directly without re-parsing any shape grammar.
fn store_fingerprints(snapshot: &CatalogMetadata) -> Vec<LockEntry> {
    sorted_fingerprints(
        snapshot
            .entries
            .iter()
            .filter(|entry| entry.lifecycle == CatalogLifecycle::Active)
            .map(LockEntry::from_catalog_entry)
            .collect(),
    )
}

/// Sort lock entries into the canonical stable-id order both the committed lock and the store
/// projection share, so two entry sets compare independent of their stored order.
fn sorted_fingerprints(mut entries: Vec<LockEntry>) -> Vec<LockEntry> {
    entries.sort_by(|left, right| left.stable_id.cmp(&right.stable_id));
    entries
}

/// A stable digest over an already-sorted lock entry set, folding every field the collision
/// comparison uses (kind, path, lifecycle, stable id, shape fingerprint). Two entry sets share this
/// digest exactly when their entries are equal, so the collision finding's debug/admin data differs
/// precisely when the finding fires.
fn lock_shape_digest(entries: &[LockEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{:?}:{}:{:?}:{}={}",
                entry.kind, entry.path, entry.lifecycle, entry.stable_id, entry.shape_fingerprint
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn probe_fence(
    dir: &str,
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
    findings: &mut Vec<Finding>,
) -> FenceReport {
    match fence(
        program.catalog.accepted_epoch,
        &program.source_digest(),
        store,
    ) {
        Ok(()) => FenceReport {
            status: "ok",
            underlying_code: None,
        },
        Err(error) => {
            let mut data = serde_json::Map::new();
            data.insert("underlying_code".into(), json!(error.code()));
            data.insert("message".into(), json!(error.message()));
            findings.push(Finding::new(
                Code::DoctorFenceMismatch.as_str(),
                "the saved store is at an older schema than the current source",
                "preview the pending change, then apply it",
                fence_next_command(dir, &error),
                data,
            ));
            FenceReport {
                status: "mismatch",
                underlying_code: Some(error.code()),
            }
        }
    }
}

fn probe_integrity_sample(
    dir: &str,
    store: &TreeStore,
    program: &marrow_check::CheckedProgram,
    findings: &mut Vec<Finding>,
) -> IntegritySample {
    match sample_integrity_problems(store, program, DOCTOR_INTEGRITY_SAMPLE_LIMIT) {
        Ok(sample) => {
            if sample.problems > 0 {
                let mut data = serde_json::Map::new();
                data.insert("items_checked".into(), json!(sample.items_checked));
                data.insert("problems".into(), json!(sample.problems));
                data.insert("limit".into(), json!(DOCTOR_INTEGRITY_SAMPLE_LIMIT));
                data.insert("truncated".into(), json!(sample.truncated));
                findings.push(Finding::new(
                    Code::DoctorIntegritySampleFailed.as_str(),
                    "bounded saved-data integrity sample found problems",
                    "run the full read-only integrity report",
                    format!("marrow data integrity {dir}"),
                    data,
                ));
            }
            sample
        }
        Err(error) => {
            let mut data = serde_json::Map::new();
            data.insert("underlying_code".into(), json!(error.code()));
            data.insert("message".into(), json!(error.to_string()));
            data.insert("limit".into(), json!(DOCTOR_INTEGRITY_SAMPLE_LIMIT));
            findings.push(Finding::new(
                Code::DoctorIntegritySampleFailed.as_str(),
                "bounded saved-data integrity sample could not complete",
                "run the full read-only integrity report",
                format!("marrow data integrity {dir}"),
                data,
            ));
            IntegritySample {
                items_checked: 0,
                problems: 0,
                truncated: false,
            }
        }
    }
}

fn project_error_finding(
    code: &'static str,
    message: impl Into<String>,
    remedy: impl Into<String>,
    next_command: String,
    error: ProjectIoError,
) -> Finding {
    let mut data = serde_json::Map::new();
    data.insert("underlying_code".into(), json!(error.code()));
    data.insert("message".into(), json!(error.message()));
    match error {
        ProjectIoError::Io { path, .. }
        | ProjectIoError::DataDirCreate { path, .. }
        | ProjectIoError::CheckLoad { path, .. } => {
            data.insert("path".into(), json!(path.display().to_string()));
        }
        ProjectIoError::ConfigMissing { dir } => {
            data.insert("path".into(), json!(dir.display().to_string()));
        }
        ProjectIoError::NotAProject { path } => {
            data.insert("path".into(), json!(path.display().to_string()));
        }
        ProjectIoError::Config { .. }
        | ProjectIoError::Catalog { .. }
        | ProjectIoError::Check { .. }
        | ProjectIoError::Store(_) => {}
    }
    Finding::new(code, message, remedy, next_command, data)
}

fn store_error_finding(
    code: &'static str,
    message: impl Into<String>,
    remedy: impl Into<String>,
    next_command: String,
    path: &Path,
    error: StoreError,
) -> Finding {
    let mut data = serde_json::Map::new();
    data.insert("underlying_code".into(), json!(error.code()));
    data.insert("message".into(), json!(error.to_string()));
    data.insert("store".into(), json!(path.display().to_string()));
    Finding::new(code, message, remedy, next_command, data)
}

fn store_fact_error(dir: &str, fact: &'static str, error: StoreError) -> Finding {
    let mut data = serde_json::Map::new();
    data.insert("underlying_code".into(), json!(error.code()));
    data.insert("fact".into(), json!(fact));
    data.insert("message".into(), json!(error.to_string()));
    let next_command = store_error_next_command(dir, &error);
    Finding::new(
        Code::DoctorStoreUnavailable.as_str(),
        "native store metadata could not be read",
        "inspect the store problem, then rerun the next read-only command",
        next_command,
        data,
    )
}

fn store_error_next_command(dir: &str, error: &StoreError) -> String {
    match error {
        StoreError::RecoveryRequired => format!("marrow data recover {dir}"),
        _ => doctor_command(dir),
    }
}

fn fence_next_command(dir: &str, error: &FenceError) -> String {
    match error {
        FenceError::StoreBehind { .. } => format!("marrow evolve apply {dir}"),
        FenceError::SchemaDrift => format!("marrow evolve preview {dir}"),
        FenceError::StoreEvolved { .. }
        | FenceError::EngineProfileDrift
        | FenceError::DurableStoreRequired => doctor_command(dir),
        FenceError::Store(store) => match store {
            StoreError::RecoveryRequired => format!("marrow data recover {dir}"),
            _ => doctor_command(dir),
        },
    }
}

fn check_command(dir: &str) -> String {
    format!("marrow check {dir}")
}

fn run_command(dir: &str) -> String {
    format!("marrow run {dir}")
}

fn doctor_command(dir: &str) -> String {
    format!("marrow doctor {dir}")
}

struct StoreReport {
    stamp: &'static str,
    store_uid: Option<String>,
    commit: Option<CommitMetadata>,
    current_engine: EngineReport,
}

struct FenceReport {
    status: &'static str,
    underlying_code: Option<&'static str>,
}

struct EngineReport {
    layout_epoch: u64,
    key_profile_version: u8,
    profile_digest: String,
}

struct Finding {
    code: &'static str,
    message: String,
    remedy: String,
    next_command: String,
    data: serde_json::Map<String, Value>,
}

impl Finding {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        remedy: impl Into<String>,
        next_command: String,
        data: serde_json::Map<String, Value>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            remedy: remedy.into(),
            next_command,
            data,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "kind": marrow_syntax::kind_for_code(self.code),
            "message": self.message,
            "remedy": self.remedy,
            "next_command": self.next_command,
            "data": self.data,
            "source_span": null,
        })
    }
}

fn render_report(
    dir: &str,
    format: CheckFormat,
    findings: &[Finding],
    store: Option<&StoreReport>,
    fence: Option<&FenceReport>,
    integrity_sample: Option<IntegritySample>,
) {
    match format {
        CheckFormat::Text => render_text(dir, findings, integrity_sample),
        CheckFormat::Json => write_json(json!({
            "project": crate::project_json_path(dir),
            "status": if findings.is_empty() { "ok" } else { "findings" },
            "findings": findings.iter().map(Finding::to_json).collect::<Vec<_>>(),
            "store": store.map(store_report_json),
            "fence": fence.map(fence_report_json),
            "integrity_sample": integrity_sample.map(integrity_sample_json),
        })),
        CheckFormat::Jsonl => {
            for finding in findings {
                write_json(finding.to_json());
            }
            write_json(json!({
                "kind": "summary",
                "project": crate::project_json_path(dir),
                "status": if findings.is_empty() { "ok" } else { "findings" },
                "findings": findings.len(),
                "store": store.map(store_report_json),
                "fence": fence.map(fence_report_json),
                "integrity_sample": integrity_sample.map(integrity_sample_json),
            }));
        }
    }
}

fn render_text(dir: &str, findings: &[Finding], integrity_sample: Option<IntegritySample>) {
    if findings.is_empty() {
        println!(
            "{} {dir} is healthy (no issues found)",
            term_style::paint(Stream::Stdout, Style::Success, "ok:")
        );
    } else {
        for finding in findings {
            let code = term_style::paint(Stream::Stdout, Style::Code, finding.code);
            let remedy = term_style::paint(Stream::Stdout, Style::Warning, "remedy:");
            let next = term_style::paint(Stream::Stdout, Style::Warning, "next:");
            println!(
                "{code}: {}; {remedy} {}; {next} `{}`",
                finding.message, finding.remedy, finding.next_command
            );
        }
    }
    if let Some(sample) = integrity_sample
        && sample.truncated
    {
        println!(
            "sample truncated after {} items; run marrow data integrity {dir} for the full read-only report",
            sample.items_checked
        );
    }
}

fn store_report_json(report: &StoreReport) -> Value {
    json!({
        "stamp": report.stamp,
        "store_uid": report.store_uid,
        "commit": report.commit.as_ref().map(commit_json),
        "current_engine": {
            "layout_epoch": report.current_engine.layout_epoch,
            "key_profile_version": report.current_engine.key_profile_version,
            "profile_digest": report.current_engine.profile_digest,
        },
    })
}

fn fence_report_json(report: &FenceReport) -> Value {
    json!({
        "status": report.status,
        "underlying_code": report.underlying_code,
    })
}

fn commit_json(commit: &CommitMetadata) -> Value {
    json!({
        "commit_id": commit.commit_id,
        "catalog_epoch": commit.catalog_epoch,
        "layout_epoch": commit.layout_epoch,
        "source_digest": commit.source_digest,
        "engine_profile_digest": crate::hex_string(&commit.engine_profile_digest),
        "changed_root_catalog_ids": commit
            .changed_root_catalog_ids
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        "changed_index_catalog_ids": commit
            .changed_index_catalog_ids
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
    })
}

fn integrity_sample_json(sample: IntegritySample) -> Value {
    json!({
        "limit": DOCTOR_INTEGRITY_SAMPLE_LIMIT,
        "items_checked": sample.items_checked,
        "problems": sample.problems,
        "truncated": sample.truncated,
    })
}
