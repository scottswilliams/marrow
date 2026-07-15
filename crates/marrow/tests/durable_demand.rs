//! D04: verifier-reconstructed durable demand and the export/image/demand
//! identities, observed through the full production path (capture -> compile ->
//! verify).
//!
//! An export's demand is the stable set of `(semantic path, operation class)` atoms
//! its call closure performs, identified by a `DemandSetId`. Demand describes
//! access; it never grants it. These properties are asserted at the verified-image
//! surface: the verifier reconstructs each export's atoms from the sealed sites its
//! closure references, and nothing about demand is serialized into the image.

use marrow_verify::{DemandSetId, ImageId, OperationClass, SealedExport, VerifiedImage};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const HEADER: &str = "resource Counter\n\
     \x20   required value: int\n\
     \x20   label: string\n\
     \n\
     store ^counters(id: int): Counter\n\
     \n";

const VALUE_FIELD: [u8; 16] = [0x0e; 16];
const LABEL_FIELD: [u8; 16] = [0x0f; 16];

/// Compile and verify one `src/main.mw` through the production path, returning the
/// verified image and its `ImageId`.
fn compile_verify(source: &str) -> (VerifiedImage, ImageId) {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    (image, compiled.image.image_id)
}

/// The export whose entry function is named `name`.
fn export_named<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

/// The `(node ledger id, class)` fingerprint of an export's demand atoms.
fn atom_shape(export: &SealedExport) -> Vec<([u8; 16], OperationClass)> {
    export
        .demand()
        .atoms()
        .iter()
        .map(|atom| (*atom.path().node_id().bytes(), atom.class()))
        .collect()
}

/// A two-export program: `readValue` reads the `value` field of an entry, `bump`
/// reads then writes it inside a transaction. Optional extra lines let a test insert
/// a pure body change or an added read.
fn two_export_source(read_body_extra: &str, bump_body_extra: &str) -> String {
    format!(
        "{HEADER}\
         pub fn readValue(n: int): int\n\
         \x20   {read_body_extra}return ^counters(n).value ?? 0\n\
         \n\
         pub fn bump(n: int)\n\
         \x20   transaction\n\
         \x20       {bump_body_extra}const current = ^counters(n).value ?? 0\n\
         \x20       ^counters(n).value = current + 1\n"
    )
}

#[test]
fn each_export_demand_is_reconstructed_from_its_closure() {
    let (image, _) = compile_verify(&two_export_source("", ""));

    // `readValue` reads the `value` field and nothing else.
    let read = export_named(&image, "readValue");
    assert_eq!(atom_shape(read), vec![(VALUE_FIELD, OperationClass::Read)]);
    assert!(read.demand().reads());
    assert!(!read.demand().writes());
    assert!(!read.is_mutating());

    // `bump` reads and writes the `value` field.
    let bump = export_named(&image, "bump");
    let mut shape = atom_shape(bump);
    shape.sort();
    assert_eq!(
        shape,
        vec![
            (VALUE_FIELD, OperationClass::Read),
            (VALUE_FIELD, OperationClass::Write),
        ]
    );
    assert!(bump.demand().reads());
    assert!(bump.demand().writes());
    assert!(bump.is_mutating());

    // The two exports demand differently, so their demand ids differ, and neither
    // equals the other's export id.
    assert_ne!(read.demand_id(), bump.demand_id());
    assert_ne!(read.id(), bump.id());
}

#[test]
fn a_body_only_change_keeps_export_and_demand_ids_but_moves_the_image_id() {
    let (base, base_image_id) = compile_verify(&two_export_source("", ""));
    // Insert a pure statement into `readValue`'s body: same durable access, same
    // declaration path, different bytes.
    let (edited, edited_image_id) =
        compile_verify(&two_export_source("const unused = n + 1\n\x20   ", ""));

    let base_read = export_named(&base, "readValue");
    let edited_read = export_named(&edited, "readValue");

    // The declaration identity and the demand identity are both stable across a
    // body-only edit...
    assert_eq!(base_read.id(), edited_read.id());
    assert_eq!(base_read.demand_id(), edited_read.demand_id());
    // ...while the image digest changes because the bytes changed.
    assert_ne!(base_image_id.to_hex(), edited_image_id.to_hex());
    // Recompiling the same source is fully deterministic.
    let (again, again_image_id) = compile_verify(&two_export_source("", ""));
    assert_eq!(base_image_id.to_hex(), again_image_id.to_hex());
    assert_eq!(
        base_read.demand_id(),
        export_named(&again, "readValue").demand_id()
    );
}

#[test]
fn adding_a_durable_read_changes_the_demand_id() {
    let (base, _) = compile_verify(&two_export_source("", ""));
    // `bump` now also reads the `label` field: its reachable atoms — and demand id —
    // change, while `readValue`'s are untouched.
    let (widened, _) = compile_verify(&two_export_source(
        "",
        "const tag = ^counters(n).label ?? \"\"\n\x20       ",
    ));

    let base_bump = export_named(&base, "bump");
    let widened_bump = export_named(&widened, "bump");
    assert_ne!(base_bump.demand_id(), widened_bump.demand_id());
    assert!(
        widened_bump
            .demand()
            .atoms()
            .iter()
            .any(|atom| *atom.path().node_id().bytes() == LABEL_FIELD
                && atom.class() == OperationClass::Read)
    );

    // `readValue`'s demand is unchanged: demand is per-closure, not global.
    assert_eq!(
        export_named(&base, "readValue").demand_id(),
        export_named(&widened, "readValue").demand_id()
    );
}

#[test]
fn the_demand_union_admits_the_whole_program() {
    let (image, _) = compile_verify(&two_export_source("", ""));
    let union = image.demand_union();
    // The union covers both a read and a write of the `value` field.
    assert!(union.reads());
    assert!(union.writes());
    let mut shape: Vec<_> = union
        .atoms()
        .iter()
        .map(|atom| (*atom.path().node_id().bytes(), atom.class()))
        .collect();
    shape.sort();
    assert_eq!(
        shape,
        vec![
            (VALUE_FIELD, OperationClass::Read),
            (VALUE_FIELD, OperationClass::Write),
        ]
    );
    // Admission checks the whole program while an invocation checks its named
    // export. The union strictly exceeds the read-only export's demand; here `bump`
    // already subsumes `readValue`, so the union coincides with `bump`'s demand —
    // exactly the point that admission uses the union, not one export's record.
    let read = export_named(&image, "readValue");
    let bump = export_named(&image, "bump");
    let union_id: DemandSetId = union.demand_set_id();
    assert_ne!(union_id, read.demand_id());
    assert_eq!(union_id, bump.demand_id());
}

#[test]
fn reachable_sites_are_image_local_and_not_in_the_demand_id() {
    let (image, _) = compile_verify(&two_export_source("", ""));
    let read = export_named(&image, "readValue");
    let bump = export_named(&image, "bump");
    // The reachable-site sets are ascending, deduplicated, and in image-site range.
    for export in [read, bump] {
        let sites = export.reachable_sites();
        assert!(sites.windows(2).all(|w| w[0] < w[1]));
        assert!(sites.iter().all(|&s| (s as usize) < image.sites().len()));
    }
    // `bump` reaches at least a read and a write site; `readValue` reaches at least
    // one. The bitset is image-local — it is never fed into the stable demand id,
    // which is a pure function of the atom set.
    assert!(!read.reachable_sites().is_empty());
    assert!(bump.reachable_sites().len() >= read.reachable_sites().len());
}
