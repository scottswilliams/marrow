use std::collections::HashMap;

use marrow_check::evolution::{EvolutionWitness, RepairDiagnostic, Verdict};
use marrow_run::evolution::{ApplyError, ApplyOutcome};

use crate::{CheckFormat, report_simple_error, write_json};

pub(super) struct SourceLabels {
    by_catalog_id: HashMap<String, String>,
}

impl SourceLabels {
    pub(super) fn from_program(program: &marrow_check::CheckedProgram) -> Self {
        Self {
            by_catalog_id: program
                .catalog
                .accepted_entries
                .iter()
                .map(|entry| (entry.stable_id.clone(), source_label(&entry.path)))
                .collect(),
        }
    }

    fn catalog_id(&self, catalog_id: &str) -> String {
        self.by_catalog_id.get(catalog_id).map_or_else(
            || catalog_id.to_string(),
            |label| format!("{catalog_id} ({label})"),
        )
    }
}

fn source_label(path: &str) -> String {
    path.replace("::", ".")
}

pub(super) fn data_check_ok(dir: &str, witness: &EvolutionWitness, format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            println!("ok: {dir} checked with attached data");
            println!("records scanned: {}", witness.counts.scanned_records);
        }
        CheckFormat::Json | CheckFormat::Jsonl => write_json(serde_json::json!({
            "kind": "data_check",
            "status": "ok",
            "records_scanned": witness.counts.scanned_records,
        })),
    }
}

pub(super) fn preview(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => {
            println!("evolution preview");
            println!(
                "status: {}",
                if witness.is_activatable() {
                    "activatable"
                } else {
                    "blocked"
                }
            );
            println!("source digest: {}", witness.source_digest);
            println!("accepted epoch: {}", witness.accepted_catalog.epoch);
            if let Some(proposal) = &witness.proposal_catalog {
                println!("proposal epoch: {}", proposal.epoch);
            }
            println!("records scanned: {}", witness.counts.scanned_records);
            println!(
                "records to backfill: {}",
                witness.counts.records_to_backfill
            );
            println!(
                "records to transform: {}",
                witness.counts.records_to_transform
            );
            if !witness.is_activatable() {
                render_blocking_text(witness, diagnostics, labels);
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => write_json(serde_json::json!({
            "kind": "evolve_preview",
            "status": if witness.is_activatable() { "activatable" } else { "blocked" },
            "source_digest": witness.source_digest,
            "accepted_epoch": witness.accepted_catalog.epoch,
            "proposal_epoch": witness.proposal_catalog.as_ref().map(|catalog| catalog.epoch),
            "records_scanned": witness.counts.scanned_records,
            "records_to_backfill": witness.counts.records_to_backfill,
            "records_to_transform": witness.counts.records_to_transform,
            "diagnostics": diagnostics.iter().map(|diagnostic| &diagnostic.message).collect::<Vec<_>>(),
            "blocking": blocking_json(witness, diagnostics, labels),
        })),
    }
}

pub(super) fn blocked(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => {
            render_blocking_text(witness, diagnostics, labels);
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(report_envelope(&first_blocking_report(
                witness,
                diagnostics,
                labels,
            )));
        }
    }
}

/// One blocking obligation as an error envelope. Structured facts nest under `data`,
/// as the envelope spec requires.
fn report_envelope(report: &BlockingReport) -> serde_json::Value {
    serde_json::json!({
        "code": report.code,
        "kind": marrow_syntax::kind_for_code(report.code),
        "message": report.message,
        "data": {
            "catalog_id": report.catalog_id,
            "populated": report.populated,
        },
        "source_span": null,
    })
}

fn render_blocking_text(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
) {
    for report in blocking_reports(witness, diagnostics, labels) {
        eprintln!("{}: {}", report.code, report.message);
    }
}

#[derive(Debug, Clone)]
struct BlockingReport {
    code: &'static str,
    message: String,
    catalog_id: Option<String>,
    populated: Option<usize>,
}

fn first_blocking_report(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
) -> BlockingReport {
    blocking_reports(witness, diagnostics, labels)
        .into_iter()
        .next()
        .unwrap_or_else(generic_blocking_report)
}

fn blocking_json(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
) -> Vec<serde_json::Value> {
    blocking_reports(witness, diagnostics, labels)
        .iter()
        .map(report_envelope)
        .collect()
}

