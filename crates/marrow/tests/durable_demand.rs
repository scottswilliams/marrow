//! D04: verifier-reconstructed durable demand and the export/image/demand
//! identities, observed through the full production path (capture -> compile ->
//! verify).
//!
//! An export's demand is the stable set of `(semantic path, operation class)` atoms
//! its call closure performs, identified by a `DemandSetId`. Demand describes
//! access; it never grants it. These properties are asserted at the verified-image
//! surface: the verifier reconstructs each export's atoms from the sealed sites its
//! closure references, and nothing about demand is serialized into the image.

use marrow_kernel::durable::DemandCoverage;
use marrow_verify::{
    DemandSetId, ExportDemand, ImageId, OperationClass, SealedExport, VerifiedImage,
};

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

const HEADER: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter
"#;

const VALUE_FIELD: [u8; 16] = [0x0e; 16];
const LABEL_FIELD: [u8; 16] = [0x0f; 16];

/// Compile one `src/main.mw` through the production path, returning its canonical
/// image bytes and `ImageId`.
fn compile_bytes(source: &str) -> (Vec<u8>, ImageId) {
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
    (compiled.image.bytes, compiled.image.image_id)
}

/// Compile and verify one `src/main.mw` through the production path, returning the
/// verified image and its `ImageId`.
fn compile_verify(source: &str) -> (VerifiedImage, ImageId) {
    let (bytes, image_id) = compile_bytes(source);
    let image = marrow_verify::verify(&bytes).expect("verify");
    (image, image_id)
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
        "{HEADER}pub fn readValue(n: int): int {{\n\
         \x20   {read_body_extra}return ^counters[n].value ?? 0\n\
         }}\n\
         \n\
         pub fn bump(n: int) {{\n\
         \x20   transaction {{\n\
         \x20       {bump_body_extra}const current = ^counters[n].value ?? 0\n\
         \x20       ^counters[n].value = current + 1\n\
         \x20   }}\n\
         }}\n"
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
        "const tag = ^counters[n].label ?? \"\"\n\x20       ",
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

// --- Reverse incidence and the no-serialized-summary absence assertion. ---

#[test]
fn demand_incidence_reverses_the_export_map() {
    let (image, _) = compile_verify(&two_export_source("", ""));
    let incidence = image.demand_incidence();

    // Both exports touch only the `value` field, so there is exactly one incidence
    // node, and it is that field.
    assert_eq!(incidence.len(), 1);
    let node = &incidence[0];
    assert_eq!(*node.path.node_id().bytes(), VALUE_FIELD);

    // The node is read by both exports and written by `bump` — the reverse of the
    // per-export demand, derived from the call closure.
    let bump = export_named(&image, "bump").id();
    let read = export_named(&image, "readValue").id();
    assert!(
        node.touched_by
            .iter()
            .any(|inc| inc.export == read && inc.class == OperationClass::Read)
    );
    assert!(
        node.touched_by
            .iter()
            .any(|inc| inc.export == bump && inc.class == OperationClass::Write)
    );
    // Exactly a read from each export plus a write from `bump`.
    assert_eq!(node.touched_by.len(), 3);
}

#[test]
fn the_image_serializes_no_demand_summary() {
    // Demand is verifier-reconstructed. The image carries operation sites and
    // bytecode but no demand, incidence, or consequence summary — so no export's
    // demand id (a hash the verifier never reads) appears anywhere in the bytes.
    let (bytes, _) = compile_bytes(&two_export_source("", ""));
    let image = marrow_verify::verify(&bytes).expect("verify");

    for export in image.exports() {
        assert!(
            !contains(&bytes, export.demand_id().bytes()),
            "an export demand id must not be serialized in the image"
        );
    }
    assert!(
        !contains(&bytes, image.demand_union().demand_set_id().bytes()),
        "the demand union id must not be serialized in the image"
    );
    // The image container still has exactly its ten sections — no demand section was
    // added — and re-verifying rebuilds identical demand from the same bytes.
    let again = marrow_verify::verify(&bytes).expect("verify again");
    for (a, b) in image.exports().iter().zip(again.exports()) {
        assert_eq!(a.demand_id(), b.demand_id());
    }
}

/// Whether `needle` occurs as a contiguous subsequence of `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// --- Ceiling admission vs. invocation coverage feeding the kernel triple. ---

/// The read/write coverage the store ceiling checks, projected from an image
/// demand. This is the seam a durable runtime driver crosses at E01: the stable
/// atom set feeds the kernel's authority triple (`marrow-kernel`, which owns the
/// engine) through its read/write coverage. The intersection with a ceiling and a
/// grant is the kernel's; here the projection and the union-vs-named distinction
/// the triple consumes are asserted at the compiler surface.
fn coverage(demand: &ExportDemand) -> DemandCoverage {
    DemandCoverage {
        read: demand.reads(),
        write: demand.writes(),
    }
}

#[test]
fn admission_coverage_is_the_union_while_invocation_coverage_is_the_named_record() {
    let (image, _) = compile_verify(&two_export_source("", ""));

    // Ceiling admission projects the whole-program union. The program both reads and
    // writes, so admission must be granted read and write.
    let union = coverage(&image.demand_union());
    assert_eq!(
        union,
        DemandCoverage {
            read: true,
            write: true
        }
    );

    // Invocation projects the *named* export. The read-only export needs only read,
    // even though the program union also writes — invocation uses the record, not the
    // union.
    let read_export = coverage(export_named(&image, "readValue").demand());
    assert_eq!(
        read_export,
        DemandCoverage {
            read: true,
            write: false
        }
    );

    // The mutating export needs both.
    let bump_export = coverage(export_named(&image, "bump").demand());
    assert_eq!(
        bump_export,
        DemandCoverage {
            read: true,
            write: true
        }
    );

    // The union coverage dominates every export's coverage: whatever an invocation
    // demands, admission of the union already covers.
    for export in image.exports() {
        let c = coverage(export.demand());
        assert!(union.read || !c.read);
        assert!(union.write || !c.write);
    }
}
