//! Architecture guard for the interned type-leaf invariant: every nominal
//! `MarrowType` leaf — resource, group entry, identity, enum — carries an interned
//! declaration id, never a stored spelling. A mismatch recovers the spelling by id
//! at render time, so no leaf may reintroduce a `String` or `Vec<String>` identity
//! field. This test reads the source rather than the runtime value because the
//! invariant is about the type's shape, and it also pins the removal of the
//! signature-slot repair pass the interning replaced.

use std::fs;
use std::path::PathBuf;

fn src(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

/// The body of the `pub enum MarrowType` declaration, from its opening brace to the
/// matching close, so the assertions below inspect only the leaf shapes.
fn marrow_type_enum_body(source: &str) -> String {
    let start = source
        .find("pub enum MarrowType {")
        .expect("MarrowType enum declaration");
    let mut depth = 0usize;
    let mut body = String::new();
    for ch in source[start..].chars() {
        body.push(ch);
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return body;
                }
            }
            _ => {}
        }
    }
    panic!("unterminated MarrowType enum declaration");
}

#[test]
fn every_nominal_type_leaf_is_interned_by_id() {
    let body = marrow_type_enum_body(&src("program.rs"));

    // Each nominal leaf names its declaration by an interned id newtype.
    for interned in [
        "Resource(ResourceId)",
        "resource: ResourceId",
        "layers: Vec<ResourceMemberId>",
        "Identity(StoreRootId)",
        "Enum(EnumId)",
    ] {
        assert!(
            body.contains(interned),
            "MarrowType must intern its nominal leaves; `{interned}` is missing:\n{body}",
        );
    }

    // No leaf may store a spelling: a `String` or `Vec<String>` identity field would
    // reintroduce the parse-and-compare-back the interning removed.
    for spelled in ["String", "Vec<String>"] {
        assert!(
            !body.contains(spelled),
            "a MarrowType leaf carries `{spelled}`; nominal identity is interned by id, \
             not stored as a spelling:\n{body}",
        );
    }
}

#[test]
fn the_signature_slot_repair_pass_is_gone() {
    // Interning made the post-assembly slot binding the single writer of every
    // named signature slot, so the old repair-pass identifier and the module-owned
    // enum-name context it threaded must not return.
    for (file, absent) in [
        ("analysis.rs", "normalize_program_named_types"),
        ("enums.rs", "normalize_program_named_types"),
        ("program.rs", "struct TypeNames"),
        ("program.rs", "TypeNames"),
    ] {
        let source = src(file);
        assert!(
            !source.contains(absent),
            "{file} still mentions `{absent}`; it should be gone after the leaf interning",
        );
    }
}
