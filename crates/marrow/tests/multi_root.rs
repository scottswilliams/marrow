//! MR01: a project may declare more than one `store` root, and the kernel executes
//! over all of them. Each root is a distinct durable graph node with its own complete
//! ledger identity, its own slot in the image DURABLE table, its own kernel
//! `StoreSchema`, and its own name-keyed physical cell family. Two roots over two
//! resources (`^assets` + `^tallies`) compile, seal, verify, and *execute* together:
//! each is addressed by its own name in ordinary function bodies, a per-root read or
//! write dispatches to that root's schema, and a single `transaction` region may write
//! both roots and commit — or roll back — as one atomic unit.
//!
//! Entry identity stays root-local: `Id(^assets, id)` addresses `^assets` and only
//! `^assets`. Using it against `^assets` executes; naming it against `^tallies` is a
//! precise `check.type` rejection, never a silent confusion of two distinct durable
//! addresses.

use marrow_compile::SourceDiagnostic;
use marrow_verify::{SealedExport, TestKind, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_driver_test, run_export};

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

/// Two roots over two resources. Reads and writes address each root by its own name;
/// `putBoth` writes both roots in one transaction, and `putBothOrFail` proves an
/// atomic cross-root rollback. `viaId` proves a root-local entry identity round-trips
/// against its own root.
const SOURCE: &str = r#"resource Asset {
    required name: string
}

resource Tally {
    required count: int
}

store ^assets[id: int]: Asset
store ^tallies[key: string]: Tally

pub fn putAsset(id: int, n: string) {
    transaction {
        ^assets[id] = Asset(name: n)
    }
}

pub fn putTally(key: string, c: int) {
    transaction {
        ^tallies[key] = Tally(count: c)
    }
}

pub fn assetName(id: int): string? {
    return ^assets[id].name
}

pub fn tallyCount(key: string): int? {
    return ^tallies[key].count
}

pub fn viaId(id: int): string? {
    const a = Id(^assets, id)
    return ^assets[a].name
}

pub fn putBoth(id: int, key: string, n: string, c: int) {
    transaction {
        ^assets[id] = Asset(name: n)
        ^tallies[key] = Tally(count: c)
    }
}

pub fn putBothOrFail(id: int, key: string, n: string, c: int, boom: bool) {
    transaction {
        ^assets[id] = Asset(name: n)
        ^tallies[key] = Tally(count: c)
        if boom {
            unreachable("the invariant broke after staging both roots")
        }
    }
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
    match marrow_compile::compile(&project) {
        Ok(compiled) => Ok(compiled),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => {
            Err(diagnostics.into_vec())
        }
        Err(marrow_compile::CompileFailure::Invariant(_) | marrow_compile::CompileFailure::ResourceLimit(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

fn verify(source: &str, ids: &str) -> VerifiedImage {
    let compiled = compile(source, ids).unwrap_or_else(|diagnostics| {
        panic!("expected a two-root project to compile, got {diagnostics:#?}");
    });
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

/// A minted two-root attachment; the kernel must execute over it, not park it.
fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a two-root image must be executable, not parked"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

/// Run `name(args)` against `attachment`, returning its VM value (a fault panics).
fn run(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        other => panic!("{name} did not run cleanly: {:?}", DebugRun(&other)),
    }
}

/// Run `name(args)` expecting a source-mapped runtime fault, returning its code.
fn run_faulting(
    image: &VerifiedImage,
    attachment: &mut marrow_kernel::durable::EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> String {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Err(fault)) => fault.code().to_string(),
        other => panic!("{name} did not fault: {:?}", DebugRun(&other)),
    }
}

struct DebugRun<'a>(&'a DurableRun);
impl std::fmt::Debug for DebugRun<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            DurableRun::Ran(Ok(_)) => write!(f, "Ran(Ok(value))"),
            DurableRun::Ran(Err(fault)) => write!(f, "Ran(Err({}))", fault.code()),
            DurableRun::Parked => write!(f, "Parked"),
            DurableRun::Failed(code) => write!(f, "Failed({code})"),
        }
    }
}

fn some_text(text: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(text.into())))))
}

fn some_int(value: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(value)))))
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

/// The kernel executes over a two-root image: a write to each root dispatches to that
/// root's own schema and name-keyed cell family, and a later read of each root returns
/// exactly that root's committed value. The two roots do not alias — a value written to
/// `^assets` is not observable through `^tallies` and vice versa.
#[test]
fn each_root_reads_and_writes_independently() {
    let image = verify(SOURCE, IDS);
    let mut attachment = attach(&image);

    // Both roots start empty.
    assert_eq!(
        run(&image, &mut attachment, "assetName", vec![Value::Int(1)]),
        Some(Value::Optional(None))
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tallyCount",
            vec![Value::Text("x".into())]
        ),
        Some(Value::Optional(None))
    );

    // Write each root under its own key type; read each back from its own schema.
    run(
        &image,
        &mut attachment,
        "putAsset",
        vec![Value::Int(1), Value::Text("widget".into())],
    );
    run(
        &image,
        &mut attachment,
        "putTally",
        vec![Value::Text("x".into()), Value::Int(5)],
    );
    assert_eq!(
        run(&image, &mut attachment, "assetName", vec![Value::Int(1)]),
        some_text("widget"),
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tallyCount",
            vec![Value::Text("x".into())]
        ),
        some_int(5),
    );
}

