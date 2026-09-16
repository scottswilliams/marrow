//! One project-capture helper: a fixture is captured through the production project
//! owner, never hand-assembled.

#![allow(dead_code)]

use marrow_project::{
    CaptureLimits, CapturedDependency, CapturedFile, Manifest, ProjectInput, capture_origins,
};

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
        "edition = \"2026\"\n\n[dependencies]\n{alias} = {{ path = \"../{alias}\" }}\n"
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
