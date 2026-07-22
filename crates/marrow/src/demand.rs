//! Shared durable-demand rendering: one owner for the lines that describe each
//! export's verifier-reconstructed durable access in source spelling.
//!
//! Two renderings project from the same demand facts. [`demand_lines`] is the full
//! per-export `module.item <sentence>` form — every read and write atom, one line per
//! export. `marrow image` prints it on standard error while the owner reviews the
//! authority a deployment ceiling accepts, and `marrow check --demand` prints it on
//! standard output for downstream consumers. [`demand_summary_lines`] is the default
//! `marrow check` view: the same facts grouped by module, exports that share an
//! identical demand listed once, each demand rolled up to its roots with a child-place
//! count, and storeless exports collapsed to one note per module. Neither rendering
//! reclassifies demand — both join the compiler's export directory to the verified
//! image and consume the compiler-owned spelling projection.

use std::collections::BTreeMap;

use marrow_compile::{DemandSummary, DurableNaming, ExportEntry, RootDemand};
use marrow_verify::VerifiedImage;

/// A coherence failure building the demand lines: the compiler's export directory
/// and the verified image disagree, or a demanded node is unnameable. Both are
/// compiler-coherence failures (the same compilation produced both), never a user
/// error, so the caller reports an internal error rather than a diagnostic.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DemandNamingError {
    /// The export directory names an id the verified image does not carry.
    DirectoryImageDisagree,
    /// An admitted export demands a durable place with no source spelling.
    UnnameablePlace,
}

impl DemandNamingError {
    /// The terse internal-error line every consumer prints, so the failure
    /// projection has one owner alongside the lines themselves.
    pub(crate) fn internal_message(&self) -> &'static str {
        match self {
            DemandNamingError::DirectoryImageDisagree => {
                "internal error: export directory and image disagree"
            }
            DemandNamingError::UnnameablePlace => {
                "internal error: an export demands an unnameable durable place"
            }
        }
    }
}

/// Build one `module.item <demand sentence>` line per export, in `module.item`
/// order, so a reader sees the whole program's durable footprint export by export.
pub(crate) fn demand_lines(
    exports: &[ExportEntry],
    naming: &DurableNaming,
    image: &VerifiedImage,
) -> Result<Vec<String>, DemandNamingError> {
    let mut ordered: Vec<&ExportEntry> = exports.iter().collect();
    ordered.sort_by(|a, b| (&a.module, &a.item).cmp(&(&b.module, &b.item)));
    let mut lines = Vec::with_capacity(ordered.len());
    for entry in ordered {
        let export = image
            .export_by_id(entry.id)
            .ok_or(DemandNamingError::DirectoryImageDisagree)?;
        let sentence = naming
            .demand_sentence(export.demand())
            .ok_or(DemandNamingError::UnnameablePlace)?;
        lines.push(format!("{}.{} {sentence}", entry.module, entry.item));
    }
    Ok(lines)
}

/// One export paired with the demand facts a summary renders it from: its full sentence
/// (the identity that groups exports with an identical demand, and the exact `--demand`
/// line) and its root rollup. `storeless` is the empty demand — those exports collapse
/// into one per-module note rather than each printing a line.
struct ExportRecord {
    module: String,
    item: String,
    sentence: String,
    summary: DemandSummary,
    storeless: bool,
}

/// Build the human-shaped default `marrow check` summary. Modules and exports are
/// ordered by spelling and grouping is a pure function of the demand facts, so the
/// output is byte-stable across runs. The full per-export atom sentences stay available
/// through [`demand_lines`] (`marrow check --demand`).
pub(crate) fn demand_summary_lines(
    exports: &[ExportEntry],
    naming: &DurableNaming,
    image: &VerifiedImage,
) -> Result<Vec<String>, DemandNamingError> {
    let records = collect_records(exports, naming, image)?;
    let mut by_module: BTreeMap<&str, Vec<&ExportRecord>> = BTreeMap::new();
    for record in &records {
        by_module
            .entry(record.module.as_str())
            .or_default()
            .push(record);
    }

    let mut lines = vec![format!(
        "{} across {}",
        count(records.len(), "export"),
        count(by_module.len(), "module"),
    )];
    for (module, module_records) in &by_module {
        lines.push(String::new());
        render_module(&mut lines, module, module_records);
    }
    Ok(lines)
}

