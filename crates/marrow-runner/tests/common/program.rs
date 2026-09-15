//! The one production build these suites attach to: capture a single `src/main.mw` under an
//! id ledger, compile it, and verify the bytes. Suites differ only in where the source and
//! ledger come from — an inline fixture string or the Workshop conformance directory — and
//! in which of the resulting artifacts they go on to use.

// Each suite uses a different subset of this surface.
#![allow(dead_code)]

use std::path::PathBuf;

use marrow_verify::VerifiedImage;

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
pub fn workshop_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

/// Capture `source` as the project's sole `src/main.mw` under the `ids` ledger, compile it,
/// and verify the result.
pub fn build(source: Vec<u8>, ids: &[u8]) -> Program {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source,
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let bytes = marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes;
    let image = marrow_verify::verify(&bytes).expect("verify");
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
