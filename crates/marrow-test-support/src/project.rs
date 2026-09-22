//! One project-capture path: a fixture is captured through the production project owner,
//! never hand-assembled.

use crate::MANIFEST;
use marrow_project::{
    CaptureLimits, CapturedDependency, CapturedFile, Manifest, ProjectInput, capture,
    capture_origins,
};

/// Capture `files` as a project at the default limits, under the identity ledger `ids`.
pub fn captured(files: &[(&str, impl AsRef<[u8]>)], ids: Option<&[u8]>) -> ProjectInput {
    let manifest = Manifest::parse(MANIFEST).expect("valid manifest");
    let files = files
        .iter()
        .map(|(path, bytes)| CapturedFile::new(path.to_string(), bytes.as_ref().to_vec()))
        .collect();
    capture(&manifest, files, ids, &CaptureLimits::DEFAULT).expect("capture project")
}

/// Capture `files` under the identity ledger `ids`.
pub fn project_with_ids(files: &[(&str, &str)], ids: Option<&[u8]>) -> ProjectInput {
    captured(files, ids)
}

/// Capture `files` with no identity ledger.
pub fn project(files: &[(&str, &str)]) -> ProjectInput {
    captured(files, None)
}

/// Capture `files` given as raw bytes, so a fixture can hold a file that is not UTF-8.
pub fn project_bytes(files: &[(&str, Vec<u8>)]) -> ProjectInput {
    captured(files, None)
}

/// Capture a root project and one dependency tree under `alias` as a single
/// origin-tagged input, the way the physical adapter does for a `[dependencies]`
/// manifest. `root`/`dependency` paths are each relative to their own tree's root.
pub fn project_with_dependency(
    alias: &str,
    root: &[(&str, &str)],
    dependency: &[(&str, &str)],
) -> ProjectInput {
    dependency_project(alias, root, None, dependency, None)
}

/// As [`project_with_dependency`], with each tree's committed `.marrow/ids` bytes.
pub fn dependency_project(
    alias: &str,
    root: &[(&str, &str)],
    root_ids: Option<&[u8]>,
    dependency: &[(&str, &str)],
    dependency_ids: Option<&[u8]>,
) -> ProjectInput {
    let manifest = Manifest::parse(&format!(
        "{MANIFEST}\n[dependencies]\n{alias} = {{ path = \"../{alias}\" }}\n"
    ))
    .expect("valid manifest");
    let alias = manifest.dependencies()[0].alias().clone();
    let mut captured: Vec<CapturedFile> = root
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    captured.extend(dependency.iter().map(|(path, source)| {
        CapturedFile::in_dependency(alias.clone(), path.to_string(), source.as_bytes().to_vec())
    }));
    capture_origins(
        &manifest,
        captured,
        root_ids,
        &[CapturedDependency::new(&alias, dependency_ids)],
        &CaptureLimits::DEFAULT,
    )
    .expect("capture project")
}
