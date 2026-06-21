use std::collections::HashMap;

use marrow_check::evolution::{
    EvolutionWitness, RejectedDefault, RepairDiagnostic, RepairReason, Verdict,
};
use marrow_check::{CheckedModule, ResourceSchema, ScalarType, Type};
use marrow_run::evolution::{ApplyError, ApplyOutcome};

use crate::{CheckFormat, report_simple_error, report_simple_error_with_data, write_json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RecoveryPoint {
    None,
    Backup { path: String },
    NoBackup,
}

pub(super) struct SourceLabels {
    by_catalog_id: HashMap<String, SourceTarget>,
}

struct SourceTarget {
    display: String,
    scaffold: String,
    /// The type-correct constant an `evolve default` scaffold backfills this target with,
    /// for a scalar resource member; `None` for a target with no defaultable leaf type
    /// (a store root, index, enum, or a non-scalar member).
    default_literal: Option<&'static str>,
}

impl SourceLabels {
    pub(super) fn from_program(program: &marrow_check::CheckedProgram) -> Self {
        let mut by_catalog_id = HashMap::new();
        for entry in &program.catalog.accepted_entries {
            by_catalog_id.insert(
                entry.stable_id.clone(),
                SourceTarget::new(program, &entry.path),
            );
        }
        if let Some(proposal) = &program.catalog.proposal {
            for entry in &proposal.entries {
                by_catalog_id
                    .entry(entry.stable_id.clone())
                    .or_insert_with(|| SourceTarget::new(program, &entry.path));
            }
        }
        Self { by_catalog_id }
    }

    fn catalog_id(&self, catalog_id: &str) -> String {
        self.by_catalog_id.get(catalog_id).map_or_else(
            || catalog_id.to_string(),
            |target| format!("{catalog_id} ({})", target.display),
        )
    }

    fn scaffold_target(&self, catalog_id: &str) -> String {
        self.by_catalog_id
            .get(catalog_id)
            .map_or_else(|| catalog_id.to_string(), |target| target.scaffold.clone())
    }

    /// The type-correct constant an `evolve default` scaffold for this target backfills.
    /// `0` is the safe fallback when a target's leaf type cannot be resolved; a real `default`
    /// or transform target always resolves in current source, so this is defensive only.
    fn default_literal(&self, catalog_id: &str) -> &'static str {
        self.by_catalog_id
            .get(catalog_id)
            .and_then(|target| target.default_literal)
            .unwrap_or("0")
    }
}

impl SourceTarget {
    fn new(program: &marrow_check::CheckedProgram, path: &str) -> Self {
        Self {
            display: source_label(path),
            scaffold: scaffold_target(program, path),
            default_literal: member_default_literal(program, path),
        }
    }
}

fn source_label(path: &str) -> String {
    path.replace("::", ".")
}

/// Source spelling for an evolve scaffold target: the resource-qualified member path the
/// checker resolves (`Book.pages`), with the owning module prefix dropped. The module can be
/// several segments (`shop::books`), so the whole module name is stripped, not just its first
/// segment. Store roots and indexes carry their caret inside the catalog path segment
/// (`shop::books::^books`, `shop::books::^books::byShelf`), so joining the remaining segments
/// with a dot yields the correct `^books` / `^books.byShelf` spelling. A path the program's
/// modules do not own falls back to the full dotted path.
fn scaffold_target(program: &marrow_check::CheckedProgram, path: &str) -> String {
    match owned_path(program, path) {
        Some((_, local)) if !local.is_empty() => local.join("."),
        _ => source_label(path),
    }
}

/// The module that owns a catalog path and the path segments below its module prefix:
/// `[Resource, member...]` for a member, `[^store]` for a store root, `[^store, index]` for
/// an index. The module whose name is the longest `::`-segment prefix wins, so a nested
/// module (`shop::books`) is not mistaken for a shorter sibling (`shop`) whose name also
/// prefixes the path text. `None` when no module owns the path.
fn owned_path<'a>(
    program: &'a marrow_check::CheckedProgram,
    path: &'a str,
) -> Option<(&'a CheckedModule, Vec<&'a str>)> {
    let module = program
        .modules
        .iter()
        .filter(|module| {
            path == module.name
                || path
                    .strip_prefix(&module.name)
                    .is_some_and(|rest| rest.starts_with("::"))
        })
        .max_by_key(|module| module.name.len())?;
    let local = path.strip_prefix(&module.name)?.strip_prefix("::")?;
    Some((module, local.split("::").collect()))
}

