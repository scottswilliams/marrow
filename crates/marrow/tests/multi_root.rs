//! MR01 step 3b: a project may declare more than one `store` root. Each root is a
//! distinct durable graph node with its own complete ledger identity, its own slot in
//! the image DURABLE table, and its own contribution to the durable-contract identity
//! the verifier independently re-encodes. Two roots over two resources
//! (`^assets` + `^tallies`) compile, seal, and verify together, and each is addressed
//! by its own name in ordinary function bodies.
//!
//! The ephemeral read kernel serves a single executable root at this step, so a
//! two-root image is honestly *parked* at attach — the compile/verify path is plural
//! while runtime execution over more than one root lands in a later step.

use marrow_compile::SourceDiagnostic;
use marrow_verify::VerifiedImage;
use marrow_vm::{Ephemeral, mint_ephemeral};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Asset 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Asset.name 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root assets 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key assets.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id product Tally 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Tally.count 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id root tallies 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key tallies.key 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Asset {
    required name: string
}

resource Tally {
    required count: int
}

store ^assets[id: int]: Asset
store ^tallies[key: string]: Tally

pub fn assetName(id: int): string? {
    return ^assets[id].name
}

pub fn tallyCount(key: string): int? {
    return ^tallies[key].count
}
"#;

fn compile(source: &str, ids: &str) -> Result<marrow_compile::Compiled, Vec<SourceDiagnostic>> {
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
    marrow_compile::compile(&project)
}

fn verify(source: &str, ids: &str) -> VerifiedImage {
    let compiled = compile(source, ids).unwrap_or_else(|diagnostics| {
        panic!("expected a two-root project to compile, got {diagnostics:#?}");
    });
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

/// Two roots over two resources compile and verify into one image carrying both roots
/// in declaration order.
#[test]
fn two_roots_compile_seal_and_verify() {
    let image = verify(SOURCE, IDS);
    assert_eq!(
        image.roots().len(),
        2,
        "both declared roots enter the image's DURABLE table"
    );
    assert_eq!(image.roots()[0].name(), "assets");
    assert_eq!(image.roots()[1].name(), "tallies");
}

/// The single-root ephemeral read kernel honestly parks a two-root image rather than
/// silently serving only the first root.
#[test]
fn a_two_root_image_parks_at_attach() {
    let image = verify(SOURCE, IDS);
    assert!(
        matches!(mint_ephemeral(&image), Ephemeral::Parked),
        "a two-root image is not yet executable by the flat kernel"
    );
}

/// Each root's entry identity `Id(^root)` carries that root's own RootId, so an identity
/// minted over one root cannot address another: it is a precise `check.type` rejection,
/// not a silently accepted confusion of two distinct durable addresses.
#[test]
fn a_cross_root_identity_cannot_address_another_root() {
    let source = r#"resource Asset {
    required name: string
}

resource Tally {
    required count: int
}

store ^assets[id: int]: Asset
store ^tallies[key: string]: Tally

pub fn confuse(id: int): int? {
    const a = Id(^assets, id)
    return ^tallies[a].count
}
"#;
    let diagnostics = compile(source, IDS).expect_err("a cross-root identity is rejected");
    assert!(
        diagnostics.iter().any(|d| d.code == "check.type"),
        "expected a check.type rejection, got {diagnostics:#?}"
    );
}
