//! Shared durable-demand rendering: one owner for the `module.item <sentence>`
//! lines that describe each export's verifier-reconstructed durable access in
//! source spelling. `marrow check` prints them on standard output; `marrow image`
//! prints them on standard error while the owner reviews the authority a deployment
//! ceiling accepts. Neither command reclassifies demand — both join the compiler's
//! export directory to the verified image through this one projection.

use marrow_compile::{DurableNaming, ExportEntry};
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
