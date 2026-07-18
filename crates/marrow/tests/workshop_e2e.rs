//! Black-box harness and access-demand facts for the Workshop catalog fixture.
//!
//! The fixture source and its committed `marrow.ids` ledger are read from disk and
//! driven through the whole production path — capture -> compile -> verify -> attach
//! -> VM — the same path `marrow test` and a terminal invocation take. One test
//! drives add/read/correct-rollback/final-read over a single *persistent* ephemeral
//! attachment: because the attachment survives across invocations, a committed write
//! is observable by a later read, and a faulting invocation rolls back its own region
//! while leaving a prior commit intact.
//!
//! The demand tests read the catalog's verifier-reconstructed access demand off the
//! sealed image: a mutating export demands writes, a read-only export does not, and
//! the deployment ceiling derived from the program's demand union carries an identity
//! under a domain tag distinct from the demand's, so a demand can never be presented
//! as a wider ceiling. The engine-refusal step of the model — the path kernel
//! resolving `demand ⊆ ceiling ∩ grant` before its first engine call — is owned and
//! exercised in `marrow-kernel` (`durable::store`); it is not reachable through the
//! app image here, because the public run path grants the full store and the
//! image->schema derivation is private, and reaching it would require a test-only
//! production entry point, which the durable model forbids.

use marrow_image::CeilingDescriptor;
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

fn compile_verify() -> VerifiedImage {
    let source = std::fs::read(fixture_dir().join("src/main.mw")).expect("read fixture source");
    let ids = std::fs::read(fixture_dir().join("marrow.ids")).expect("read fixture ledger");
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source,
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(&ids),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .unwrap_or_else(|| panic!("export `{name}` present"))
}

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("the catalog image must be executable, not parked"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn run(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        DurableRun::Ran(Err(fault)) => panic!("{name} faulted: {}", fault.code()),
        DurableRun::Parked => panic!("{name} parked"),
        DurableRun::Failed(code) => panic!("{name} failed: {code}"),
    }
}

fn run_faulting(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> String {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Err(fault)) => fault.code().to_string(),
        DurableRun::Ran(Ok(_)) => panic!("{name} did not fault"),
        DurableRun::Parked => panic!("{name} parked"),
        DurableRun::Failed(code) => panic!("{name} failed: {code}"),
    }
}

fn present_name(name: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(name.into())))))
}

/// One ephemeral attachment serves a sequence of invocations: `add` commits an asset
/// across both roots, a later `assetName` reads it back, a committed `recordMove`
/// advances the moves tally, then an unguarded `recordMove` on an absent asset faults
/// `run.required_missing` and rolls its whole cross-root region back, and a final read
/// shows both roots at their prior committed values with no asset created by the fault.
#[test]
fn add_read_rollback_final_read() {
    let image = compile_verify();
    let mut att = attach(&image);

    assert_eq!(
        run(
            &image,
            &mut att,
            "add",
            vec![
                Value::Int(1),
                Value::Text("T-100".into()),
                Value::Text("Cordless Drill".into()),
                Value::Text("power".into()),
                Value::Instant(0),
            ],
        ),
        Some(Value::Bool(true)),
    );

    assert_eq!(
        run(&image, &mut att, "assetName", vec![Value::Int(1)]),
        present_name("Cordless Drill"),
    );
    // add advanced the ^tallies "catalogued" counter in the same cross-root region.
    assert_eq!(
        run(&image, &mut att, "catalogued", vec![]),
        Some(Value::Int(1)),
    );

    // A committed cross-root move: recordMove writes the ^assets `location` field and
    // advances the ^tallies "moves" tally together.
    run(
        &image,
        &mut att,
        "recordMove",
        vec![Value::Int(1), Value::Text("Bay 3".into())],
    );
    assert_eq!(
        run(&image, &mut att, "location", vec![Value::Int(1)]),
        present_name("Bay 3"),
    );
    assert_eq!(
        run(&image, &mut att, "moveCount", vec![]),
        Some(Value::Int(1))
    );

    // Cross-root rollback: recordMove on an absent asset stages a lone `location` on
    // ^assets and a "moves" increment on ^tallies; the required-missing fault rolls the
    // whole region back across BOTH roots.
    assert_eq!(
        run_faulting(
            &image,
            &mut att,
            "recordMove",
            vec![Value::Int(2), Value::Text("Bay 9".into())],
        ),
        "run.required_missing",
    );

    // Neither root moved: the prior asset and its location stand, no asset 2 exists,
    // and the ^tallies tallies (catalogued and moves) are exactly what committed before
    // the fault — the moves tally was NOT advanced by the rolled-back region.
    assert_eq!(
        run(&image, &mut att, "assetName", vec![Value::Int(1)]),
        present_name("Cordless Drill"),
    );
    assert_eq!(
        run(&image, &mut att, "location", vec![Value::Int(1)]),
        present_name("Bay 3"),
    );
    assert_eq!(
        run(&image, &mut att, "present", vec![Value::Int(2)]),
        Some(Value::Bool(false)),
    );
    assert_eq!(
        run(&image, &mut att, "catalogued", vec![]),
        Some(Value::Int(1)),
    );
    assert_eq!(
        run(&image, &mut att, "moveCount", vec![]),
        Some(Value::Int(1))
    );
}

