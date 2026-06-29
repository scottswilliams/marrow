use std::collections::{HashMap, HashSet};

use marrow_check::evolution::{
    EvolutionWitness, RejectedDefault, RepairDiagnostic, RepairGuidance, RepairReason, Verdict,
};
use marrow_check::{CheckedModule, ResourceSchema, ScalarType, Type};
use marrow_run::evolution::{ApplyError, ApplyOutcome};

use crate::term_style::{self, Stream, Style};
use crate::{CheckFormat, report_simple_error, report_simple_error_with_data, write_json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RecoveryPoint {
    None,
    Backup { path: String },
    NoBackup,
}

pub(super) struct SourceLabels {
    by_catalog_id: HashMap<String, SourceTarget>,
    /// Source-spelling keyed by catalog path, for targets a renderer holds as a path rather
    /// than a catalog id — the `from`/`to` of a rename guidance carry catalog paths, not ids.
    by_source_path: HashMap<String, String>,
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
        let mut by_source_path = HashMap::new();
        for entry in &program.catalog.accepted_entries {
            by_catalog_id.insert(
                entry.stable_id.clone(),
                SourceTarget::new(program, &entry.path),
            );
            by_source_path
                .entry(entry.path.clone())
                .or_insert_with(|| entry_source_spelling(program, entry));
        }
        if let Some(proposal) = &program.catalog.proposal {
            for entry in &proposal.entries {
                by_catalog_id
                    .entry(entry.stable_id.clone())
                    .or_insert_with(|| SourceTarget::new(program, &entry.path));
                by_source_path
                    .entry(entry.path.clone())
                    .or_insert_with(|| entry_source_spelling(program, entry));
            }
        }
        Self {
            by_catalog_id,
            by_source_path,
        }
    }

    /// The source spelling of a catalog path (`books::Book::tagline` -> `Book.tagline`), or the
    /// dotted catalog path when the program declares no such target.
    fn source_path_spelling(&self, path: &str) -> String {
        self.by_source_path
            .get(path)
            .cloned()
            .unwrap_or_else(|| source_label(path))
    }

    fn catalog_id(&self, catalog_id: &str) -> String {
        self.by_catalog_id.get(catalog_id).map_or_else(
            || catalog_id.to_string(),
            |target| format!("{catalog_id} ({})", target.display),
        )
    }

    /// The dotted source spelling (`Book.author`) for a catalog id, for naming what a retire
    /// removes in prose.
    fn display(&self, catalog_id: &str) -> String {
        self.by_catalog_id
            .get(catalog_id)
            .map_or_else(|| catalog_id.to_string(), |target| target.display.clone())
    }

    /// The resource-qualified field path (`Book.pages`) for a catalog id: the form a paste-ready
    /// scaffold spells and the form `--approve-retire` accepts and the reference documents. Falls
    /// back to the catalog id when the program declares no such entry.
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

/// The source spelling of a catalog entry. An enum member reads as `Enum::member` (its value
/// surface form), every other entry as the dotted resource/store form. The kind is the one signal
/// that distinguishes an `Enum::member` value path from a dotted `Resource.member` path, since
/// both carry the same module prefix.
fn entry_source_spelling(
    program: &marrow_check::CheckedProgram,
    entry: &marrow_catalog::CatalogEntry,
) -> String {
    match entry.kind {
        marrow_catalog::CatalogEntryKind::EnumMember => enum_member_spelling(program, &entry.path),
        _ => scaffold_target(program, &entry.path),
    }
}

/// The `Enum::member` source spelling of an enum-member catalog path, dropping the owning module
/// prefix. Falls back to the dotted path when no module owns it.
fn enum_member_spelling(program: &marrow_check::CheckedProgram, path: &str) -> String {
    match owned_path(program, path) {
        Some((_, local)) if !local.is_empty() => local.join("::"),
        _ => source_label(path),
    }
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
/// prefixes the path text. A single-file script declares no `module`, so its module's name is
/// empty and its catalog paths carry no prefix; that module owns the whole path as-is. `None`
/// when no module owns the path.
pub(super) fn owned_path<'a>(
    program: &'a marrow_check::CheckedProgram,
    path: &'a str,
) -> Option<(&'a CheckedModule, Vec<&'a str>)> {
    let module = program
        .modules
        .iter()
        .filter(|module| owns_path(&module.name, path))
        .max_by_key(|module| module.name.len())?;
    let local = local_segments(&module.name, path)?;
    Some((module, local))
}

/// Whether a module named `name` owns `path`: the unnamed module of a single-file script owns
/// every path, and a named module owns the path it equals or `::`-prefixes.
fn owns_path(name: &str, path: &str) -> bool {
    name.is_empty()
        || path == name
        || path
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with("::"))
}

