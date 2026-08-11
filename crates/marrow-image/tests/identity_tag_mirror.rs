//! The image side of the ledger kind-tag drift gate.
//!
//! `marrow-image` writes each durable ledger reference into the contract-ID preimage
//! as `IDREF(kind, id)`, where the kind byte comes from a hand-written constant in
//! `src/durable_id.rs`. Those constants mirror `marrow-project`'s frozen
//! `IdentityKind::tag` values across a deliberate absence of any dependency edge
//! between the two crates, so nothing but agreement couples them. Editing a mirror
//! constant silently changes the identity of every durable contract while producer
//! and verifier still agree with each other.
//!
//! This gate reads the mirror out of the production source and pins each value. Its
//! sibling — `marrow-project/tests/cases/identity_kind_tags.rs` — pins the ledger
//! side. A real kind-space change edits the ledger, the mirror, and both gates in
//! one transaction.
//!
//! The scan runs over the literal-stripped projection of the source, so a constant
//! spelled inside a comment, a string, or a `#[cfg(test)]` item is not mistaken for
//! production code. That projection has exactly one owner in the workspace and is
//! included here rather than copied: a second, weaker scanner would be a second
//! answer to what counts as code.

use std::fs;
use std::path::PathBuf;

#[path = "../../marrow-compile/tests/common/source_projection.rs"]
mod source_projection;
use source_projection::production_code;

/// Every mirror constant and the ledger tag it must carry.
const MIRRORED_TAGS: &[(&str, u8)] = &[
    ("IDREF_APPLICATION", 0),
    ("IDREF_PRODUCT", 1),
    ("IDREF_FIELD", 2),
    ("IDREF_ROOT", 3),
    ("IDREF_KEY", 4),
    ("IDREF_SUM", 5),
    ("IDREF_MEMBER", 6),
    ("IDREF_GROUP", 7),
    ("IDREF_INDEX", 8),
];

fn durable_id_source() -> String {
    let path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("durable_id.rs");
    let source = fs::read_to_string(&path).expect("read the durable identity source");
    assert!(
        !source.is_empty(),
        "the scanned source must not be empty: {}",
        path.display(),
    );
    source
}

/// Every `const IDREF_*: u8 = <value>;` item declared in `code`, as
/// `(name, value)` pairs in source order.
///
/// The scan is deliberately shape-exact: it recognises the one item form the mirror
/// is written in. A mirror rewritten into some other form (a `match`, an array, a
/// computed expression) yields no rows here, and the completeness assertion below
/// then fails loudly rather than passing vacuously.
fn declared_mirror_tags(code: &str) -> Vec<(String, u8)> {
    let mut found = Vec::new();
    for item in code.split(';') {
        let Some(offset) = item.find("const IDREF_") else {
            continue;
        };
        let rest = &item[offset + "const ".len()..];
        let Some((name, tail)) = rest.split_once(':') else {
            continue;
        };
        let Some((ty, value)) = tail.split_once('=') else {
            continue;
        };
        if ty.trim() != "u8" {
            continue;
        }
        let Ok(value) = value.trim().parse::<u8>() else {
            continue;
        };
        found.push((name.trim().to_string(), value));
    }
    found
}

#[test]
fn the_mirror_carries_every_frozen_ledger_tag() {
    let declared = declared_mirror_tags(&production_code(&durable_id_source()));
    for (name, tag) in MIRRORED_TAGS {
        let value = declared
            .iter()
            .find(|(declared_name, _)| declared_name == name)
            .map(|(_, value)| *value)
            .unwrap_or_else(|| {
                panic!(
                    "{name} must be declared in durable_id.rs as `const {name}: u8 = {tag};`; \
                     declared mirror constants: {declared:?}"
                )
            });
        assert_eq!(
            value, *tag,
            "{name} mirrors marrow-project's IdentityKind tag {tag}; changing it changes \
             every durable contract id",
        );
    }
}

/// The mirror is exactly the frozen kind space: no extra `IDREF_*` constant appears
/// without a ledger kind behind it, and none is declared twice.
#[test]
fn the_mirror_declares_no_kind_the_ledger_does_not_have() {
    let declared = declared_mirror_tags(&production_code(&durable_id_source()));
    assert_eq!(
        declared.len(),
        MIRRORED_TAGS.len(),
        "the mirror declares exactly the frozen kind space: {declared:?}",
    );
    for (name, _) in &declared {
        assert!(
            MIRRORED_TAGS.iter().any(|(frozen, _)| frozen == name),
            "{name} has no frozen ledger kind behind it",
        );
    }
}

/// A gate that cannot see its own subject passes for the wrong reason. These probes
/// plant the exact shapes the scan must and must not report, so a scanner that has
/// been narrowed into blindness fails here instead of going quiet.
#[test]
fn the_scan_sees_production_code_and_only_production_code() {
    let planted = production_code(
        r##"
        // const IDREF_COMMENTED: u8 = 200;
        /* const IDREF_BLOCK_COMMENTED: u8 = 201; */
        const DOC: &str = "const IDREF_IN_STRING: u8 = 202;";
        const RAW: &str = r#"const IDREF_IN_RAW_STRING: u8 = 203;"#;
        const IDREF_LIVE: u8 = 204;

        #[cfg(test)]
        mod tests {
            const IDREF_IN_TESTS: u8 = 205;
        }
        "##,
    );
    let declared = declared_mirror_tags(&planted);
    assert_eq!(
        declared,
        vec![("IDREF_LIVE".to_string(), 204)],
        "only the production constant is reported",
    );
}