/// The type-correct `evolve default` constant for a scalar resource member, resolved from
/// its leaf type. `None` for a store root, index, enum, or a non-scalar member, none of
/// which a `default` scaffold targets with a constant.
fn member_default_literal(
    program: &marrow_check::CheckedProgram,
    path: &str,
) -> Option<&'static str> {
    let (module, local) = owned_path(program, path)?;
    let (resource_name, member_chain) = local.split_first()?;
    if member_chain.is_empty() {
        return None;
    }
    let resource = module
        .resources
        .iter()
        .find(|resource: &&ResourceSchema| resource.name == *resource_name)?;
    match resource.field_type(member_chain)? {
        Type::Scalar(scalar) => Some(default_literal(*scalar)),
        _ => None,
    }
}

/// A valid `.mw` constant literal of each scalar type for a `default` scaffold. The temporal
/// and bytes forms use the validating constructor over a canonical-form string, the only
/// constant the const-default evaluator carries for those types; the placeholders are the
/// canonical zero of each type a developer then edits to a real fill.
fn default_literal(scalar: ScalarType) -> &'static str {
    match scalar {
        ScalarType::Int => "0",
        ScalarType::Bool => "false",
        ScalarType::Str => "\"\"",
        ScalarType::Decimal => "0.0",
        ScalarType::Bytes => "bytes(\"\")",
        ScalarType::Date => "date(\"1970-01-01\")",
        ScalarType::Instant => "instant(\"1970-01-01T00:00:00Z\")",
        ScalarType::Duration => "duration(\"PT0S\")",
    }
}

fn nothing_to_discharge(witness: &EvolutionWitness) -> bool {
    witness.counts.records_to_backfill == 0
        && witness.counts.records_to_transform == 0
        && witness
            .verdicts
            .iter()
            .all(|obligation| discharge_is_no_work(&obligation.verdict))
}

fn discharge_is_no_work(verdict: &Verdict) -> bool {
    matches!(
        verdict,
        Verdict::NoOp | Verdict::CatalogOnly | Verdict::DataProof
    )
}