/// The `::`-split segments of `path` below the prefix module `name`. The unnamed module of a
/// single-file script contributes no prefix, so the whole path is local.
fn local_segments<'a>(name: &str, path: &'a str) -> Option<Vec<&'a str>> {
    let local = if name.is_empty() {
        path
    } else {
        path.strip_prefix(name)?.strip_prefix("::")?
    };
    Some(local.split("::").collect())
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
        && witness.counts.records_to_readdress == 0
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
    dir: &str,
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
    format: CheckFormat,
    scaffold: bool,
) {
    match format {
        CheckFormat::Text if scaffold => {
            let body = scaffold_source(witness, diagnostics, labels);
            print!("{body}");
            if !witness.is_activatable() {
                let footer = if body.is_empty() {
                    ScaffoldFooter::NoBlock
                } else {
                    ScaffoldFooter::PasteBlock
                };
                render_blocking_text(witness, diagnostics, labels, footer, dir);
            }
        }
        CheckFormat::Text => {
            if !witness.is_activatable() {
                println!("This evolution cannot be applied yet:");
                render_blocking_text(witness, diagnostics, labels, ScaffoldFooter::Hint, dir);
            } else if nothing_to_discharge(witness) {
                // The store already matches the source, so a repeat apply would be a no-op.
                // The text surface must agree with the JSON surface's `nothing_to_discharge`
                // and must not recommend the no-op apply.
                println!("Nothing to discharge; the store already matches your source.");
            } else {
                let backfill = witness.counts.records_to_backfill;
                let transform = witness.counts.records_to_transform;
                println!("This evolution is safe to apply.");
                println!("records to backfill: {backfill}");
                println!("records to transform: {transform}");
                println!(
                    "{} marrow evolve apply {dir}",
                    term_style::paint(Stream::Stdout, Style::Warning, "Next:")
                );
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
                    serde_json::json!(scaffold_source(witness, diagnostics, labels)),
                );
            }
            write_json(serde_json::Value::Object(object));
        }
    }
}

