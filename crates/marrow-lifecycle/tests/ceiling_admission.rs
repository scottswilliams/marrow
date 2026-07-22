//! The G03 term-3 (D08) effect-ceiling honesty guarantee over the real lifecycle actor.
//!
//! A store records its accepted deployment ceiling at provision — the separately owned
//! standing maximum authority — and the atom-granular admission check enforces it at attach:
//! an image whose verified demand *exceeds* the accepted ceiling (a read-only export broadened
//! to also mutate, its deployment authority not yet updated) is refused before any engine
//! call, naming the exceeding export, effect, and place in source vocabulary; an image whose
//! demand fits *within* the ceiling (even when narrower than a prior image's) is admitted.

use std::path::{Path, PathBuf};

use marrow_lifecycle::{
    AttachOutcome, LifecycleError, ProvisionApproval, ProvisionReport, attach, provision_image,
};
use marrow_verify::{VerifiedImage, verify};

/// The identity ledger shared by every source variant below: the application, the `Counter`
/// product, its two fields, the `counters` root, and its key column. Because the variants
/// share this ledger, the durable contract and the exported interface stay identical across
/// them and only the demand differs.
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

/// The shape shared by every variant.
const SHAPE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter
"#;

/// Variant A: a read-only export. Its demand union is the accepted ceiling a store
/// provisioned under it records: it reads `^counters.value` and nothing more.
fn source_read_only() -> String {
    format!(
        "{SHAPE}\npub fn readValue(n: int): int {{\n    return ^counters[n].value ?? 0\n}}\n"
    )
}

/// Variant B: the same export, same signature, broadened to also mutate — it now stamps the
/// sparse `label` of a present counter. The durable contract and interface are unchanged; only
/// the demand grows, by a write of `^counters.label` (and the presence probe the guard makes).
fn source_broadened() -> String {
    format!(
        "{SHAPE}\npub fn readValue(n: int): int {{\n    var result = 0\n    \
         transaction {{\n        place slot = ^counters[n]\n        \
         if exists(slot) {{\n            slot.label = \"seen\"\n        }}\n        \
         result = ^counters[n].value ?? 0\n    }}\n    return result\n}}\n"
    )
}

fn compile(source: &str) -> (VerifiedImage, Vec<u8>) {
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
    let image = verify(&compiled.image.bytes).expect("verify");
    (image, compiled.image.bytes)
}

fn provision(store: &Path, image: &VerifiedImage) {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    let report = ProvisionReport::new(store, image, &schemas);
    let approval = ProvisionApproval::accept(&report);
    provision_image(store, image, schemas, sites, &approval).expect("provision");
}

fn attach_image(store: &Path, image: &VerifiedImage) -> Result<AttachOutcome, LifecycleError> {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    attach(store, image, schemas, sites)
}

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "marrow-g03-ceiling-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&base).expect("scratch base");
        Self {
            dir: base.join("store"),
        }
    }
    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(parent) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

/// The MUST-WIN: a store provisioned under the read-only image refuses the broadened image —
/// the demand now exceeds the accepted ceiling — naming the export, the new effect, and the
/// place in source vocabulary, before any engine call, leaving the store intact and usable.
#[test]
fn a_broadened_demand_is_refused_naming_the_exceeding_place() {
    let scratch = Scratch::new("refuse");
    let (read_only, _) = compile(&source_read_only());
    let (broadened, _) = compile(&source_broadened());

    // The broadening changes the code and the demand, but not the durable contract or the
    // exported interface — so the refusal is specifically an authority refusal, not a
    // contract-changed one.
    provision(scratch.dir(), &read_only);
    let head_before = std::fs::read(scratch.dir().join("head")).expect("read head");

    let refusal = match attach_image(scratch.dir(), &broadened) {
        Err(LifecycleError::DemandExceedsCeiling(refusal)) => refusal,
        Err(other) => panic!(
            "the broadened image must be refused as demand-exceeds-ceiling, got: {}",
            other.code()
        ),
        Ok(_) => panic!("the broadened image must be refused, not admitted"),
    };

    let rendered = refusal.to_string();
    assert_eq!(refusal.code(), "store.demand_exceeds_ceiling");
    // The refusal names the export, the new effect, and the place in source vocabulary.
    assert!(
        rendered.contains("export `readValue`"),
        "names the export: {rendered}"
    );
    assert!(
        rendered.contains("writes ^counters.label"),
        "names the new write and its place in source vocabulary: {rendered}"
    );
    assert!(
        rendered.contains("Consciously expand"),
        "points the owner at consciously expanding the accepted ceiling: {rendered}"
    );
    assert!(
        refusal
            .exceeding
            .iter()
            .any(|atom| atom.effect == "write"
                && atom.place.as_deref() == Some("^counters.label")),
        "a typed exceeding atom names the write of ^counters.label: {:?}",
        refusal.exceeding,
    );

    // Zero engine calls / store intact: the head is byte-unchanged and the store still opens
    // and serves the prior (read-only) program as already-active.
    let head_after = std::fs::read(scratch.dir().join("head")).expect("read head");
    assert_eq!(head_before, head_after, "the refusal wrote nothing");
    assert!(
        matches!(
            attach_image(scratch.dir(), &read_only),
            Ok(AttachOutcome::AlreadyActive(_))
        ),
        "the prior program remains usable after the refusal",
    );
}

/// The dual: a store provisioned under the broad image *admits* the narrower read-only image —
/// its demand is a strict subset of the accepted ceiling — as a binding-only rebind (same
/// durable contract and interface, different code). This proves the check is a real
/// intersection, not equality: demand ⊊ ceiling is admitted, not refused.
#[test]
fn a_narrowed_demand_within_the_ceiling_is_admitted() {
    let scratch = Scratch::new("narrow");
    let (read_only, _) = compile(&source_read_only());
    let (broadened, _) = compile(&source_broadened());

    provision(scratch.dir(), &broadened);
    match attach_image(scratch.dir(), &read_only) {
        Ok(AttachOutcome::Rebound { .. }) => {}
        Ok(AttachOutcome::AlreadyActive(_)) => {
            panic!("the narrower image differs in code, so it rebinds rather than already-active")
        }
        Err(other) => panic!(
            "a demand within the accepted ceiling must be admitted, got refusal: {}",
            other.code()
        ),
    }
}
