//! Managed-index reads at the source level (ID01 session 2).
//!
//! A nonunique index is scanned with a bounded `for` head that binds the source-root
//! `Id(^root)`; a unique index is looked up with a bracket access that yields the
//! optional `Id(^root)`. Both drive the whole production path — capture -> compile ->
//! verify -> attach -> VM — and compose with the entry-identity dereference: the bound
//! identity reads its entry through `^root[id]`.

use marrow_kernel::durable::EphemeralAttachment;
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// `^books[id: int]: Book` with a nonunique `byShelf[shelf, id]` and a unique
// `byIsbn[isbn]`. The index anchors live at `books.<index name>`.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.shelf 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id field Book.isbn 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index books.byShelf 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id index books.byIsbn 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Book {
    required title: string
    required shelf: string
    required isbn: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

pub fn shelve(id: int, title: string, shelf: string, isbn: string) {
    transaction {
        ^books[id] = Book(title: title, shelf: shelf, isbn: isbn)
    }
}

pub fn countOnShelf(shelf: string): int {
    var count = 0
    for bookId in ^books.byShelf[shelf] at most 100 {
        if const b = ^books[bookId] {
            count += 1
        }
    } on more {
        count = -1
    }
    return count
}

pub fn countOnShelfBounded(shelf: string): int {
    var count = 0
    for bookId in ^books.byShelf[shelf] at most 2 {
        if const b = ^books[bookId] {
            count += 1
        }
    } on more {
        return -1
    }
    return count
}

pub fn isbnPresent(isbn: string): bool {
    if const found = ^books.byIsbn[isbn] {
        return true
    }
    return false
}

pub fn titleByIsbn(isbn: string): string? {
    if const found = ^books.byIsbn[isbn] {
        return ^books[found].title
    }
    return absent
}
"#;

fn compile_verify(source: &str, ids: &str) -> VerifiedImage {
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

fn run(
    image: &VerifiedImage,
    attachment: &mut EphemeralAttachment,
    name: &str,
    args: Vec<Value>,
) -> Option<Value> {
    match run_export(image, attachment, export(image, name), args) {
        DurableRun::Ran(Ok(value)) => value,
        other => panic!("{name} did not run cleanly: {:?}", DebugRun(&other)),
    }
}

fn attach(image: &VerifiedImage) -> EphemeralAttachment {
    match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("the books root must be executable"),
        Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
    }
}

fn s(v: &str) -> Value {
    Value::Text(v.into())
}

fn seed(image: &VerifiedImage, store: &mut EphemeralAttachment) {
    // Two books on shelf "A", one on "B"; distinct isbns.
    run(image, store, "shelve", vec![Value::Int(1), s("dune"), s("A"), s("i1")]);
    run(image, store, "shelve", vec![Value::Int(2), s("hyperion"), s("A"), s("i2")]);
    run(image, store, "shelve", vec![Value::Int(3), s("neuromancer"), s("B"), s("i3")]);
}

#[test]
fn a_nonunique_scan_binds_the_identity_and_dereferences_it() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    seed(&image, &mut store);

    assert_eq!(run(&image, &mut store, "countOnShelf", vec![s("A")]), Some(Value::Int(2)));
    assert_eq!(run(&image, &mut store, "countOnShelf", vec![s("B")]), Some(Value::Int(1)));
    assert_eq!(run(&image, &mut store, "countOnShelf", vec![s("Z")]), Some(Value::Int(0)));
}

#[test]
fn a_bounded_scan_is_exact_and_fan_out_independent() {
    // The precise O(distinct + 1) seek cost is owned by the kernel `index_read` fixtures;
    // these assert the source-observable consequences the compiler's lowering delivers: a
    // bounded scan freezes exactly `at most N` identities and reports `on more`, and one
    // shelf's scan is isolated from another shelf's fan-out.
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    // Shelf "A" holds three books, shelf "B" one — `at most 2` on "A" hits the bound and
    // runs `on more`; on "B" it does not.
    run(&image, &mut store, "shelve", vec![Value::Int(1), s("a"), s("A"), s("i1")]);
    run(&image, &mut store, "shelve", vec![Value::Int(2), s("b"), s("A"), s("i2")]);
    run(&image, &mut store, "shelve", vec![Value::Int(3), s("c"), s("A"), s("i3")]);
    run(&image, &mut store, "shelve", vec![Value::Int(4), s("d"), s("B"), s("i4")]);

    assert_eq!(run(&image, &mut store, "countOnShelfBounded", vec![s("A")]), Some(Value::Int(-1)));
    assert_eq!(run(&image, &mut store, "countOnShelfBounded", vec![s("B")]), Some(Value::Int(1)));
    // Isolation: the full (unbounded) scan of each shelf sees only that shelf's rows,
    // independent of the other shelf's fan-out.
    assert_eq!(run(&image, &mut store, "countOnShelf", vec![s("A")]), Some(Value::Int(3)));
    assert_eq!(run(&image, &mut store, "countOnShelf", vec![s("B")]), Some(Value::Int(1)));
}

#[test]
fn a_unique_lookup_is_present_or_absent() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    seed(&image, &mut store);

    assert_eq!(run(&image, &mut store, "isbnPresent", vec![s("i2")]), Some(Value::Bool(true)));
    assert_eq!(run(&image, &mut store, "isbnPresent", vec![s("missing")]), Some(Value::Bool(false)));
}

#[test]
fn a_unique_lookup_dereferences_the_found_identity() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    seed(&image, &mut store);

    assert_eq!(
        run(&image, &mut store, "titleByIsbn", vec![s("i2")]),
        Some(Value::Optional(Some(Box::new(s("hyperion"))))),
    );
    assert_eq!(
        run(&image, &mut store, "titleByIsbn", vec![s("missing")]),
        Some(Value::Optional(None)),
    );
}
