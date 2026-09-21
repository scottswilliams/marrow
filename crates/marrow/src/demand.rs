//! Shared durable-demand rendering: one owner for the lines that describe each
//! export's verifier-reconstructed durable access in source spelling.
//!
//! Two renderings project from the same demand facts. [`demand_report_lines`] is the
//! `marrow check` report: exports grouped by module, each naming every durable place it
//! reads and writes, exports that share an identical demand listed once, and storeless
//! exports collapsed to one note per module. [`demand_lines`] is the per-export
//! `module.item <sentence>` form `marrow image` prints on standard error while the owner
//! reviews the authority a deployment ceiling accepts. Neither rendering reclassifies
//! demand — both join the compiler's export directory to the verified image and consume
//! the compiler-owned spelling projection.

use std::collections::BTreeMap;

use marrow_compile::{DemandPlaces, DurableNaming, ExportEntry};
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
    let mut lines = Vec::with_capacity(exports.len());
    for entry in ordered(exports) {
        let demand = demand_of(entry, image)?;
        let sentence = naming
            .demand_sentence(demand)
            .ok_or(DemandNamingError::UnnameablePlace)?;
        lines.push(format!("{}.{} {sentence}", entry.module, entry.item));
    }
    Ok(lines)
}

/// One export paired with the exact places it reads and writes. `places` is also the
/// identity that groups exports with an identical demand; an empty demand is storeless,
/// and those exports collapse into one per-module note rather than each printing a line.
struct ExportRecord {
    module: String,
    item: String,
    places: DemandPlaces,
}

impl ExportRecord {
    fn storeless(&self) -> bool {
        self.places.reads.is_empty() && self.places.writes.is_empty()
    }
}

/// Build the `marrow check` report. Modules and exports are ordered by spelling and
/// grouping is a pure function of the demand facts, so the output is byte-stable across
/// runs.
pub(crate) fn demand_report_lines(
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

/// The export directory in `module.item` order.
fn ordered(exports: &[ExportEntry]) -> Vec<&ExportEntry> {
    let mut ordered: Vec<&ExportEntry> = exports.iter().collect();
    ordered.sort_by(|a, b| (&a.module, &a.item).cmp(&(&b.module, &b.item)));
    ordered
}

/// One export's verifier-reconstructed demand, joined through the compiler's directory.
fn demand_of<'a>(
    entry: &ExportEntry,
    image: &'a VerifiedImage,
) -> Result<marrow_image::DemandView<'a>, DemandNamingError> {
    let export = image
        .export_by_id(entry.id)
        .ok_or(DemandNamingError::DirectoryImageDisagree)?;
    Ok(image
        .function(export.function())
        .expect("verified export function")
        .demand())
}

/// Resolve every export to its exact places, in `module.item` order.
fn collect_records(
    exports: &[ExportEntry],
    naming: &DurableNaming,
    image: &VerifiedImage,
) -> Result<Vec<ExportRecord>, DemandNamingError> {
    let mut records = Vec::with_capacity(exports.len());
    for entry in ordered(exports) {
        let places = naming
            .demand_places(demand_of(entry, image)?)
            .ok_or(DemandNamingError::UnnameablePlace)?;
        records.push(ExportRecord {
            module: entry.module.clone(),
            item: entry.item.clone(),
            places,
        });
    }
    Ok(records)
}

/// Render one module: a header, one entry per distinct demand (exports that share a
/// demand listed together) naming every place read and written, and a single trailing
/// note for any storeless exports. A module whose exports are all storeless folds to its
/// header line alone.
fn render_module(lines: &mut Vec<String>, module: &str, records: &[&ExportRecord]) {
    let storeless: Vec<&str> = records
        .iter()
        .filter(|record| record.storeless())
        .map(|record| record.item.as_str())
        .collect();
    let durable: Vec<&ExportRecord> = records
        .iter()
        .filter(|record| !record.storeless())
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
        let places = &group[0].places;
        if !places.reads.is_empty() {
            lines.push(format!("    reads {}", places.reads.join(", ")));
        }
        if !places.writes.is_empty() {
            lines.push(format!("    writes {}", places.writes.join(", ")));
        }
    }
    if !storeless.is_empty() {
        lines.push(format!("  storeless: {}", storeless.join(", ")));
    }
}

/// Group durable exports that share an identical demand. Groups appear in
/// first-appearance order over the `module.item`-sorted input, so both the group order
/// and each group's member order are deterministic.
fn group_by_demand<'a>(records: &[&'a ExportRecord]) -> Vec<Vec<&'a ExportRecord>> {
    let mut groups: Vec<Vec<&'a ExportRecord>> = Vec::new();
    for &record in records {
        match groups
            .iter_mut()
            .find(|members| members[0].places == record.places)
        {
            Some(members) => members.push(record),
            None => groups.push(vec![record]),
        }
    }
    groups
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