/// Resolve every export to its demand facts, in `module.item` order. The sentence and
/// the summary come from the same demand through the same compiler-owned join, so the
/// grouping key and the rendered rollup never disagree.
fn collect_records(
    exports: &[ExportEntry],
    naming: &DurableNaming,
    image: &VerifiedImage,
) -> Result<Vec<ExportRecord>, DemandNamingError> {
    let mut ordered: Vec<&ExportEntry> = exports.iter().collect();
    ordered.sort_by(|a, b| (&a.module, &a.item).cmp(&(&b.module, &b.item)));
    let mut records = Vec::with_capacity(ordered.len());
    for entry in ordered {
        let export = image
            .export_by_id(entry.id)
            .ok_or(DemandNamingError::DirectoryImageDisagree)?;
        let demand = export.demand();
        let sentence = naming
            .demand_sentence(demand)
            .ok_or(DemandNamingError::UnnameablePlace)?;
        let summary = naming
            .demand_summary(demand)
            .ok_or(DemandNamingError::UnnameablePlace)?;
        records.push(ExportRecord {
            module: entry.module.clone(),
            item: entry.item.clone(),
            sentence,
            summary,
            storeless: demand.is_empty(),
        });
    }
    Ok(records)
}

/// Render one module: a header, one entry per distinct demand (exports that share a
/// demand listed together), and a single trailing note for any storeless exports. A
/// module whose exports are all storeless folds to its header line alone.
fn render_module(lines: &mut Vec<String>, module: &str, records: &[&ExportRecord]) {
    let storeless: Vec<&str> = records
        .iter()
        .filter(|record| record.storeless)
        .map(|record| record.item.as_str())
        .collect();
    let durable: Vec<&ExportRecord> = records
        .iter()
        .filter(|record| !record.storeless)
        .copied()
        .collect();

    if durable.is_empty() {
        lines.push(format!(
            "{module}: {}, all storeless",
            count(records.len(), "export"),
        ));
        return;
    }

    lines.push(format!("{module}: {}", count(records.len(), "export")));
    for group in group_by_demand(&durable) {
        let items: Vec<&str> = group.iter().map(|record| record.item.as_str()).collect();
        if let [only] = items.as_slice() {
            lines.push(format!("  {only}"));
        } else {
            lines.push(format!(
                "  {} ({}, one shared demand)",
                items.join(", "),
                count(items.len(), "export"),
            ));
        }
        let summary = &group[0].summary;
        if let Some(rendered) = render_roots(&summary.reads) {
            lines.push(format!("    reads {rendered}"));
        }
        if let Some(rendered) = render_roots(&summary.writes) {
            lines.push(format!("    writes {rendered}"));
        }
    }
    if !storeless.is_empty() {
        lines.push(format!("  storeless: {}", storeless.join(", ")));
    }
}

/// Group durable exports that share an identical demand, keyed by their full sentence.
/// Groups appear in first-appearance order over the `module.item`-sorted input, so both
/// the group order and each group's member order are deterministic.
fn group_by_demand<'a>(records: &[&'a ExportRecord]) -> Vec<Vec<&'a ExportRecord>> {
    let mut groups: Vec<(&str, Vec<&'a ExportRecord>)> = Vec::new();
    for &record in records {
        match groups
            .iter_mut()
            .find(|(sentence, _)| *sentence == record.sentence.as_str())
        {
            Some((_, members)) => members.push(record),
            None => groups.push((record.sentence.as_str(), vec![record])),
        }
    }
    groups.into_iter().map(|(_, members)| members).collect()
}

/// Render one coverage's roots as `^a (+2 fields), ^b (+1), ^c`, or `None` for no roots.
/// The child-place unit is spelled on the first root that carries a count and abbreviated
/// after, so a reader learns the unit once without repeating it down a long list.
fn render_roots(roots: &[RootDemand]) -> Option<String> {
    if roots.is_empty() {
        return None;
    }
    let mut unit_spelled = false;
    let mut parts = Vec::with_capacity(roots.len());
    for root in roots {
        if root.field_count == 0 {
            parts.push(root.root.clone());
        } else if unit_spelled {
            parts.push(format!("{} (+{})", root.root, root.field_count));
        } else {
            let unit = if root.field_count == 1 {
                "field"
            } else {
                "fields"
            };
            parts.push(format!("{} (+{} {unit})", root.root, root.field_count));
            unit_spelled = true;
        }
    }
    Some(parts.join(", "))
}

/// `1 export` / `3 exports`: a count with its noun pluralized. The nouns this renderer
/// uses (`export`, `module`) are regular, so a trailing `s` suffices.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
