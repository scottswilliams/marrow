//! Wide-resource scale floor: a resource may declare thousands of sparse
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
/// canonical image. The width cap once rejected it; the widened law-9 bound
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

/// Monotone-widen byte-identity law: an image representational bound is a decode-time
/// allocation guard, never a stored-format byte, so widening a bound must never change
/// the encoded image of a program already within the old bounds. This pins, by content
/// hash frozen at this head, the encoded bytes of a small durable resource that sits
/// well within the former 64-type / 256 KiB caps. Any future edit that serializes a
/// bound constant, or otherwise perturbs an in-bounds program's bytes, turns this red.
#[test]
fn an_in_bounds_program_has_frozen_image_bytes() {
    let bytes = compile_ok(10).image.bytes;
    let hex: String = marrow_image::image_id(&bytes)
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        hex,
        "f1500613d311c58ae7652b3048d13390dc07cab740793f517b63e43185ef3397",
        "in-bounds image bytes changed; the monotone-widen law forbids this \
         (encoded {} bytes)",
        bytes.len(),
    );
}

/// The full field guard is reachable: a durable resource declaring the complete
/// [`marrow_image::bounds::MAX_RECORD_FIELDS`] width (4096 fields — the required `tag`
/// plus 4095 sparse fields) compiles cleanly. This width anchors ~4100 durable-identity
/// ledger rows (one `Field` per field plus application/product/root/key overhead), so it
/// was previously refused by the 4096-row ledger cap; the widened `MAX_IDS_ROWS` admits
/// the full guard for a single wide resource. The binder at this width is now the
/// field-count guard, not the ledger row cap.
#[test]
fn the_full_field_guard_width_durable_resource_compiles() {
    // 4095 sparse + the required `tag` = MAX_RECORD_FIELDS (4096) declared fields.
    let compiled = compile_ok(marrow_image::bounds::MAX_RECORD_FIELDS - 1);
    assert!(
        !compiled.image.bytes.is_empty(),
        "the full-width resource lowers to a non-empty image",
    );
}

/// A durable resource near the record-field width at ~4090 sparse fields encodes to
/// ~343 KB. That exceeds the v0 256 KiB image ceiling and fits the widened 512 KiB one,
/// so it pins [`marrow_image::bounds::MAX_IMAGE_BYTES`] as the load-bearing bound for a
/// wide durable resource rather than the field-count guard.
#[test]
fn a_near_max_width_durable_resource_needs_the_widened_image_ceiling() {
    let bytes = compile_ok(4090).image.bytes.len();
    assert!(
        bytes > 256 * 1024,
        "the near-max-width durable resource exceeds the v0 256 KiB ceiling: {bytes} bytes",
    );
    assert!(
        bytes <= marrow_image::bounds::MAX_IMAGE_BYTES,
        "it must fit the widened image ceiling ({} bytes): {bytes} bytes",
        marrow_image::bounds::MAX_IMAGE_BYTES,
    );
}
