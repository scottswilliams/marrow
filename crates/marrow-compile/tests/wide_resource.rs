//! WR01 wide-resource scale floor: a resource may declare thousands of sparse
//! fields (the M-shaped workload) and still compile to a canonical image. The
//! width bound is a law-9 decode-allocation guard, not a durable-format byte, so
//! widening it admits the wide declaration with no stored-format change. The
//! independent verifier re-check of the same width lives in `marrow-verify`.

use marrow_compile::{Compiled, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// A distinct 16-byte identity rendered as 32 lowercase hex, seeded by `n`.
fn hexid(n: u64) -> String {
    format!("{n:032x}")
}

/// The durable identity ledger a `Wide` resource with `sparse` optional fields
/// needs: the application, the product, one identity per field (the required
/// `tag` plus `f0..f{sparse}`), the root, and its key column.
fn wide_ids(sparse: usize) -> Vec<u8> {
    let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    let mut seed = 1u64;
    let line = |kind_path: String, s: &mut u64, out: &mut String| {
        out.push_str(&format!("id {kind_path} {}\n", hexid(*s)));
        *s += 1;
    };
    line("application .".into(), &mut seed, &mut out);
    line("product Wide".into(), &mut seed, &mut out);
    line("field Wide.tag".into(), &mut seed, &mut out);
    for i in 0..sparse {
        line(format!("field Wide.f{i}"), &mut seed, &mut out);
    }
    line("root wide".into(), &mut seed, &mut out);
    line("key wide.id".into(), &mut seed, &mut out);
    out.push_str("high-water 0\nend\n");
    out.into_bytes()
}

/// A resource declaring one required key-bearing field and `sparse` optional
/// fields, stored under an int key — the M-shaped workload: a wide, mostly-sparse
/// resource.
fn wide_source(sparse: usize) -> String {
    let mut src = String::from("module main\n\nresource Wide {\n    required tag: int\n");
    for i in 0..sparse {
        src.push_str(&format!("    f{i}: int\n"));
    }
    src.push_str("}\n\nstore ^wide[id: int]: Wide\n\n");
    src.push_str("pub fn noop(): int {\n    return 0\n}\n");
    src
}

fn project(source: &str, ids: &[u8]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, Some(ids), &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn compile_ok(sparse: usize) -> Compiled {
    let source = wide_source(sparse);
    let ids = wide_ids(sparse);
    compile(&project(&source, &ids)).unwrap_or_else(|diagnostics| {
        panic!("expected a clean compile, got {diagnostics:#?}");
    })
}

/// The M-shaped declared width — two thousand sparse fields — compiles to a
/// canonical image. Before WR01 the width cap rejected it; the widened law-9 bound
/// now admits it with no durable-format change.
#[test]
fn a_wide_resource_compiles() {
    let compiled = compile_ok(2000);
    assert!(
        !compiled.image.bytes.is_empty(),
        "the wide resource lowers to a non-empty image",
    );
}

/// A modest declared width compiles, so the wide case is a scale property, not a
/// shape change.
#[test]
fn a_narrow_resource_compiles() {
    compile_ok(10);
}
