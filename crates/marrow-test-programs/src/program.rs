//! The one production build every suite attaches to: capture source under an id ledger,
//! compile it, and verify the bytes. Suites differ only in where the source and ledger
//! come from — an inline fixture string or the Workshop conformance directory — and in
//! which of the resulting artifacts they go on to use.

use std::path::PathBuf;

use marrow_verify::{VerifiedImage, verify};

use marrow_test_support::project::captured;

/// A verified program together with the image bytes a terminal ships to a runner. The bytes
/// are the exact input `image` was verified from, so the two always name the same program.
pub struct Program {
    pub image: VerifiedImage,
    pub bytes: Vec<u8>,
}

impl Program {
    pub fn export_id(&self, name: &str) -> [u8; 32] {
        export_id(&self.image, name)
    }
}

/// The declaration id of the export whose function is named `name`.
pub fn export_id(image: &VerifiedImage, name: &str) -> [u8; 32] {
    *image
        .exports()
        .iter()
        .find(|export| {
            image
                .function(export.function())
                .expect("verified function")
                .body()
                .name()
                == name
        })
        .unwrap_or_else(|| panic!("export `{name}` present"))
        .id()
        .bytes()
}

/// The Workshop conformance fixture directory: the shared durable program the attach and
/// death-boundary suites run against.
fn workshop_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

/// Capture and compile a project of several modules, returning the image bytes. Several
/// modules, so an export's *module* — half of its declaration path identity — can be varied
/// as well as its item name.
pub fn compile_files(sources: &[(&str, &str)], ids: &str) -> Vec<u8> {
    marrow_compile::compile(&captured(sources, Some(ids.as_bytes())))
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
    let project = captured(&[("src/main.mw", source)], Some(ids.as_bytes()));
    let compiled = marrow_compile::compile_with_tests(&project).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}

/// Capture `source` as the project's sole `src/main.mw` under the `ids` ledger, compile it,
/// and verify the result.
pub fn build(source: Vec<u8>, ids: &[u8]) -> Program {
    let bytes = marrow_compile::compile(&captured(&[("src/main.mw", source)], Some(ids)))
        .expect("compile")
        .image
        .bytes;
    let image = verify(&bytes).expect("verify");
    Program { image, bytes }
}

/// Build the Workshop fixture with `extra` appended to its source, over its existing durable
/// schema and id ledger.
pub fn workshop_with(extra: &str) -> Program {
    let dir = workshop_dir();
    let mut source = std::fs::read(dir.join("src/main.mw")).expect("read fixture source");
    source.extend_from_slice(extra.as_bytes());
    let ids = std::fs::read(dir.join(".marrow/ids")).expect("read fixture ledger");
    build(source, &ids)
}

/// Build the Workshop fixture unchanged.
pub fn workshop() -> Program {
    workshop_with("")
}