/// A committed add is durable across the attachment, and a subsequent read invocation
/// on the same attachment observes the whole payload-plus-descendant shape: the asset
/// name and its first `log` entry both read back.
#[test]
fn a_committed_add_is_observable_with_its_log_descendant() {
    let image = compile_verify();
    let mut att = attach(&image);

    run(
        &image,
        &mut att,
        "add",
        vec![
            Value::Int(7),
            Value::Text("T-700".into()),
            Value::Text("Sander".into()),
            Value::Text("power".into()),
            Value::Instant(0),
        ],
    );

    assert_eq!(
        run(&image, &mut att, "assetName", vec![Value::Int(7)]),
        present_name("Sander"),
    );
    assert_eq!(
        run(
            &image,
            &mut att,
            "noteText",
            vec![Value::Int(7), Value::Int(1)],
        ),
        present_name("catalogued"),
    );
}

/// Verified demand distinguishes a mutating export from a read-only one: `add`
/// demands writes and is mutating, `assetName` and `catalogued` observe without
/// mutating, and the program-wide union both reads and writes.
#[test]
fn verified_demand_distinguishes_readers_from_mutators() {
    let image = compile_verify();

    let add = export(&image, "add");
    assert!(add.is_mutating(), "add mutates durable state");
    assert!(add.demand().writes(), "add demands writes");

    let name = export(&image, "assetName");
    assert!(!name.is_mutating(), "assetName is read-only");
    assert!(name.demand().reads(), "assetName demands a read");
    assert!(!name.demand().writes(), "assetName demands no write");

    let count = export(&image, "catalogued");
    assert!(!count.is_mutating(), "catalogued is read-only");
    assert!(count.demand().reads(), "catalogued demands traversal reads");
    assert!(!count.demand().writes(), "catalogued demands no write");

    let union = image.demand_union();
    assert!(
        union.reads() && union.writes(),
        "the catalog both reads and writes"
    );
}

/// The deployment ceiling derived from the program's demand union is a separate
/// authority from the demand itself: it covers the same read/write terms, but its
/// identity is framed under a domain tag distinct from the demand's, so a demand set
/// id can never be re-presented as a wider ceiling over the same atoms.
#[test]
fn the_deployment_ceiling_is_a_separate_authority_from_demand() {
    let image = compile_verify();
    let union = image.demand_union();

    let descriptor = CeilingDescriptor::from_demand_union(union.clone());
    assert_eq!(descriptor.reads(), union.reads());
    assert_eq!(descriptor.writes(), union.writes());

    assert_ne!(
        descriptor.ceiling_id().bytes(),
        union.demand_set_id().bytes(),
        "the ceiling id and the demand id must not collide over the same atoms",
    );
}