pub(super) fn preview(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
    format: CheckFormat,
    scaffold: bool,
) {
    match format {
        CheckFormat::Text if scaffold => {
            print!("{}", scaffold_source(witness, labels));
            if !witness.is_activatable() {
                render_blocking_text(witness, diagnostics, labels);
            }
        }
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
            if nothing_to_discharge(witness) {
                println!("nothing to discharge");
            }
            if !witness.is_activatable() {
                render_blocking_text(witness, diagnostics, labels);
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            let mut object = serde_json::Map::from_iter([
                ("kind".to_string(), serde_json::json!("evolve_preview")),
                (
                    "status".to_string(),
                    serde_json::json!(if witness.is_activatable() {
                        "activatable"
                    } else {
                        "blocked"
                    }),
                ),
                (
                    "source_digest".to_string(),
                    serde_json::json!(witness.source_digest),
                ),
                (
                    "accepted_epoch".to_string(),
                    serde_json::json!(witness.accepted_catalog.epoch),
                ),
                (
                    "proposal_epoch".to_string(),
                    serde_json::json!(
                        witness
                            .proposal_catalog
                            .as_ref()
                            .map(|catalog| catalog.epoch)
                    ),
                ),
                (
                    "records_scanned".to_string(),
                    serde_json::json!(witness.counts.scanned_records),
                ),
                (
                    "records_to_backfill".to_string(),
                    serde_json::json!(witness.counts.records_to_backfill),
                ),
                (
                    "records_to_transform".to_string(),
                    serde_json::json!(witness.counts.records_to_transform),
                ),
                (
                    "nothing_to_discharge".to_string(),
                    serde_json::json!(nothing_to_discharge(witness)),
                ),
                (
                    "diagnostics".to_string(),
                    serde_json::json!(
                        diagnostics
                            .iter()
                            .map(|diagnostic| &diagnostic.message)
                            .collect::<Vec<_>>()
                    ),
                ),
                (
                    "blocking".to_string(),
                    serde_json::json!(blocking_json(witness, diagnostics, labels)),
                ),
            ]);
            if scaffold {
                object.insert(
                    "scaffold".to_string(),
                    serde_json::json!(scaffold_source(witness, labels)),
                );
            }
            write_json(serde_json::Value::Object(object));
        }
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
    eprintln!(
        "hint: run `marrow evolve preview --scaffold <projectdir>` to print parseable evolve blocks"
    );
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

fn scaffold_source(witness: &EvolutionWitness, labels: &SourceLabels) -> String {
    let blocks: Vec<String> = witness
        .verdicts
        .iter()
        .filter_map(|obligation| {
            scaffold_block(obligation.catalog_id.as_str(), &obligation.verdict, labels)
        })
        .collect();
    if blocks.is_empty() {
        return String::new();
    }
    let raw = blocks.join("\n");
    let formatted = marrow_syntax::format_source(&raw);
    debug_assert!(
        !marrow_syntax::parse_source(&formatted).has_errors(),
        "evolve scaffold must parse after formatting"
    );
    formatted
}

fn scaffold_block(catalog_id: &str, verdict: &Verdict, labels: &SourceLabels) -> Option<String> {
    match verdict {
        Verdict::DestructiveDecisionRequired { populated } => {
            Some(retire_scaffold(catalog_id, *populated, labels))
        }
        Verdict::RepairRequired { reason } => repair_scaffold(catalog_id, reason, labels),
        _ => None,
    }
}

fn repair_scaffold(
    catalog_id: &str,
    reason: &RepairReason,
    labels: &SourceLabels,
) -> Option<String> {
    match reason {
        RepairReason::MissingRequiredMember
        | RepairReason::DefaultRejected {
            reason: RejectedDefault::TypeMismatch | RejectedDefault::NotEncodable,
        } => Some(default_scaffold(catalog_id, labels)),
        RepairReason::DefaultRejected {
            reason: RejectedDefault::NotConstant,
        }
        | RepairReason::TypeChangeRequiresTransform
        | RepairReason::UndecodableTransformInput => Some(transform_scaffold(catalog_id, labels)),
        RepairReason::PopulatedDropRequiresRetire | RepairReason::RetireRequired { .. } => {
            Some(retire_scaffold(catalog_id, 0, labels))
        }
        _ => None,
    }
}

fn retire_scaffold(catalog_id: &str, populated: usize, labels: &SourceLabels) -> String {
    let target = labels.scaffold_target(catalog_id);
    format!(
        "evolve\n    retire {target}\n    ; approve with marrow evolve apply --maintenance --approve-retire {catalog_id}:{populated} (--backup <backup-file> | --no-backup) <projectdir>\n"
    )
}

fn default_scaffold(catalog_id: &str, labels: &SourceLabels) -> String {
    let target = labels.scaffold_target(catalog_id);
    let value = labels.default_literal(catalog_id);
    format!("evolve\n    default {target} = {value}\n")
}

fn transform_scaffold(catalog_id: &str, labels: &SourceLabels) -> String {
    let target = labels.scaffold_target(catalog_id);
    let value = labels.default_literal(catalog_id);
    format!("evolve\n    transform {target}\n        return {value}\n")
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
        "catalog id {display_id} retires {populated} populated record(s); rerun with --maintenance --approve-retire {catalog_id}:{populated} --backup <backup-file> (or --no-backup to opt out)"
    )
}

/// Report a committed evolution apply: the activated epoch, the fresh commit id, and the
/// per-kind record counts the receipt proves.
pub(super) fn apply_success(outcome: &ApplyOutcome, recovery: &RecoveryPoint, format: CheckFormat) {
    let receipt = &outcome.receipt;
    let nothing_to_apply = receipt.store_commit_id_before == Some(receipt.commit_id)
        && receipt.records_backfilled == 0
        && receipt.records_transformed == 0
        && receipt.records_retired == 0
        && receipt.indexes_rebuilt == 0;
    match format {
        CheckFormat::Text if nothing_to_apply => {
            println!("nothing to apply");
            println!("catalog epoch: {}", receipt.catalog_epoch);
            println!("commit id: {}", receipt.commit_id);
            render_recovery_text(recovery);
        }
        CheckFormat::Text => {
            println!("applied evolution");
            println!("catalog epoch: {}", receipt.catalog_epoch);
            println!("commit id: {}", receipt.commit_id);
            println!("records backfilled: {}", receipt.records_backfilled);
            println!("records transformed: {}", receipt.records_transformed);
            println!("records retired: {}", receipt.records_retired);
            println!("indexes rebuilt: {}", receipt.indexes_rebuilt);
            render_recovery_text(recovery);
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            let mut object = serde_json::Map::from_iter([
                ("kind".to_string(), serde_json::json!("evolve_apply")),
                ("status".to_string(), serde_json::json!("applied")),
                (
                    "catalog_epoch".to_string(),
                    serde_json::json!(receipt.catalog_epoch),
                ),
                (
                    "commit_id".to_string(),
                    serde_json::json!(receipt.commit_id),
                ),
                (
                    "records_backfilled".to_string(),
                    serde_json::json!(receipt.records_backfilled),
                ),
                (
                    "records_transformed".to_string(),
                    serde_json::json!(receipt.records_transformed),
                ),
                (
                    "records_retired".to_string(),
                    serde_json::json!(receipt.records_retired),
                ),
                (
                    "indexes_rebuilt".to_string(),
                    serde_json::json!(receipt.indexes_rebuilt),
                ),
            ]);
            if let Some(value) = recovery_json(recovery) {
                object.insert("recovery_point".to_string(), value);
            }
            write_json(serde_json::Value::Object(object));
        }
    }
}

fn render_recovery_text(recovery: &RecoveryPoint) {
    match recovery {
        RecoveryPoint::None => {}
        RecoveryPoint::Backup { path } => println!("recovery point: backup {path}"),
        RecoveryPoint::NoBackup => println!("recovery point: no-backup"),
    }
}

fn recovery_json(recovery: &RecoveryPoint) -> Option<serde_json::Value> {
    match recovery {
        RecoveryPoint::None => None,
        RecoveryPoint::Backup { path } => {
            Some(serde_json::json!({ "kind": "backup", "path": path }))
        }
        RecoveryPoint::NoBackup => Some(serde_json::json!({ "kind": "no_backup" })),
    }
}

pub(super) fn requires_backup(format: CheckFormat) {
    report_simple_error(
        "evolve.requires_backup",
        "destructive retire apply requires --backup <path> or explicit --no-backup",
        format,
    );
}

pub(super) fn apply_error(error: ApplyError, labels: &SourceLabels, format: CheckFormat) {
    match error {
        ApplyError::NoAcceptedCatalog => report_simple_error(
            "evolve.no_accepted_catalog",
            "this program has no durable catalog to apply from; it declares no saved data, so there is no baseline epoch to advance",
            format,
        ),
        ApplyError::Drift => report_drift_error(
            drift_kind("witness"),
            "the live source, catalog, store snapshot, or counts no longer match the preview witness; rerun `marrow evolve preview`, then rerun `marrow evolve apply`",
            format,
        ),
        ApplyError::StoreCommitDrift { pinned, found } => report_drift_error(
            drift_kind_with_fields(
                "store_commit",
                [
                    ("pinned", serde_json::json!(pinned)),
                    ("found", serde_json::json!(found)),
                ],
            ),
            &format!(
                "store commit changed after preview (pinned {pinned:?}, found {found:?}); rerun `marrow evolve preview`, then rerun `marrow evolve apply`"
            ),
            format,
        ),
        ApplyError::CatalogDrift { pinned, found } => report_simple_error(
            "evolve.catalog_drift",
            &format!(
                "store accepted catalog changed after preview (pinned {pinned}, found {found:?}); rerun `marrow evolve preview`, then rerun `marrow evolve apply`"
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
        ApplyError::PlanMismatch { expected, staged } => report_drift_error(
            drift_kind_with_fields(
                "plan_mismatch",
                [
                    ("expected", serde_json::json!(expected)),
                    ("staged", serde_json::json!(staged)),
                ],
            ),
            &format!("staged {staged} item(s), but the witness expected {expected}"),
            format,
        ),
        ApplyError::TransformBodyFaulted {
            target,
            record,
            inner_code,
            reason,
        } => report_simple_error_with_data(
            "evolve.transform_faulted",
            &format!(
                "transform for {} faulted on record {record} ({inner_code}): {reason}",
                labels.catalog_id(target.as_str())
            ),
            serde_json::Map::from_iter([
                ("target".to_string(), serde_json::json!(target.as_str())),
                ("record".to_string(), serde_json::json!(record)),
                ("inner_code".to_string(), serde_json::json!(inner_code)),
            ]),
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

fn drift_kind(kind: &str) -> serde_json::Value {
    drift_kind_with_fields(kind, [])
}

fn drift_kind_with_fields<const N: usize>(
    kind: &str,
    fields: [(&str, serde_json::Value); N],
) -> serde_json::Value {
    let mut object = serde_json::Map::from_iter([("kind".to_string(), serde_json::json!(kind))]);
    for (name, value) in fields {
        object.insert(name.to_string(), value);
    }
    serde_json::Value::Object(object)
}

fn report_drift_error(drift_kind: serde_json::Value, message: &str, format: CheckFormat) {
    report_simple_error_with_data(
        "evolve.drift",
        message,
        serde_json::Map::from_iter([("drift_kind".to_string(), drift_kind)]),
        format,
    );
}
