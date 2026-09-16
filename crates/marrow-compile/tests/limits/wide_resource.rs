//! Wide-resource scale floor: a resource may declare thousands of sparse fields and still
//! compile to a canonical image. The width bound is a decode-allocation guard, not a
//! durable-format byte, so admitting a wider declaration costs no stored-format change.
//! The independent verifier re-check of the same width lives in `marrow-verify`.

use marrow_compile::{Compiled, compile};

use super::{ledger, project};

/// The ledger a `Wide` resource with `sparse` optional fields needs: application, product,
/// one identity per field, root, and key column.
fn wide_ids(sparse: usize) -> Vec<u8> {
    let mut anchors = vec![
        "application .".to_string(),
        "product Wide".to_string(),
        "field Wide.tag".to_string(),
    ];
    anchors.extend((0..sparse).map(|i| format!("field Wide.f{i}")));
    anchors.push("root wide".to_string());
    anchors.push("key wide.id".to_string());
    ledger(&anchors)
}

/// A wide, mostly-sparse resource: one required field plus `sparse` optional ones, stored
/// under an int key.
fn wide_source(sparse: usize) -> String {
    let mut src = String::from("module main\n\nresource Wide {\n    required tag: int\n");
    for i in 0..sparse {
        src.push_str(&format!("    f{i}: int\n"));
    }
    src.push_str("}\n\nstore ^wide[id: int]: Wide\n\n");
    src.push_str("pub fn noop(): int {\n    return 0\n}\n");
    src
}

fn compile_ok(sparse: usize) -> Compiled {
    let source = wide_source(sparse);
    let ids = wide_ids(sparse);
    compile(&project(&source, Some(&ids))).unwrap_or_else(|diagnostics| {
        panic!("expected a clean compile, got {diagnostics:#?}");
    })
}

/// Two thousand sparse fields compile to a canonical image; the width cap is a
/// decode-allocation guard, so admitting this costs no durable-format change.
#[test]
fn a_wide_resource_compiles() {
    let compiled = compile_ok(2000);
    assert!(
        !compiled.image.bytes.is_empty(),
        "the wide resource lowers to a non-empty image",
    );
}

/// The control for the wide case: width is a scale property, not a shape change.
#[test]
fn a_narrow_resource_compiles() {
    compile_ok(10);
}

/// An image bound is a decode-time allocation guard, never a stored-format byte, so widening
/// one must never change the encoded image of a program already inside the narrower bound.
/// An edit that serializes a bound constant, or otherwise perturbs an in-bounds program's
/// bytes, fails this content hash.
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
        "218fe46220c8e40adefce8a9c4e29196d7c37c39fa4d35e94788bcfb80eb25ed",
        "in-bounds image bytes changed; the monotone-widen law forbids this \
         (encoded {} bytes)",
        bytes.len(),
    );
}

/// The full [`marrow_image::bounds::MAX_RECORD_FIELDS`] width compiles cleanly. It anchors
/// ~4100 ledger rows, which `MAX_IDS_ROWS` must admit for one resource: the binder at this
/// width is the field-count guard, not the row cap.
#[test]
fn the_full_field_guard_width_durable_resource_compiles() {
    // 4095 sparse + the required `tag` = MAX_RECORD_FIELDS (4096) declared fields.
    let compiled = compile_ok(marrow_image::bounds::MAX_RECORD_FIELDS - 1);
    assert!(
        !compiled.image.bytes.is_empty(),
        "the full-width resource lowers to a non-empty image",
    );
}

/// A wide resource's image does not scale with its declared width when its code does not
/// touch every field, which is why [`marrow_image::bounds::MAX_IMAGE_BYTES`] is not the
/// durable-width binder. Emitting a site per field would cost ~84 B/field (~343 KB at this
/// width, past a 256 KiB ceiling); lazy field-leaf sites emit only the member tree, record
/// type, and interned names, so the same resource lands near 126 KB.
#[test]
fn a_wide_resource_image_is_decoupled_from_declared_width() {
    let bytes = compile_ok(4090).image.bytes.len();
    assert!(
        bytes < 256 * 1024,
        "a wide resource whose code touches no field must fit far under the eager \
         ~84 B/field cost now that field-leaf sites are lazy: {bytes} bytes",
    );
    assert!(
        bytes <= marrow_image::bounds::MAX_IMAGE_BYTES,
        "it fits the image ceiling ({} bytes): {bytes} bytes",
        marrow_image::bounds::MAX_IMAGE_BYTES,
    );
}
