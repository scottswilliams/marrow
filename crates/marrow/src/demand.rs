//! Shared durable-demand rendering: one owner for the lines that describe each
//! export's verifier-reconstructed durable access in source spelling.
//!
//! Two renderings project from the same demand facts. [`demand_report`] and
//! [`write_demand_report`] are the `marrow check` report: exports grouped by module,
//! adjacent exports with one demand set identity listed once, each group naming every
//! durable place it reads and writes, and storeless exports collapsed to one note per
//! module. [`demand_lines`] is the per-export `module.item <sentence>` form `marrow
//! image` prints on standard error while the owner reviews the authority a deployment
//! ceiling accepts. Neither rendering reclassifies demand — both join the compiler's
//! export directory to the verified image and consume the compiler-owned spelling
//! projection.

use std::io::{self, Write};

use marrow_compile::{DemandPlaces, DurableNaming, ExportEntry};
use marrow_image::DemandSetId;
use marrow_verify::VerifiedImage;

/// The widest line the report writes before a place list continues on the next line.
/// A line holds at most this many bytes, or one place alone when that place is longer.
pub(crate) const ROW_WIDTH: usize = 96;

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
        let (_, demand) = demand_of(entry, image)?;
        let sentence = naming
            .demand_sentence(demand)
            .ok_or(DemandNamingError::UnnameablePlace)?;
        lines.push(format!("{}.{} {sentence}", entry.module, entry.item));
    }
    Ok(lines)
}

/// One module's report: its exports in `module.item` order, adjacent exports with one
/// demand set identity grouped, and the storeless exports named once.
pub(crate) struct ModuleReport<'a> {
    module: &'a str,
    exports: usize,
    groups: Vec<Group<'a>>,
    storeless: Vec<&'a str>,
}

/// Adjacent exports of one module whose demand sets are identical, spelled once.
struct Group<'a> {
    id: DemandSetId,
    items: Vec<&'a str>,
    places: DemandPlaces,
}

/// Resolve the `marrow check` report. Exports are grouped by their typed demand set
/// identity before any place is spelled, so a demand is spelled once per group;
/// grouping joins only adjacent exports, so the `module.item` order of the report is
/// the order of the directory. Byte-stable across runs: modules, exports, and places
/// are ordered by spelling and grouping is a pure function of the demand facts.
pub(crate) fn demand_report<'a>(
    exports: &'a [ExportEntry],
    naming: &DurableNaming,
    image: &VerifiedImage,
) -> Result<Vec<ModuleReport<'a>>, DemandNamingError> {
    let mut modules: Vec<ModuleReport<'a>> = Vec::new();
    for entry in ordered(exports) {
        let (id, demand) = demand_of(entry, image)?;
        if modules
            .last()
            .is_none_or(|last| last.module != entry.module)
        {
            modules.push(ModuleReport {
                module: &entry.module,
                exports: 0,
                groups: Vec::new(),
                storeless: Vec::new(),
            });
        }
        let module = modules.last_mut().expect("the module was just pushed");
        module.exports += 1;
        if demand.is_empty() {
            module.storeless.push(&entry.item);
        } else if let Some(group) = module.groups.last_mut().filter(|group| group.id == id) {
            group.items.push(&entry.item);
        } else {
            let places = naming
                .demand_places(demand)
                .ok_or(DemandNamingError::UnnameablePlace)?;
            module.groups.push(Group {
                id,
                items: vec![&entry.item],
                places,
            });
        }
    }
    Ok(modules)
}

/// Write the report: a header, then per module a header, one entry per group naming
/// every place it reads and writes, and a trailing note for the storeless exports. A
/// module whose exports are all storeless folds to its header line alone.
///
/// The output is linear in the demand facts: a place list line holds at most
/// [`ROW_WIDTH`] bytes (or one place alone when that place is longer) and continues on
/// an indented line, so the whole report is at most three times the bytes of its place
/// spellings, each spelled once per group, plus 128 bytes for each export, module, and
/// header line.
pub(crate) fn write_demand_report(
    writer: &mut impl Write,
    report: &[ModuleReport<'_>],
) -> io::Result<()> {
    let exports: usize = report.iter().map(|module| module.exports).sum();
    writeln!(
        writer,
        "{} across {}",
        count(exports, "export"),
        count(report.len(), "module")
    )?;
    for module in report {
        writeln!(writer)?;
        write_module(writer, module)?;
    }
    writer.flush()
}

fn write_module(writer: &mut impl Write, module: &ModuleReport<'_>) -> io::Result<()> {
    if module.groups.is_empty() {
        return writeln!(
            writer,
            "{}: {}, all storeless",
            module.module,
            count(module.exports, "export")
        );
    }
    writeln!(
        writer,
        "{}: {}",
        module.module,
        count(module.exports, "export")
    )?;
    for group in &module.groups {
        if let [only] = group.items.as_slice() {
            writeln!(writer, "  {only}")?;
        } else {
            writeln!(
                writer,
                "  {} ({}, one shared demand)",
                group.items.join(", "),
                count(group.items.len(), "export"),
            )?;
        }
        write_places(writer, "reads", &group.places.reads)?;
        write_places(writer, "writes", &group.places.writes)?;
    }
    if !module.storeless.is_empty() {
        writeln!(writer, "  storeless: {}", module.storeless.join(", "))?;
    }
    Ok(())
}

/// One `reads`/`writes` list, continued on an indented line whenever the next place,
/// with its separator and the comma a continued line ends in, would carry the line
/// past [`ROW_WIDTH`]. An empty list writes nothing.
fn write_places(writer: &mut impl Write, label: &str, places: &[String]) -> io::Result<()> {
    let Some((first, rest)) = places.split_first() else {
        return Ok(());
    };
    let mut line = format!("    {label} {first}");
    for place in rest {
        if line.len() + ", ".len() + place.len() + ",".len() > ROW_WIDTH {
            writeln!(writer, "{line},")?;
            line = format!("      {place}");
        } else {
            line.push_str(", ");
            line.push_str(place);
        }
    }
    writeln!(writer, "{line}")
}

/// The export directory in `module.item` order.
fn ordered(exports: &[ExportEntry]) -> Vec<&ExportEntry> {
    let mut ordered: Vec<&ExportEntry> = exports.iter().collect();
    ordered.sort_by(|a, b| (&a.module, &a.item).cmp(&(&b.module, &b.item)));
    ordered
}

/// One export's demand set identity and verifier-reconstructed demand, joined through
/// the compiler's directory.
fn demand_of<'a>(
    entry: &ExportEntry,
    image: &'a VerifiedImage,
) -> Result<(DemandSetId, marrow_image::DemandView<'a>), DemandNamingError> {
    let export = image
        .export_by_id(entry.id)
        .ok_or(DemandNamingError::DirectoryImageDisagree)?;
    let demand = image
        .function(export.function())
        .expect("verified export function")
        .demand();
    Ok((export.demand_id(), demand))
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
