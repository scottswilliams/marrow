//! Entry-identity runtime value.
//!
//! `Id(^root)` is a first-class runtime value wrapping a store root and a key tuple.
//! These tests drive the whole production path — capture -> compile -> verify -> attach
//! -> VM — proving the value's construction (`Id(^books, k)`), equality (`==`/`!=` for
//! identities of the same root), its use as a function parameter and return, and its
//! dereference (`^books[id]`) composing with an ordinary entry read. An entry identity
//! is not durably stored here: it is a runtime/lookup value only.

use marrow_compile::SourceDiagnostic;
use marrow_kernel::durable::EphemeralAttachment;
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

// A single-`int` keyed root `^books[id: int]: Book` with a required `title`.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn shelve(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn make(id: int): Id(^books) {
    return Id(^books, id)
}

pub fn titleVia(id: Id(^books)): string? {
    return ^books[id].title
}

pub fn same(a: Id(^books), b: Id(^books)): bool {
    return a == b
}

pub fn different(a: Id(^books), b: Id(^books)): bool {
    return a != b
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

/// Compile a source that the checker must reject, returning its diagnostics.
fn compile_errors(source: &str) -> Vec<SourceDiagnostic> {
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
    match marrow_compile::compile(&project) {
        Ok(_) => panic!("expected the checker to reject this program"),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(marrow_compile::CompileFailure::Invariant(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

fn has_code(diagnostics: &[SourceDiagnostic], code: &str) -> bool {
    diagnostics.iter().any(|d| d.code == code)
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

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

#[test]
fn construct_dereference_reads_the_named_entry() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);

    run(
        &image,
        &mut store,
        "shelve",
        vec![Value::Int(1), Value::Text("dune".into())],
    );
    run(
        &image,
        &mut store,
        "shelve",
        vec![Value::Int(2), Value::Text("hyperion".into())],
    );

    // `make` returns an `Id(^books)` value; `titleVia` dereferences it.
    let id = run(&image, &mut store, "make", vec![Value::Int(2)])
        .expect("make returns an identity value");
    assert_eq!(
        run(&image, &mut store, "titleVia", vec![id]),
        some_text("hyperion")
    );
}

#[test]
fn dereference_of_absent_entry_is_absent() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);
    let id = run(&image, &mut store, "make", vec![Value::Int(99)]).expect("identity value");
    assert_eq!(
        run(&image, &mut store, "titleVia", vec![id]),
        Some(Value::Optional(None))
    );
}

#[test]
fn identity_equality_is_key_tuple_equality() {
    let image = compile_verify(SOURCE, IDS);
    let mut store = attach(&image);

    let id1 = run(&image, &mut store, "make", vec![Value::Int(7)]).expect("id1");
    let id1b = run(&image, &mut store, "make", vec![Value::Int(7)]).expect("id1b");
    let id2 = run(&image, &mut store, "make", vec![Value::Int(8)]).expect("id2");

    assert_eq!(
        run(&image, &mut store, "same", vec![id1.clone(), id1b]),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        run(&image, &mut store, "same", vec![id1.clone(), id2.clone()]),
        Some(Value::Bool(false)),
    );
    assert_eq!(
        run(&image, &mut store, "different", vec![id1, id2]),
        Some(Value::Bool(true)),
    );
}

// --- Adversarial rejections: the identity value's boundaries. ---

const PREAMBLE: &str =
    "resource Book {\n    required title: string\n}\n\nstore ^books[id: int]: Book\n\n";

fn program(body: &str) -> String {
    format!("{PREAMBLE}{body}")
}

#[test]
fn an_identity_type_over_an_undeclared_root_is_unsupported() {
    let diagnostics = compile_errors(&program("pub fn f(x: Id(^nope)): int {\n    return 0\n}\n"));
    assert!(has_code(&diagnostics, "check.unsupported"));
}

#[test]
fn an_identity_constructor_over_an_undeclared_root_is_rejected() {
    let diagnostics = compile_errors(&program(
        "pub fn f(): Id(^books) {\n    return Id(^nope, 1)\n}\n",
    ));
    assert!(has_code(&diagnostics, "check.type"));
}

#[test]
fn an_identity_constructor_with_the_wrong_key_arity_is_rejected() {
    // The root has one key column; supplying none is a key-arity error.
    let diagnostics = compile_errors(&program(
        "pub fn f(): Id(^books) {\n    return Id(^books)\n}\n",
    ));
    assert!(has_code(&diagnostics, "check.type"));
}

#[test]
fn an_identity_constructor_with_the_wrong_key_type_is_rejected() {
    // The single key column is `int`; a string operand does not coerce.
    let diagnostics = compile_errors(&program(
        "pub fn f(): Id(^books) {\n    return Id(^books, \"x\")\n}\n",
    ));
    assert!(has_code(&diagnostics, "check.type"));
}

#[test]
fn comparing_an_identity_with_a_scalar_is_rejected() {
    let diagnostics = compile_errors(&program(
        "pub fn f(a: Id(^books)): bool {\n    return a == 5\n}\n",
    ));
    assert!(has_code(&diagnostics, "check.type"));
}

#[test]
fn an_identity_is_not_an_orderable_collection_key() {
    // Entry identities are not admitted in a key position, so a map keyed by one is
    // an unsupported type.
    let diagnostics = compile_errors(&program(
        "pub fn f(): int {\n    const m: Map<Id(^books), int> = Map()\n    return length(m)\n}\n",
    ));
    assert!(has_code(&diagnostics, "check.unsupported"));
}

#[test]
fn a_declaration_named_id_is_reserved() {
    let diagnostics = compile_errors(&program("pub fn Id(): int {\n    return 0\n}\n"));
    assert!(!diagnostics.is_empty());
}