/// A root-local entry identity round-trips against its own root: `Id(^assets, id)` used
/// against `^assets` reads the committed entry, exercising the declaration-ordered
/// `RootId` at runtime rather than only at check time.
#[test]
fn a_root_local_identity_reads_its_own_root() {
    let image = verify(SOURCE, IDS);
    let mut attachment = attach(&image);
    run(
        &image,
        &mut attachment,
        "putAsset",
        vec![Value::Int(7), Value::Text("gear".into())],
    );
    assert_eq!(
        run(&image, &mut attachment, "viaId", vec![Value::Int(7)]),
        some_text("gear"),
        "an identity minted over ^assets reads ^assets",
    );
}

/// One `transaction` region writes both roots and commits them as one atomic unit: a
/// later read observes both writes. The witness rides one engine transaction spanning
/// the disjoint name-keyed cell families of both roots.
#[test]
fn a_cross_root_transaction_commits_both_roots() {
    let image = verify(SOURCE, IDS);
    let mut attachment = attach(&image);

    run(
        &image,
        &mut attachment,
        "putBoth",
        vec![
            Value::Int(2),
            Value::Text("k".into()),
            Value::Text("bolt".into()),
            Value::Int(9),
        ],
    );
    assert_eq!(
        run(&image, &mut attachment, "assetName", vec![Value::Int(2)]),
        some_text("bolt"),
        "the cross-root transaction committed the ^assets write",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tallyCount",
            vec![Value::Text("k".into())]
        ),
        some_int(9),
        "the cross-root transaction committed the ^tallies write",
    );
}

/// A `transaction` region that stages writes to both roots and then faults before
/// committing rolls *both* roots back together: neither the `^assets` write nor the
/// `^tallies` write survives. Atomicity is cross-root, not per-root.
#[test]
fn a_cross_root_transaction_rolls_both_roots_back() {
    let image = verify(SOURCE, IDS);
    let mut attachment = attach(&image);

    // Seed distinct committed values on both roots.
    run(
        &image,
        &mut attachment,
        "putBoth",
        vec![
            Value::Int(3),
            Value::Text("r".into()),
            Value::Text("old".into()),
            Value::Int(1),
        ],
    );

    // Stage a replacement of both roots, then fault before the commit.
    let code = run_faulting(
        &image,
        &mut attachment,
        "putBothOrFail",
        vec![
            Value::Int(3),
            Value::Text("r".into()),
            Value::Text("new".into()),
            Value::Int(2),
            Value::Bool(true),
        ],
    );
    assert_eq!(code, "run.unreachable");

    // Both roots retain their pre-transaction committed values: the rollback was atomic
    // across both roots, not partial.
    assert_eq!(
        run(&image, &mut attachment, "assetName", vec![Value::Int(3)]),
        some_text("old"),
        "the faulted transaction rolled the ^assets write back",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "tallyCount",
            vec![Value::Text("r".into())]
        ),
        some_int(1),
        "the faulted transaction rolled the ^tallies write back",
    );
}

/// Two `store` declarations that share a root name are a precise `check.type` rejection:
/// each root's name keys a distinct physical cell family, so a repeated name has no
/// unambiguous address. The verifier rejects the same collision independently for a
/// forged image (see marrow-verify's multi_root_hostile).
#[test]
fn two_stores_sharing_a_root_name_are_rejected_at_check() {
    let source = r#"resource Asset {
    required name: string
}

store ^assets[id: int]: Asset
store ^assets[key: int]: Asset
"#;
    let diagnostics = compile(source, IDS).expect_err("a duplicate root name is rejected");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "check.type" && d.message.contains("more than once")),
        "expected a duplicate-root-name check.type rejection, got {diagnostics:#?}"
    );
}

/// A driver `test` drives both roots through export calls — each call its own invocation
/// boundary — exactly as a terminal drives an application: a mutating export writes both
/// roots and commits, and later reading exports observe each root's committed value. This
/// is the two-root shape of the invocation-boundary isolation law.
#[test]
fn a_two_root_driver_test_drives_both_roots_through_exports() {
    let source = format!(
        "{SOURCE}\ntest \"cross-root driver round trip\" {{\n    \
             putBoth(5, \"d\", \"beam\", 4)\n    \
             assert (assetName(5) ?? \"none\") == \"beam\"\n    \
             assert (tallyCount(\"d\") ?? 0) == 4\n}}\n"
    );
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile_with_tests(&project).unwrap_or_else(|diagnostics| {
        panic!("a two-root driver test must compile: {diagnostics:#?}")
    });
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");

    let entry = image
        .test_entries()
        .iter()
        .find(|entry| entry.name() == "cross-root driver round trip")
        .expect("the driver test entry is sealed");
    assert!(
        matches!(entry.kind(), TestKind::Driver),
        "a test that only calls exports is a driver test",
    );
    match run_driver_test(&image, entry) {
        DurableRun::Ran(Ok(_)) => {}
        other => panic!(
            "the two-root driver test must run cleanly: {:?}",
            DebugRun(&other)
        ),
    }
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
