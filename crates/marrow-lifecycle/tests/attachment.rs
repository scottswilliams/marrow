//! The image/store attachment over real compiled images: a store executes only the image
//! lifecycle admitted for it, every field site reads its own value through real execution on
//! both hosts, selection is checked against the owned image before any store is minted, and a
//! parked or storeless image keeps its identity and its storeless exports.

use std::path::{Path, PathBuf};

use marrow_lifecycle::{
    AttachOutcome, EphemeralOutcome, LifecycleError, MemoryAttachment, NativeAttachment,
    PreparedImage, ProvisionApproval, ProvisionReport, attach, fresh_test, mint_ephemeral, prepare,
    provision_image,
};
use marrow_verify::{ExportId, VerifiedImage, verify};
use marrow_vm::{DurableRun, Value, run_export, run_test};

const COUNTER_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// Image A. Image B differs only in `readValue`'s fallback (`?? 0` → `?? 1`): same durable
/// contract, interface, and ceiling, different code.
const COUNTER_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn setValue(n: int, v: int) {
    transaction {
        ^counters[n].value = v
    }
}

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

const PAIR_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id product Pair 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Pair.a 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id field Pair.b 1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f\n\
     id root pairs 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key pairs.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// Two same-scalar fields with distinct stored values: each field-leaf site must read its own
/// field, which only real execution over the derived site table proves.
const PAIR_SOURCE: &str = r#"resource Pair {
    required a: int
    required b: int
}

store ^pairs[id: int]: Pair

pub fn seed(id: int) {
    transaction {
        ^pairs[id] = Pair(a: 1, b: 2)
    }
}

pub fn readA(id: int): int {
    return ^pairs[id].a ?? 0
}

pub fn readB(id: int): int {
    return ^pairs[id].b ?? 0
}
"#;

const STORELESS_IDS: &str =
    "marrow ids v0\nmachine-written by marrow; do not edit\nhigh-water 0\nend\n";

const STORELESS_SOURCE: &str = r#"pub fn two(): int {
    return 2
}

test "two is two" {
    assert two() == 2
}
"#;

fn capture(source: &str, ids: &str) -> marrow_project::ProjectInput {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture")
}

fn compile(source: &str, ids: &str) -> VerifiedImage {
    let compiled = marrow_compile::compile(&capture(source, ids)).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}

fn compile_with_tests(source: &str, ids: &str) -> VerifiedImage {
    let compiled = marrow_compile::compile_with_tests(&capture(source, ids)).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}

/// A unique scratch store directory, removed on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "marrow-lifecycle-attachment-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&base).expect("create scratch base");
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

fn provision(store: &Path, image: &VerifiedImage) {
    let prepared = prepare(image.clone());
    let report = ProvisionReport::new(store, &prepared).expect("flat-executable");
    let approval = ProvisionApproval::accept(&report);
    provision_image(store, &prepared, &approval).expect("provision");
}

fn export(image: &VerifiedImage, name: &str) -> ExportId {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .unwrap_or_else(|| panic!("export {name}"))
        .id()
}

fn memory(image: &VerifiedImage) -> MemoryAttachment {
    match mint_ephemeral(prepare(image.clone())) {
        EphemeralOutcome::Ready(attachment) => attachment,
        EphemeralOutcome::Parked(_) => panic!("the fixture is flat-executable"),
        EphemeralOutcome::Failed { cause, .. } => panic!("mint failed: {cause}"),
    }
}

fn native(store: &Path, image: &VerifiedImage) -> (NativeAttachment, bool) {
    match attach(store, prepare(image.clone())).expect("attach") {
        AttachOutcome::AlreadyActive(attachment) => (attachment, false),
        AttachOutcome::Rebound { attachment, .. } => (attachment, true),
    }
}

/// Run the attachment's own export `name`, returning its value.
fn call<H: marrow_kernel::durable::SessionHost>(
    attachment: &mut marrow_lifecycle::Attachment<H>,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    let export = export(attachment.image(), name);
    match run_export(attachment, export, args).expect("the export is in the attached image") {
        DurableRun::Ran(Ok(value)) => value,
        DurableRun::Ran(Err(fault)) => panic!("{name} faulted: {}", fault.code()),
        DurableRun::Parked => panic!("{name} parked"),
        DurableRun::Failed(code) => panic!("{name} failed: {code}"),
    }
}

/// A store executes the image it is bound to and no other. Attaching A serves A's code and A
/// is the head; the only way B's code runs against the store is the explicit attach of B,
/// which rebinds the head to B with a receipt and leaves the committed data in place. There
/// is no route that pairs B with a store whose head is A: `run_export` takes only the
/// attachment, whose image is the one `attach` admitted (see the compile-fail cases on
/// `marrow_lifecycle::Attachment` and `marrow_vm::run_export`).
#[test]
fn a_store_executes_only_the_image_it_is_bound_to() {
    let image_a = compile(COUNTER_SOURCE, COUNTER_IDS);
    let image_b = compile(&COUNTER_SOURCE.replace("?? 0", "?? 1"), COUNTER_IDS);
    assert_ne!(image_a.image_id().0, image_b.image_id().0);
    let scratch = Scratch::new("pair");
    provision(scratch.dir(), &image_a);

    // A/A: the head binds A and A's code runs (its default is 0).
    {
        let (mut attachment, rebound) = native(scratch.dir(), &image_a);
        assert!(!rebound);
        assert_eq!(attachment.head().binding.image_id, image_a.image_id().0);
        assert_eq!(attachment.image().image_id().0, image_a.image_id().0);
        assert_eq!(
            call(&mut attachment, "readValue", vec![Value::Int(1)]),
            Some(Value::Int(0))
        );
        call(
            &mut attachment,
            "setValue",
            vec![Value::Int(2), Value::Int(5)],
        );
    }

    // Explicit B attach: the head rebinds to B, B's code runs (its default is 1), and the
    // data committed under A is intact.
    let (mut attachment, rebound) = native(scratch.dir(), &image_b);
    assert!(rebound, "a body-only edit rebinds");
    assert_eq!(attachment.head().binding.image_id, image_b.image_id().0);
    assert_eq!(attachment.image().image_id().0, image_b.image_id().0);
    assert_eq!(
        call(&mut attachment, "readValue", vec![Value::Int(1)]),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(&mut attachment, "readValue", vec![Value::Int(2)]),
        Some(Value::Int(5))
    );
}

