//! Bounded-traversal source rejections (E04): every ill-formed durable `for` head is a
//! precise typed compiler rejection carrying a located span, never a silent miscompile.
//!
//! These drive the production capture -> compile pipeline over a store whose identity is
//! complete (so compilation reaches lowering), and assert the typed diagnostic code plus
//! that the rejection carries a source span (1-based line and column), per the
//! repository's diagnostic-evidence standard. The rendered message prose is a renderer
//! concern and is not asserted.

use marrow_compile::SourceDiagnostic;

/// The identity ledger for the `^books` store and its `notes` branch, so every fixture
/// below is identity-complete and its only defect is the traversal head under test.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.pos 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     high-water 0\n\
     end\n";

/// The shared schema prefix: a `Book` root with a single-level `notes(pos)` branch. Each
/// test appends one function whose `for` head is the defect under test.
const HEADER: &str = r#"resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book
"#;

/// Capture and compile `HEADER + body`, returning the rejection diagnostics.
fn diagnostics_of(body: &str) -> Vec<SourceDiagnostic> {
    let source = format!("{HEADER}{body}");
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
    match marrow_compile::compile(&project) {
        Ok(_) => panic!("the traversal head should be rejected"),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

/// Assert compilation is rejected with a diagnostic of `code` whose span is present
/// (1-based line and column point into the source).
fn assert_rejected(body: &str, code: &str) {
    let diagnostics = diagnostics_of(body);
    let hit = diagnostics
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| {
            panic!(
                "expected a `{code}` diagnostic, got {:?}",
                diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
            )
        });
    assert!(
        hit.line() >= 1 && hit.column() >= 1,
        "the rejection carries a located span: {hit:?}"
    );
}

#[test]
fn a_durable_for_without_at_most_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k in ^books {
        t += k
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn a_bounded_traversal_without_on_more_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k in ^books at most 2 {
        t += k
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn at_most_on_a_range_for_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for i in 0..10 at most 5 {
        t += i
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn at_most_on_a_local_collection_for_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    var xs: List<int> = List()
    xs = append(xs, 1)
    for x in xs at most 5 {
        t += x
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn a_reversed_durable_traversal_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k in reversed ^books at most 5 {
        t += k
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.unsupported",
    );
}

#[test]
fn a_durable_for_with_more_than_a_key_and_an_address_is_rejected() {
    // A durable traversal binds the immediate key and, optionally, a per-iteration address
    // pin; a third binding has no durable meaning.
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k, visit, extra in ^books at most 5 {
        t += k
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.unsupported",
    );
}

#[test]
fn a_non_literal_bound_is_rejected() {
    assert_rejected(
        r#"pub fn f(n: int): int {
    var t = 0
    for k in ^books at most n {
        t += k
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn a_zero_bound_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k in ^books at most 0 {
        t += k
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn a_negative_bound_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k in ^books at most -1 {
        t += k
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn an_oversized_bound_is_rejected() {
    assert_rejected(
        r#"pub fn f(): int {
    var t = 0
    for k in ^books at most 65537 {
        t += k
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}

#[test]
fn an_unknown_traversed_branch_is_rejected() {
    assert_rejected(
        r#"pub fn f(n: int): int {
    var t = 0
    for p in ^books[n].unknownBranch at most 5 {
        t += p
    } on more {
        t = -1
    }
    return t
}
"#,
        "check.type",
    );
}
