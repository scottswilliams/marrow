//! The `require <condition> else <value>` guard statement.
//!
//! `require C else E` is pure lowering sugar for `if not C { return err(E) }`: the condition
//! is a `bool`, the bare failure value types against the enclosing function's `Result` error
//! type, and the failure exit is an implicit return committing only the function's active
//! region. The pure-sugar claim is enforced here as byte identity of the two spellings'
//! images in every section but the source-position table, which necessarily differs.
//!
//! Transaction-owner and helper exits are pinned in
//! `durable_model/transaction_ownership.rs`.

use marrow_codes::Code;
use std::collections::BTreeMap;

use marrow_compile::{CompileFailure, SourceDiagnostic, TypeMismatch, compile};

use super::project;

/// Compile a pure (storeless) module, returning the check diagnostics.
fn diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&project(source)) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("source-triggered failure must remain diagnostics, got {other:?}"),
    }
}

/// Compile a pure module that must be clean, returning the encoded image bytes.
fn image_bytes(source: &str) -> Vec<u8> {
    match compile(&project(source)) {
        Ok(compiled) => compiled.image.bytes,
        Err(failure) => panic!("fixture must compile clean, got {failure:?}"),
    }
}

#[test]
fn a_require_in_a_result_function_compiles() {
    let source = "module main\n\nfn isPositive(n: int): bool {\n    return n > 0\n}\n\npub fn check(n: int): Result<int, string> {\n    require isPositive(n) else \"not positive\"\n    return ok(n)\n}\n";
    assert!(diagnostics(source).is_empty(), "{:#?}", diagnostics(source));
}

/// The implicit failure exit needs an error channel to return through.
#[test]
fn a_require_outside_a_result_function_is_rejected() {
    let source = "module main\n\npub fn check(n: int): int {\n    require n > 0 else \"not positive\"\n    return n\n}\n";
    let diagnostics = diagnostics(source);
    let diagnostic = diagnostics.first().expect("a rejection");
    assert_eq!(diagnostic.code(), Code::CheckType);
    assert!(
        diagnostic.message().contains("require") && diagnostic.message().contains("Result"),
        "names `require` and the Result requirement: {}",
        diagnostic.message()
    );
}

/// The same refusal for a unit function: no return value at all.
#[test]
fn a_require_in_a_unit_function_is_rejected() {
    let source =
        "module main\n\npub fn check(n: int) {\n    require n > 0 else \"not positive\"\n}\n";
    let diagnostics = diagnostics(source);
    let diagnostic = diagnostics.first().expect("a rejection");
    assert_eq!(diagnostic.code(), Code::CheckType);
    assert!(
        diagnostic.message().contains("Result"),
        "{}",
        diagnostic.message()
    );
}

/// The bare failure value must be the function's exact error type, as for `try`; there is
/// no implicit conversion.
#[test]
fn a_mistyped_else_value_is_rejected() {
    let source = "module main\n\npub fn check(n: int): Result<int, string> {\n    require n > 0 else 42\n    return ok(n)\n}\n";
    let diagnostics = diagnostics(source);
    let diagnostic = diagnostics.first().expect("a rejection");
    assert_eq!(diagnostic.code(), Code::CheckType);
}

/// The condition must be `bool`, exactly as an `if` condition must.
#[test]
fn a_non_bool_condition_is_rejected() {
    let source = "module main\n\npub fn check(n: int): Result<int, string> {\n    require n else \"not positive\"\n    return ok(n)\n}\n";
    let diagnostics = diagnostics(source);
    let diagnostic = diagnostics.first().expect("a rejection");
    assert_eq!(diagnostic.code(), Code::CheckType);
    let Some(TypeMismatch::Condition { found }) = diagnostic.type_mismatch() else {
        panic!("a non-bool condition is a condition mismatch: {diagnostic:?}");
    };
    assert_eq!(found.as_str(), "int");
}

