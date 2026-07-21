//! The counter-as-allocator idiom, executed end to end from source (DX04).
//!
//! Marrow has no `nextId` built-in: an application that needs a fresh, monotonically
//! increasing key mints one from a durable counter it owns. This pins the documented
//! journey ([Counter allocation](../../../docs/language/idioms.md)) green: a single
//! `name`-keyed `^idseq` counter root, a `place seq` bind, the `seq.value ?? 0`
//! read-with-default, the write-back, and the payload create all share the export's one
//! `transaction`, so the increment and the create commit as a unit. The test drives the
//! whole production path — capture -> compile -> verify -> attach -> VM — against one
//! persistent ephemeral attachment, so a later read observes an earlier allocation.

use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// application, the reusable `Counter { value }` product and its `idseq` counter root,
// then the `Book { title }` payload product and its `books` root.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Counter.value 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id root idseq 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key idseq.name 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// The documented counter journey. `createBook` allocates the next key from
/// `^idseq["book"]` and creates the `^books` entry in one transaction; the observers
/// read a title back by key and read the counter's current value.
const SOURCE: &str = r#"resource Counter {
    required value: int
}

store ^idseq[name: string]: Counter

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn createBook(title: string): int {
    transaction {
        place seq = ^idseq["book"]
        const next = (seq.value ?? 0) + 1
        seq.value = next
        ^books[next] = Book(title: title)
        return next
    }
}

pub fn titleOf(id: int): string? {
    return ^books[id].title
}

pub fn seqValue(): int? {
    return ^idseq["book"].value
}
"#;

fn compile_verify(source: &str) -> VerifiedImage {
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
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

fn export<'a>(image: &'a VerifiedImage, name: &str) -> &'a SealedExport {
    image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .expect("export present")
}

fn attach(image: &VerifiedImage) -> marrow_kernel::durable::EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("a flat counter and payload root must be executable"),
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

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

/// Allocating two ids in sequence yields 1 then 2, each create lands under its minted
/// key, and the shared counter ends at the last value allocated. The counter is minted
/// by the first allocation, so `seq.value ?? 0` supplies the first-use value with no
/// separate initialization.
#[test]
fn the_counter_allocates_monotonic_keys_and_binds_each_create() {
    let image = compile_verify(SOURCE);
    let mut attachment = attach(&image);

    assert_eq!(
        run(
            &image,
            &mut attachment,
            "createBook",
            vec![Value::Text("alpha".into())]
        ),
        Some(Value::Int(1)),
        "the first allocation reads the absent counter as 0 and mints 1",
    );
    assert_eq!(
        run(
            &image,
            &mut attachment,
            "createBook",
            vec![Value::Text("beta".into())]
        ),
        Some(Value::Int(2)),
        "the second allocation advances the persisted counter to 2",
    );

    assert_eq!(
        run(&image, &mut attachment, "titleOf", vec![Value::Int(1)]),
        some_text("alpha"),
        "the first create landed under key 1",
    );
    assert_eq!(
        run(&image, &mut attachment, "titleOf", vec![Value::Int(2)]),
        some_text("beta"),
        "the second create landed under key 2",
    );

    assert_eq!(
        run(&image, &mut attachment, "seqValue", vec![]),
        some_int(2),
        "the counter persists its last allocated value",
    );
}
