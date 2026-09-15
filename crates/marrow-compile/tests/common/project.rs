//! One project-capture helper: a fixture is captured through the production project
//! owner, never hand-assembled.

#![allow(dead_code)]

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// Capture `files` as a project at the default limits, under the identity ledger `ids`.
pub fn project_with_ids(files: &[(&str, &str)], ids: Option<&[u8]>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, ids, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// Capture `files` as a project with no identity ledger.
pub fn project(files: &[(&str, &str)]) -> ProjectInput {
    project_with_ids(files, None)
}

/// Capture `files` given as raw bytes, so a fixture can hold a file that is not UTF-8.
pub fn project_bytes(files: &[(&str, Vec<u8>)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, bytes)| CapturedFile::new(path.to_string(), bytes.clone()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}
