//! Measured fast-path costs for the persistent terminal path (F02b), recorded against the
//! interactive budget the A02a control freezes (a durable terminal call must complete well
//! within a human-interactive threshold). These are recorded numbers, not asserted
//! durability or latency claims: the test prints each measured median and enforces only a
//! generous non-regression ceiling so it never flakes, while the completion packet carries
//! the recorded table.
//!
//! Components measured, matching the exit gate (lock, spawn, verification, head commit):
//!
//! - **verification** — `marrow_verify::verify` over the Workshop image bytes;
//! - **open** (lock + decode + engine open) — `marrow_lifecycle::open` on a provisioned store;
//! - **head commit** — a binding-only rebind's atomic envelope+head rewrite;
//! - **end-to-end companion call** — spawn + attach + open + run + commit + teardown, the
//!   whole terminal fast path a `marrow run --store` pays.
//!
//! Run with `--nocapture` to see the recorded medians.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use marrow_runner::{Json, attach_and_call};
use marrow_verify::VerifiedImage;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

fn workshop() -> (Vec<u8>, VerifiedImage) {
    let source = std::fs::read(fixture_dir().join("src/main.mw")).expect("source");
    let ids = std::fs::read(fixture_dir().join(".marrow/ids")).expect("ids");
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
    let bytes = marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes;
    let image = marrow_verify::verify(&bytes).expect("verify");
    (bytes, image)
}

fn scratch(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "marrow-fastpath-{tag}-{}-{nonce}/store",
        std::process::id()
    ))
}

fn provision(store: &Path, image: &VerifiedImage) {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat");
    let report = marrow_lifecycle::ProvisionReport::new(store, image, &schemas);
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(store, image, schemas, sites, &approval).expect("provision");
}

fn export_id(image: &VerifiedImage, name: &str) -> [u8; 32] {
    *image
        .exports()
        .iter()
        .find(|e| image.function(e.function()).name() == name)
        .expect("export")
        .id()
        .bytes()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Time `f` `runs` times and return the median.
fn measure(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        f();
        samples.push(start.elapsed());
    }
    median(samples)
}

#[test]
fn fast_path_costs_are_recorded() {
    let (bytes, image) = workshop();
    let (schemas, sites) = marrow_vm::derive_store_schemas(&image).expect("flat");

    // verification
    let verify = measure(21, || {
        marrow_verify::verify(&bytes).expect("verify");
    });

    // open (lock + decode + engine open): provision once, then open/close repeatedly.
    let store = scratch("open");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    provision(&store, &image);
    let open = measure(21, || {
        let opened = marrow_lifecycle::open(&store, schemas.clone(), sites.clone()).expect("open");
        drop(opened);
    });

    // head commit: a binding-only rebind's atomic envelope+head rewrite. Rebind back and
    // forth between two body-only-different images so each attach performs one head commit.
    let edited = {
        let mut src = std::fs::read(fixture_dir().join("src/main.mw")).unwrap();
        src.extend_from_slice(b"\nfn _budgetProbe(): int {\n    return 0\n}\n");
        let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").unwrap();
        let ids = std::fs::read(fixture_dir().join(".marrow/ids")).unwrap();
        let files = vec![marrow_project::CapturedFile::new(
            "src/main.mw".to_string(),
            src,
        )];
        let project = marrow_project::capture(
            &manifest,
            files,
            Some(&ids),
            &marrow_project::CaptureLimits::DEFAULT,
        )
        .unwrap();
        let b = marrow_compile::compile(&project).unwrap().image.bytes;
        marrow_verify::verify(&b).unwrap()
    };
    let mut toggle = false;
    let head_commit = measure(11, || {
        // Alternate the active image so every attach is a real rebind (a head commit).
        let img = if toggle { &image } else { &edited };
        toggle = !toggle;
        match marrow_lifecycle::attach(&store, img, schemas.clone(), sites.clone()).expect("attach")
        {
            marrow_lifecycle::AttachOutcome::Rebound { store, .. } => drop(store),
            marrow_lifecycle::AttachOutcome::AlreadyActive(store) => drop(store),
        }
    });

    // end-to-end companion call (spawn + attach + open + run + commit + teardown).
    let call_store = scratch("call");
    std::fs::create_dir_all(call_store.parent().unwrap()).unwrap();
    provision(&call_store, &image);
    let runner = PathBuf::from(env!("CARGO_BIN_EXE_marrow-runner"));
    let present = export_id(&image, "present");
    let end_to_end = measure(11, || {
        attach_and_call(
            &runner,
            &image,
            &bytes,
            &call_store,
            present,
            vec![Json::Int(1)],
        )
        .expect("call");
    });

    println!("F02b fast-path measured medians (this host):");
    println!("  verification        : {verify:?}");
    println!("  open (lock+decode+engine): {open:?}");
    println!("  head commit (rebind): {head_commit:?}");
    println!("  end-to-end call     : {end_to_end:?}");

    // Non-regression ceilings only (generous — the recorded medians are the evidence). Each
    // in-process step is well under an interactive frame; the end-to-end spawn dominates and
    // stays far under a human-interactive threshold.
    assert!(
        verify < Duration::from_millis(50),
        "verification: {verify:?}"
    );
    assert!(open < Duration::from_millis(200), "open: {open:?}");
    assert!(
        head_commit < Duration::from_secs(1),
        "head commit: {head_commit:?}"
    );
    assert!(
        end_to_end < Duration::from_secs(5),
        "end-to-end: {end_to_end:?}"
    );

    let _ = std::fs::remove_dir_all(store.parent().unwrap());
    let _ = std::fs::remove_dir_all(call_store.parent().unwrap());
}