/// Every field-leaf site reads its own field through real execution over the derived site
/// table, on the in-memory host and on the native store, with the empty-site control first.
#[test]
fn every_field_site_reads_its_own_value_on_both_hosts() {
    let image = compile(PAIR_SOURCE, PAIR_IDS);

    let mut memory = memory(&image);
    assert_eq!(
        call(&mut memory, "readA", vec![Value::Int(1)]),
        Some(Value::Int(0)),
        "an absent entry reads the fallback",
    );
    call(&mut memory, "seed", vec![Value::Int(1)]);
    assert_eq!(
        call(&mut memory, "readA", vec![Value::Int(1)]),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(&mut memory, "readB", vec![Value::Int(1)]),
        Some(Value::Int(2))
    );

    let scratch = Scratch::new("sites");
    provision(scratch.dir(), &image);
    let (mut native, _) = native(scratch.dir(), &image);
    assert_eq!(
        call(&mut native, "readB", vec![Value::Int(1)]),
        Some(Value::Int(0))
    );
    call(&mut native, "seed", vec![Value::Int(1)]);
    assert_eq!(
        call(&mut native, "readA", vec![Value::Int(1)]),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(&mut native, "readB", vec![Value::Int(1)]),
        Some(Value::Int(2))
    );
}

/// An export identity the attached image does not carry is `None` from `run_export`: the
/// lookup is in the attachment's own image, before any session opens.
#[test]
fn an_absent_export_selects_nothing() {
    let image = compile(PAIR_SOURCE, PAIR_IDS);
    let mut attachment = memory(&image);
    assert!(
        run_export(
            &mut attachment,
            ExportId::of_local("", "missing"),
            Vec::new()
        )
        .is_none()
    );
    // The attachment is untouched and still serves its own exports.
    assert_eq!(
        call(&mut attachment, "readA", vec![Value::Int(1)]),
        Some(Value::Int(0))
    );
}

/// A storeless image has no store: its mint parks while keeping the image, whose identity
/// and storeless exports stay reachable; a native attach refuses it before the store is
/// touched; its storeless test runs with no store at all; and an absent test index selects
/// nothing.
#[test]
fn a_storeless_image_keeps_its_identity_and_mints_no_store() {
    let image = compile_with_tests(STORELESS_SOURCE, STORELESS_IDS);
    let id = image.image_id().0;

    let outcome = mint_ephemeral(prepare(image.clone()));
    let EphemeralOutcome::Parked(owned) = &outcome else {
        panic!("a storeless image has no store to mint");
    };
    assert_eq!(
        owned.image_id().0,
        id,
        "the parked outcome owns the same image"
    );
    let two = outcome
        .image()
        .export_by_id(export(&image, "two"))
        .expect("two");
    assert_eq!(
        marrow_vm::run(outcome.image(), two.function(), Vec::new()),
        Ok(Some(Value::Int(2)))
    );

    let scratch = Scratch::new("storeless");
    assert!(matches!(
        attach(scratch.dir(), prepare(image.clone())),
        Err(LifecycleError::NotExecutable)
    ));
    assert!(
        !scratch.dir().exists(),
        "a refused storeless attach touches no store"
    );

    let prepared: PreparedImage = prepare(image);
    assert!(prepared.projection().is_none());
    let test = fresh_test(&prepared, 0).expect("the image carries one test");
    assert_eq!(test.entry().name(), "two is two");
    assert!(matches!(run_test(test), DurableRun::Ran(Ok(_))));
    assert!(
        fresh_test(&prepared, 1).is_none(),
        "an absent test index selects nothing"
    );
}

/// The whole matrix of fresh tests over a durable image: a storeless entry and a durable
/// entry each run from their own selection, and an absent index selects nothing before any
/// store is minted.
#[test]
fn fresh_tests_run_from_their_own_image() {
    let source = format!(
        "{PAIR_SOURCE}\ntest \"pure\" {{\n    assert 1 + 1 == 2\n}}\n\ntest \"seeded\" {{\n    \
         seed(1)\n    assert readA(1) == 1\n    assert readB(1) == 2\n}}\n"
    );
    let image = compile_with_tests(&source, PAIR_IDS);
    let prepared = prepare(image);
    assert_eq!(prepared.image().test_entries().len(), 2);
    for index in 0..2 {
        let test = fresh_test(&prepared, index).expect("both entries are in the image");
        assert!(
            matches!(run_test(test), DurableRun::Ran(Ok(_))),
            "test {index} runs from its own selection",
        );
    }
    assert!(fresh_test(&prepared, 2).is_none());
}
