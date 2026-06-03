use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_run::evolution::{ApplyError, ApplyOutcome};

use crate::{CheckFormat, report_simple_error, write_json};

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

pub(super) fn preview(witness: &EvolutionWitness, diagnostics: &[String], format: CheckFormat) {
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
                render_blocking_text(witness, diagnostics);
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
            "diagnostics": diagnostics,
            "blocking": blocking_json(witness, diagnostics),
        })),
    }
}

pub(super) fn blocked(witness: &EvolutionWitness, diagnostics: &[String], format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            render_blocking_text(witness, diagnostics);
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(report_envelope(&first_blocking_report(
                witness,
                diagnostics,
            )));
        }
    }
}

/// The shared error envelope for one blocking obligation: the stable code, its derived
/// `kind`, the message, and the structured facts nested under `data` as the envelope
/// spec requires.
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

fn render_blocking_text(witness: &EvolutionWitness, diagnostics: &[String]) {
    for report in blocking_reports(witness, diagnostics) {
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

fn first_blocking_report(witness: &EvolutionWitness, diagnostics: &[String]) -> BlockingReport {
    blocking_reports(witness, diagnostics)
        .into_iter()
        .next()
        .unwrap_or_else(generic_blocking_report)
}

fn blocking_json(witness: &EvolutionWitness, diagnostics: &[String]) -> Vec<serde_json::Value> {
    blocking_reports(witness, diagnostics)
        .iter()
        .map(report_envelope)
        .collect()
}

fn blocking_reports(witness: &EvolutionWitness, diagnostics: &[String]) -> Vec<BlockingReport> {
    let mut reports = Vec::new();
    let mut repair_diagnostics = diagnostics.iter();
    for obligation in &witness.verdicts {
        match &obligation.verdict {
            Verdict::RepairRequired { .. } => {
                reports.push(BlockingReport {
                    code: "evolve.repair_required",
                    message: repair_diagnostics.next().cloned().unwrap_or_else(|| {
                        format!(
                            "catalog id {} requires repair before activation",
                            obligation.catalog_id.as_str()
                        )
                    }),
                    catalog_id: Some(obligation.catalog_id.as_str().to_string()),
                    populated: None,
                });
            }
            Verdict::DestructiveDecisionRequired { populated } => {
                reports.push(BlockingReport {
                    code: "evolve.approval_required",
                    message: format!(
                        "catalog id {} retires {populated} populated record(s); rerun with --maintenance --approve-retire {}:{populated}",
                        obligation.catalog_id.as_str(),
                        obligation.catalog_id.as_str()
                    ),
                    catalog_id: Some(obligation.catalog_id.as_str().to_string()),
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

pub(super) fn apply_success(outcome: &ApplyOutcome, format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            println!("applied evolution");
            println!("catalog epoch: {}", outcome.catalog_epoch);
            println!("commit id: {}", outcome.committed_commit_id);
            println!("records backfilled: {}", outcome.records_backfilled);
            println!("records transformed: {}", outcome.records_transformed);
            println!("records retired: {}", outcome.records_retired);
            println!("indexes rebuilt: {}", outcome.indexes_rebuilt);
        }
        CheckFormat::Json | CheckFormat::Jsonl => write_json(serde_json::json!({
            "kind": "evolve_apply",
            "status": "applied",
            "catalog_epoch": outcome.catalog_epoch,
            "commit_id": outcome.committed_commit_id,
            "records_backfilled": outcome.records_backfilled,
            "records_transformed": outcome.records_transformed,
            "records_retired": outcome.records_retired,
            "indexes_rebuilt": outcome.indexes_rebuilt,
        })),
    }
}

pub(super) fn apply_error(error: ApplyError, format: CheckFormat) {
    match error {
        ApplyError::NoAcceptedCatalog => report_simple_error(
            "evolve.no_accepted_catalog",
            "this program accepted no catalog, so there is no baseline epoch to apply from; run `marrow catalog accept` first",
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
            &format!(
                "catalog id {} retires {populated} populated record(s); rerun with --maintenance --approve-retire {}:{populated}",
                catalog_id.as_str(),
                catalog_id.as_str()
            ),
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
