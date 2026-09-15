//! The one compile helper the lifecycle integration tests share.

#![allow(dead_code)]

use marrow_verify::{VerifiedImage, verify};

/// Capture and compile a project of several modules, returning the image bytes. Several
/// modules, so an export's *module* — half of its declaration path identity — can be varied
/// as well as its item name.
pub fn compile_files(sources: &[(&str, &str)], ids: &str) -> Vec<u8> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = sources
        .iter()
        .map(|(path, text)| {
            marrow_project::CapturedFile::new(path.to_string(), text.as_bytes().to_vec())
        })
        .collect();
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes
}

/// The image bytes of a single-module project.
pub fn compile_bytes(source: &str, ids: &str) -> Vec<u8> {
    compile_files(&[("src/main.mw", source)], ids)
}

/// A verified single-module image.
pub fn compile(source: &str, ids: &str) -> VerifiedImage {
    verify(&compile_bytes(source, ids)).expect("verify")
}

/// A verified single-module image carrying its test entries.
pub fn compile_with_tests(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile_with_tests(&project).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}