fn blocking_reports(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
) -> Vec<BlockingReport> {
    let messages: HashMap<&str, &str> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.catalog_id.as_str(), diagnostic.message.as_str()))
        .collect();
    let mut reports = Vec::new();
    for obligation in &witness.verdicts {
        match &obligation.verdict {
            Verdict::RepairRequired { .. } => {
                let catalog_id = obligation.catalog_id.as_str();
                reports.push(BlockingReport {
                    code: "evolve.repair_required",
                    message: messages.get(catalog_id).map_or_else(
                        || format!("catalog id {catalog_id} requires repair before activation"),
                        |m| m.to_string(),
                    ),
                    catalog_id: Some(catalog_id.to_string()),
                    populated: None,
                });
            }
            Verdict::DestructiveDecisionRequired { populated } => {
                let catalog_id = obligation.catalog_id.as_str();
                reports.push(BlockingReport {
                    code: "evolve.approval_required",
                    message: approval_required_message(catalog_id, *populated, labels),
                    catalog_id: Some(catalog_id.to_string()),
                    populated: Some(*populated),
                });
            }
            _ => {}
        }
    }
    if reports.is_empty() && !witness.is_activatable() {
        reports.push(generic_blocking_report());
    }
    reports
}

fn generic_blocking_report() -> BlockingReport {
    BlockingReport {
        code: "evolve.repair_required",
        message: "evolution witness is not activatable".to_string(),
        catalog_id: None,
        populated: None,
    }
}

/// The `evolve.approval_required` prose, shared by the preview's blocking report and the
/// apply error so both name the same retire-approval invocation for a destructive evolution.
fn approval_required_message(catalog_id: &str, populated: usize, labels: &SourceLabels) -> String {
    let display_id = labels.catalog_id(catalog_id);
    format!(
        "catalog id {display_id} retires {populated} populated record(s); rerun with --maintenance --approve-retire {catalog_id}:{populated}"
    )
}

/// Report a committed evolution apply: the activated epoch, the fresh commit id, and the
/// per-kind record counts the receipt proves.
pub(super) fn apply_success(outcome: &ApplyOutcome, format: CheckFormat) {
    let receipt = &outcome.receipt;
    match format {
        CheckFormat::Text => {
            println!("applied evolution");
            println!("catalog epoch: {}", receipt.catalog_epoch);
            println!("commit id: {}", receipt.commit_id);
            println!("records backfilled: {}", receipt.records_backfilled);
            println!("records transformed: {}", receipt.records_transformed);
            println!("records retired: {}", receipt.records_retired);
            println!("indexes rebuilt: {}", receipt.indexes_rebuilt);
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(serde_json::json!({
                "kind": "evolve_apply",
                "status": "applied",
                "catalog_epoch": receipt.catalog_epoch,
                "commit_id": receipt.commit_id,
                "records_backfilled": receipt.records_backfilled,
                "records_transformed": receipt.records_transformed,
                "records_retired": receipt.records_retired,
                "indexes_rebuilt": receipt.indexes_rebuilt,
            }));
        }
    }
}

pub(super) fn apply_error(error: ApplyError, labels: &SourceLabels, format: CheckFormat) {
    match error {
        ApplyError::NoAcceptedCatalog => report_simple_error(
            "evolve.no_accepted_catalog",
            "this program has no durable catalog to apply from; it declares no saved data, so there is no baseline epoch to advance",
            format,
        ),
        ApplyError::Drift => report_simple_error(
            "evolve.drift",
            "the live source, catalog, store snapshot, or counts no longer match the preview witness",
            format,
        ),
        ApplyError::StoreCommitDrift { pinned, found } => report_simple_error(
            "evolve.store_commit_drift",
            &format!("store commit changed after preview (pinned {pinned:?}, found {found:?})"),
            format,
        ),
        ApplyError::CatalogDrift { pinned, found } => report_simple_error(
            "evolve.catalog_drift",
            &format!(
                "store accepted catalog changed after preview (pinned {pinned}, found {found:?})"
            ),
            format,
        ),
        ApplyError::MaintenanceRequired => report_simple_error(
            "evolve.maintenance_required",
            "destructive evolution apply requires --maintenance",
            format,
        ),
        ApplyError::ApprovalRequired {
            catalog_id,
            populated,
        } => report_simple_error(
            "evolve.approval_required",
            &approval_required_message(catalog_id.as_str(), populated, labels),
            format,
        ),
        ApplyError::ApprovalMismatch => report_simple_error(
            "evolve.approval_mismatch",
            "destructive approval did not match the preview witness",
            format,
        ),
        ApplyError::PlanMismatch { expected, staged } => report_simple_error(
            "evolve.plan_mismatch",
            &format!("staged {staged} item(s), but the witness expected {expected}"),
            format,
        ),
        ApplyError::TransformBodyFaulted { target, reason } => report_simple_error(
            "evolve.transform_faulted",
            &format!(
                "transform for catalog id {} failed: {reason}",
                target.as_str()
            ),
            format,
        ),
        ApplyError::Fenced(error) => report_simple_error(error.code(), &error.message(), format),
        ApplyError::Store(error) => report_simple_error(error.code(), &error.to_string(), format),
        ApplyError::NotActivatable => report_simple_error(
            "evolve.repair_required",
            "evolution witness is not activatable",
            format,
        ),
    }
}
