//! `marrow doctor`: read-only operator triage over existing project facts.

use std::path::Path;
use std::process::ExitCode;

use marrow_check::ProjectIoError;
use marrow_check::tooling::{IntegritySample, sample_integrity_problems};
use marrow_run::evolution::{FenceError, current_engine_profile, fence};
use marrow_store::StoreError;
use marrow_store::tree::{CommitMetadata, TreeStore};
use serde_json::{Value, json};

use crate::{CheckFormat, write_json};

pub(crate) const DOCTOR_INTEGRITY_SAMPLE_LIMIT: usize = 64;

pub(crate) fn doctor(args: &[String]) -> ExitCode {
    let DoctorArgs { dir, format } = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if let Err(code) = crate::reject_bare_file_target("doctor", &dir) {
        return code;
    }
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
    let accepted = probe_catalog_artifact(root, dir, &mut findings);
    let program = match (&config, &accepted) {
        (Some(config), Some(accepted)) => {
            probe_check(root, dir, config, accepted.as_ref(), &mut findings)
        }
        (Some(config), None) => probe_check(root, dir, config, None, &mut findings),
        (None, _) => None,
    };
    let store = config
        .as_ref()
        .and_then(|config| probe_store_open(root, dir, config, &mut findings));
    let store_report = store
        .as_ref()
        .map(|store| probe_store_facts(dir, store, &accepted, &mut findings));
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
            findings.push(project_error_finding(
                "doctor.config_invalid",
                "project configuration could not be loaded",
                "fix marrow.json, then rerun the next command",
                doctor_command(dir),
                error,
            ));
            None
        }
    }
}

fn probe_catalog_artifact(
    root: &Path,
    dir: &str,
    findings: &mut Vec<Finding>,
) -> Option<Option<marrow_catalog::CatalogMetadata>> {
    match marrow_check::read_accepted_catalog_artifact(root) {
        Ok(accepted) => Some(accepted),
        Err(error @ ProjectIoError::Catalog { .. }) => {
            findings.push(project_error_finding(
                "doctor.catalog_invalid",
                "accepted catalog artifact is invalid",
                "restore or regenerate marrow.catalog.json, then rerun the next command",
                check_command(dir),
                error,
            ));
            None
        }
        Err(error) => {
            findings.push(project_error_finding(
                "doctor.catalog_unreadable",
                "accepted catalog artifact could not be read",
                "make marrow.catalog.json readable, then rerun the next command",
                check_command(dir),
                error,
            ));
            None
        }
    }
}

fn probe_check(
    root: &Path,
    dir: &str,
    config: &marrow_project::ProjectConfig,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    findings: &mut Vec<Finding>,
) -> Option<marrow_check::CheckedProgram> {
    let snapshot = match marrow_check::analyze_project(
        root,
        config,
        &marrow_check::ProjectSources::new(),
        accepted,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut data = serde_json::Map::new();
            data.insert("underlying_code".into(), json!(error.code));
            data.insert("path".into(), json!(error.path.display().to_string()));
            findings.push(Finding::new(
                "doctor.check_failed",
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
            "doctor.check_failed",
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

fn probe_store_open(
    root: &Path,
    dir: &str,
    config: &marrow_project::ProjectConfig,
    findings: &mut Vec<Finding>,
) -> Option<TreeStore> {
    let path = match marrow_check::native_store_path(root, config) {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(error) => {
            findings.push(project_error_finding(
                "doctor.config_invalid",
                "project configuration is invalid",
                "fix marrow.json, then rerun the next command",
                doctor_command(dir),
                error,
            ));
            return None;
        }
    };
    if !path.exists() {
        return None;
    }
    match TreeStore::open_read_only(&path) {
        Ok(store) => Some(store),
        Err(error @ StoreError::Locked { .. }) => {
            findings.push(store_error_finding(
                "doctor.store_locked",
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
                "doctor.store_recovery_required",
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
                "doctor.store_unavailable",
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

fn probe_store_facts(
    dir: &str,
    store: &TreeStore,
    accepted: &Option<Option<marrow_catalog::CatalogMetadata>>,
    findings: &mut Vec<Finding>,
) -> StoreReport {
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
    let store_catalog = match store.read_catalog_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            findings.push(store_fact_error(dir, "store catalog snapshot", error));
            None
        }
    };

    if let (Some(Some(accepted)), Some(store_catalog)) = (accepted, &store_catalog)
        && accepted != store_catalog
    {
        let mut data = serde_json::Map::new();
        data.insert("artifact_epoch".into(), json!(accepted.epoch));
        data.insert("artifact_digest".into(), json!(accepted.digest));
        data.insert("store_epoch".into(), json!(store_catalog.epoch));
        data.insert("store_digest".into(), json!(store_catalog.digest));
        findings.push(Finding::new(
            "doctor.catalog_drift",
            "accepted catalog artifact differs from the store snapshot",
            "restore the intended catalog artifact, then rerun the next command",
            check_command(dir),
            data,
        ));
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
                "doctor.fence_mismatch",
                "store activation fence does not match the checked project",
                "run the command named here to inspect or apply the required activation work",
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
                    "doctor.integrity_sample_failed",
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
                "doctor.integrity_sample_failed",
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
        ProjectIoError::Io { path, .. } | ProjectIoError::CheckLoad { path, .. } => {
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
        "doctor.store_unavailable",
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
        FenceError::StoreEvolved { .. } | FenceError::EngineProfileDrift => doctor_command(dir),
        FenceError::Store(store) => match store {
            StoreError::RecoveryRequired => format!("marrow data recover {dir}"),
            _ => doctor_command(dir),
        },
    }
}

fn check_command(dir: &str) -> String {
    format!("marrow check {dir}")
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
        println!("ok: {dir} doctor found no findings");
    } else {
        for finding in findings {
            println!(
                "{}: {}; remedy: {}; next: `{}`",
                finding.code, finding.message, finding.remedy, finding.next_command
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
