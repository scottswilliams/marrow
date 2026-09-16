//! Editor-facing facts read off the revisioned `AnalysisSnapshot`: hover and definition,
//! completions, active call, the document-symbol outline, and whole-document format.
//!
//! Every query distinguishes a genuine absence from a syntax- or dependency-unavailable
//! position and from an invalid coordinate, and none of them rebuilds a language fact from
//! source text.

use std::sync::Arc;

use marrow_compile::{AnalysisSnapshot, InputRevision, analyze};
use marrow_project::FileIdentity;

#[path = "common/project.rs"]
mod project_capture;
use project_capture::{project, project_bytes};

#[path = "editor_facts/active_call.rs"]
mod active_call;
#[path = "editor_facts/completions.rs"]
mod completions;
#[path = "editor_facts/document_symbols.rs"]
mod document_symbols;
#[path = "editor_facts/format_query.rs"]
mod format_query;
#[path = "editor_facts/hover_facts.rs"]
mod hover_facts;

/// Analyze a project and unwrap its snapshot (the opaque `AnalysisFailure` is not
/// `Debug`, so a `let`-else keeps the failure boundary opaque).
fn snap(files: &[(&str, &str)]) -> Arc<AnalysisSnapshot> {
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot for {files:?}");
    };
    snapshot
}

/// The snapshot of a lone `src/app.mw` holding `source`, for the position queries whose
/// fixtures are one file each.
fn snap_app(source: &str) -> Arc<AnalysisSnapshot> {
    snap(&[("src/app.mw", source)])
}

fn identity(path: &str) -> marrow_compile::ProjectFile {
    marrow_compile::ProjectFile::root(FileIdentity::validate(path).expect("canonical identity").0)
}

/// The byte offset of the first occurrence of `needle` in `source`, advanced by `extra`
/// bytes.
fn at(source: &str, needle: &str, extra: usize) -> usize {
    source.find(needle).expect("needle present in source") + extra
}