pub(super) fn blocked(
    dir: &str,
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => {
            render_blocking_text(witness, diagnostics, labels, ScaffoldFooter::Hint, dir);
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

/// The footer a blocked preview prints after its blocking reports. `--scaffold` that emitted a
/// block points the developer at pasting it; `--scaffold` that synthesized nothing prints no
/// footer, since there is no block to paste and re-suggesting the flag would loop; a flagless
/// preview points at `--scaffold` to print the blocks.
enum ScaffoldFooter {
    PasteBlock,
    NoBlock,
    Hint,
}

fn render_blocking_text(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
    footer: ScaffoldFooter,
    dir: &str,
) {
    for report in blocking_reports(witness, diagnostics, labels) {
        eprintln!(
            "{}: {}",
            term_style::paint(Stream::Stderr, Style::Code, report.code),
            report.message
        );
    }
    match footer {
        // A retire scaffold is gated on --maintenance, --approve-retire, and a recovery choice, so
        // a flagless apply is rejected: name the same complete command the scaffold body's Step 3
        // teaches, with the same `<count>` placeholder (the count is unknown until the retire block
        // is in source). A non-destructive evolution applies with the bare command.
        ScaffoldFooter::PasteBlock => {
            let next = match retire_scaffold_targets(witness, diagnostics, labels).as_slice() {
                [] => format!("run `marrow evolve apply {dir}`"),
                targets => format!(
                    "run `marrow evolve apply --maintenance {} (--backup <backup-file> | --no-backup) {dir}`",
                    targets
                        .iter()
                        .map(|target| format!("--approve-retire {target}:<count>"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            };
            eprintln!(
                "{} paste the evolve block above into your source, then {next}",
                term_style::paint(Stream::Stderr, Style::Warning, "next:")
            );
        }
        ScaffoldFooter::NoBlock => {}
        ScaffoldFooter::Hint => {
            eprintln!(
                "{} run `marrow evolve preview --scaffold {dir}` to print parseable evolve blocks",
                term_style::paint(Stream::Stderr, Style::Warning, "hint:")
            );
        }
    }
}

/// The source-path targets whose scaffold block is a `retire` — every obligation `scaffold_block`
/// renders as a retire. A bare drop with a same-shape rename target scaffolds the identity-keeping
/// rename instead, so it does not appear here. The footer names these so it stays consistent with
/// the body's Step 3, which is the gated `--maintenance --approve-retire` command, not a flagless
/// apply the retire gates reject.
fn retire_scaffold_targets(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
) -> Vec<String> {
    let guidance: HashMap<&str, &RepairGuidance> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.catalog_id.as_str(), &diagnostic.guidance))
        .collect();
    witness
        .verdicts
        .iter()
        .filter(|obligation| {
            obligation_scaffolds_retire(
                &obligation.verdict,
                guidance.get(obligation.catalog_id.as_str()).copied(),
            )
        })
        .map(|obligation| labels.scaffold_target(obligation.catalog_id.as_str()))
        .collect()
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

fn scaffold_source(
    witness: &EvolutionWitness,
    diagnostics: &[RepairDiagnostic],
    labels: &SourceLabels,
) -> String {
    let guidance: HashMap<&str, &RepairGuidance> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.catalog_id.as_str(), &diagnostic.guidance))
        .collect();
    // A bare rename surfaces as two obligations: the dropped source field, whose guidance
    // names the rename target, and the added target field, whose own missing-required
    // obligation would otherwise scaffold a `default <target> = empty`. That default runs
    // before the rename and wipes every record the rename carries over, so the target's
    // default block is dropped here in favor of the identity-preserving rename alone.
    let rename_targets: HashSet<String> = diagnostics
        .iter()
        .filter_map(|diagnostic| match &diagnostic.guidance {
            RepairGuidance::RenameOrRetire { to, .. } => Some(labels.source_path_spelling(to)),
            _ => None,
        })
        .collect();
    let blocks: Vec<String> = witness
        .verdicts
        .iter()
        .filter_map(|obligation| {
            let catalog_id = obligation.catalog_id.as_str();
            if rename_targets.contains(&labels.scaffold_target(catalog_id)) {
                return None;
            }
            scaffold_block(
                catalog_id,
                &obligation.verdict,
                guidance.get(catalog_id).copied(),
                labels,
            )
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

fn scaffold_block(
    catalog_id: &str,
    verdict: &Verdict,
    guidance: Option<&RepairGuidance>,
    labels: &SourceLabels,
) -> Option<String> {
    match verdict {
        Verdict::DestructiveDecisionRequired { .. } => Some(retire_scaffold(catalog_id, labels)),
        Verdict::RepairRequired { reason } => repair_scaffold(catalog_id, reason, guidance, labels),
        _ => None,
    }
}

fn repair_scaffold(
    catalog_id: &str,
    reason: &RepairReason,
    guidance: Option<&RepairGuidance>,
    labels: &SourceLabels,
) -> Option<String> {
    match reason {
        RepairReason::MissingRequiredMember
        | RepairReason::DefaultRejected {
            reason: RejectedDefault::TypeMismatch | RejectedDefault::NotEncodable,
        } => Some(default_scaffold(catalog_id, labels)),
        // A populated leaf retype can never be discharged in place: reading the member's old
        // bytes as the new type would silently reinterpret or drop them, so the supported path
        // is to add a member of the new type, populate it with a transform from this one, then
        // retire this one. A runnable in-place `transform` skeleton would invite exactly the
        // silent data loss the contract forbids, so the scaffold emits a commented migration
        // the author must complete instead.
        RepairReason::TypeChangeRequiresTransform => Some(leaf_retype_skeleton(catalog_id, labels)),
        RepairReason::DefaultRejected {
            reason: RejectedDefault::NotConstant,
        }
        | RepairReason::UndecodableTransformInput => Some(transform_scaffold(catalog_id, labels)),
        // A bare drop whose check-time guidance found a single plausible same-shape rename
        // target scaffolds the identity-preserving rename rather than a destructive retire.
        RepairReason::PopulatedDropRequiresRetire | RepairReason::RetireRequired { .. } => {
            match guidance {
                Some(RepairGuidance::RenameOrRetire { from, to }) => {
                    Some(rename_scaffold(from, to, labels))
                }
                _ => Some(retire_scaffold(catalog_id, labels)),
            }
        }
        // A stored enum value naming a member the enum renamed away scaffolds the
        // identity-preserving rename when the check inferred a single same-shape candidate.
        // With no rename candidate the orphaned value has no paste-ready block — a transform
        // is record-specific source the developer must write — so it synthesizes nothing.
        RepairReason::InvalidStoredValue => match guidance {
            Some(RepairGuidance::RenameOrRetire { from, to }) => {
                Some(rename_scaffold(from, to, labels))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Whether `scaffold_block` renders an obligation as a `retire`: a destructive decision always
/// does, and a populated drop or explicit retire-required does unless its guidance offers a
/// same-shape rename to scaffold instead. The footer routes through this so it names the gated
/// retire command for exactly the obligations the body scaffolds as a retire.
fn obligation_scaffolds_retire(verdict: &Verdict, guidance: Option<&RepairGuidance>) -> bool {
    match verdict {
        Verdict::DestructiveDecisionRequired { .. } => true,
        Verdict::RepairRequired {
            reason: RepairReason::PopulatedDropRequiresRetire | RepairReason::RetireRequired { .. },
        } => !is_rename_guidance(guidance),
        _ => false,
    }
}

/// Whether check-time guidance offers a same-shape rename — the signal a populated drop scaffolds
/// the identity-preserving rename rather than a destructive retire.
fn is_rename_guidance(guidance: Option<&RepairGuidance>) -> bool {
    matches!(guidance, Some(RepairGuidance::RenameOrRetire { .. }))
}

/// The two-step retire scaffold. The first step is the parseable `evolve { retire ... }` block to
/// paste into source; the second is a commented instruction to preview for the exact populated
/// count and then apply with that count. It never prints a ready-to-run `--approve-retire <id>:0`
/// line: at scaffold time the retire block is not yet in source, so the count is unknown and any
/// printed count would fail `approval_mismatch` when run verbatim.
fn retire_scaffold(catalog_id: &str, labels: &SourceLabels) -> String {
    let target = labels.scaffold_target(catalog_id);
    format!(
        "evolve\n    retire {target}\n    ; Step 1: paste the evolve block above into your source.\n    ; Step 2: run marrow evolve preview <projectdir> to get the exact populated count for {target}.\n    ; Step 3: run marrow evolve apply --maintenance --approve-retire {target}:<count> (--backup <backup-file> | --no-backup) <projectdir>\n"
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

/// The safe scaffold for a populated leaf retype. An in-place transform would overwrite every
/// stored value, silently dropping the data the old type held, so the skeleton is wholly
/// commented guidance the author must complete: add a member of the new type, populate it with
/// a transform from the old member, then retire the old member. Emitting a runnable in-place
/// block here is the data-loss this finding fixes, so every line stays a comment.
fn leaf_retype_skeleton(catalog_id: &str, labels: &SourceLabels) -> String {
    let target = labels.scaffold_target(catalog_id);
    format!(
        "; {target} changed type in place over populated data, which cannot be reinterpreted safely.\n\
         ; Add a member of the new type, populate it with a transform from the old member, then retire the old member:\n\
         ;\n\
         ;     evolve\n\
         ;         transform <newMember>\n\
         ;             return <conversion of old.{leaf}>\n\
         ;         retire {target}\n",
        leaf = leaf_name(&target),
    )
}

/// The bare member name (`pages`) of a resource-qualified scaffold target (`Book.pages`), for
/// spelling the `old.<member>` a transform reads. Falls back to the whole target when it has
/// no trailing member segment, which a leaf retype always does.
fn leaf_name(target: &str) -> &str {
    target.rsplit('.').next().unwrap_or(target)
}

/// The identity-preserving rename scaffold a bare same-shape rename gets in place of a
/// destructive retire: the explicit `evolve rename` that moves the stable id and keeps the
/// stored cells attached. Both sides are spelled in source form (`Book.subtitle`).
fn rename_scaffold(from: &str, to: &str, labels: &SourceLabels) -> String {
    let from = labels.source_path_spelling(from);
    let to = labels.source_path_spelling(to);
    format!("evolve\n    rename {from} -> {to}\n")
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
    let path = labels.scaffold_target(catalog_id);
    let display = labels.display(catalog_id);
    format!(
        "retiring {display} removes {populated} populated record(s); rerun with --maintenance --approve-retire {path}:{populated} --backup <backup-file> (or --no-backup to opt out)"
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
            println!("Nothing to apply; the store already matches your source.");
            render_recovery_text(recovery);
        }
        CheckFormat::Text => {
            println!("Evolution applied. marrow.lock updated - commit it.");
            let records_changed =
                receipt.records_backfilled + receipt.records_transformed + receipt.records_retired;
            println!(
                "{} record(s) changed, {} index(es) rebuilt.",
                records_changed, receipt.indexes_rebuilt
            );
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

/// The concrete `evolve.approval_mismatch` for a witness whose destructive set the approval did not
/// match: it names each retire the evolution requires by human path and exact count, so the
/// operator can copy the corrected `--approve-retire` flag. Falls back to the generic message when
/// the witness carries no destructive obligation (a defensive case admission would not reach).
pub(super) fn approval_mismatch(
    witness: &EvolutionWitness,
    labels: &SourceLabels,
    format: CheckFormat,
) {
    let expected: Vec<String> = witness
        .verdicts
        .iter()
        .filter_map(|obligation| match &obligation.verdict {
            Verdict::DestructiveDecisionRequired { populated } => {
                let path = labels.scaffold_target(obligation.catalog_id.as_str());
                Some(format!("--approve-retire {path}:{populated}"))
            }
            _ => None,
        })
        .collect();
    if expected.is_empty() {
        apply_error(ApplyError::ApprovalMismatch, labels, format);
        return;
    }
    report_simple_error(
        "evolve.approval_mismatch",
        &format!(
            "the --approve-retire counts did not match what this evolution retires; approve exactly: {}",
            expected.join(" ")
        ),
        format,
    );
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
        // ApprovalMismatch is rendered by the caller, which holds the witness and can name the
        // exact approval the destructive set requires; reaching it here means the caller did not,
        // so fall back to a still-actionable message rather than the opaque "preview witness" one.
        ApplyError::ApprovalMismatch => report_simple_error(
            "evolve.approval_mismatch",
            "the --approve-retire counts did not match what this evolution retires; run `marrow evolve preview <projectdir>` to see the exact path and count to approve",
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