/// `try` propagates an existing failure and `require` originates one; both exits type
/// against the same error channel.
#[test]
fn require_and_try_interleave_in_one_function() {
    let source = "module main\n\nfn isPositive(n: int): bool {\n    return n > 0\n}\n\nfn half(n: int): Result<int, string> {\n    require isPositive(n) else \"not positive\"\n    return ok(n / 2)\n}\n\npub fn quarter(n: int): Result<int, string> {\n    const h = try half(n)\n    require isPositive(h) else \"halved away\"\n    const q = try half(h)\n    return ok(q)\n}\n";
    assert!(diagnostics(source).is_empty(), "{:#?}", diagnostics(source));
}

/// The common body of the fixture pair, behind a helper so the two spellings differ in
/// nothing but the guard. The computed failure value is evaluated only on the failure path
/// in both.
const SUGARED: &str = "module main\n\nfn isPositive(n: int): bool {\n    return n > 0\n}\n\nfn inRange(n: int): bool {\n    return n < 100\n}\n\nfn renderHigh(n: int): string {\n    return \"too high\"\n}\n\nfn classify(n: int): Result<int, string> {\n    require isPositive(n) else \"not positive\"\n    require inRange(n) else renderHigh(n)\n    return ok(n)\n}\n\npub fn classifyPort(n: int): Result<int, string> {\n    return classify(n)\n}\n";

const HANDWRITTEN: &str = "module main\n\nfn isPositive(n: int): bool {\n    return n > 0\n}\n\nfn inRange(n: int): bool {\n    return n < 100\n}\n\nfn renderHigh(n: int): string {\n    return \"too high\"\n}\n\nfn classify(n: int): Result<int, string> {\n    if not isPositive(n) {\n        return err(\"not positive\")\n    }\n    if not inRange(n) {\n        return err(renderHigh(n))\n    }\n    return ok(n)\n}\n\npub fn classifyPort(n: int): Result<int, string> {\n    return classify(n)\n}\n";

/// The image's numbered sections, parsed from the canonical container layout:
/// magic(4) ‖ version(1) ‖ image-id(32) ‖ section-count(1) ‖
/// [id(1) ‖ len(u32 BE) ‖ body]*.
fn sections(bytes: &[u8]) -> BTreeMap<u8, Vec<u8>> {
    let tail = &bytes[37..];
    let count = tail[0] as usize;
    let mut sections = BTreeMap::new();
    let mut at = 1usize;
    for _ in 0..count {
        let id = tail[at];
        let len = u32::from_be_bytes(tail[at + 1..at + 5].try_into().expect("length")) as usize;
        sections.insert(id, tail[at + 5..at + 5 + len].to_vec());
        at += 5 + len;
    }
    assert_eq!(at, tail.len(), "sections cover the container tail");
    sections
}

/// The one section allowed to differ between the pair: the two spellings occupy different
/// source positions.
const SPAN_SECTION: u8 = 0x07;

/// The enforced form of the pure-sugar claim: same strings, types, constants, function
/// code, exports, and enums, differing only in the source-position table.
#[test]
fn require_is_byte_identical_to_the_handwritten_guard() {
    let sugared = sections(&image_bytes(SUGARED));
    let handwritten = sections(&image_bytes(HANDWRITTEN));
    assert_eq!(
        sugared.keys().collect::<Vec<_>>(),
        handwritten.keys().collect::<Vec<_>>(),
        "the fixture pair encodes the same section set"
    );
    for (&id, body) in &sugared {
        if id == SPAN_SECTION {
            continue;
        }
        assert_eq!(
            body, &handwritten[&id],
            "section {id:#04x} must be byte-identical between the sugared and handwritten forms"
        );
    }
    // The code section is present and non-trivial, so the identity above is not vacuous.
    assert!(
        !sugared[&0x05].is_empty(),
        "the function section carries the lowered code"
    );
}
