//! `CatalogId` is the opaque stable storage identity every physical tree-cell key is
//! built from, so its public constructor is a fail-closed gate on the `cat_<32 lowercase
//! hex>` shape. A value that slips past it would mint a physical key under a malformed
//! identity. These are Tier-0 boundary laws over the public `cell` surface: exactly the
//! canonical shape is accepted, every length and character boundary is rejected with the
//! typed `CellIdError`, and an accepted id reads back unchanged.

use marrow_store::cell::{CatalogId, CellIdError};

const CANONICAL: &str = "cat_0123456789abcdef0123456789abcdef";

#[test]
fn the_canonical_shape_is_accepted_and_preserved() {
    let id = CatalogId::new(CANONICAL).expect("the canonical shape is accepted");
    assert_eq!(id.as_str(), CANONICAL, "an accepted id is stored verbatim");
    // Every lowercase hex digit is a legal body character.
    CatalogId::new("cat_abcdef0123456789abcdef0123456789").expect("all-hex body accepted");
}

/// Every rejected spelling resolves to the one typed error, never a different error or a
/// silent acceptance.
fn assert_rejected(id: &str) {
    assert_eq!(
        CatalogId::new(id),
        Err(CellIdError),
        "expected a typed rejection of {id:?}",
    );
}

#[test]
fn the_length_boundary_is_exact() {
    // 32 hex digits is the only accepted body length; one short or one long is rejected.
    let body = "0123456789abcdef0123456789abcdef"; // 32 chars
    assert!(CatalogId::new(format!("cat_{body}")).is_ok());

    assert_rejected(&format!("cat_{}", &body[..31])); // 31 digits
    assert_rejected(&format!("cat_{body}0")); // 33 digits
    assert_rejected("cat_"); // empty body
}

#[test]
fn non_hex_and_non_lowercase_bodies_are_rejected() {
    // Uppercase hex is non-canonical even though it is a valid hex digit.
    assert_rejected("cat_0123456789ABCDEF0123456789abcdef");
    assert_rejected("cat_0123456789abcdef0123456789abcdeF");
    // A non-hex letter anywhere in the body.
    assert_rejected("cat_g123456789abcdef0123456789abcdef");
    assert_rejected("cat_0123456789abcdef0123456789abcdeg");
    // A separator inside an otherwise-32-char body.
    assert_rejected("cat_0123456789abcdef_123456789abcdef");
    // A trailing separator that would pad the body to 32 visible chars.
    assert_rejected("cat_0123456789abcdef0123456789abcdef_1");
}

#[test]
fn a_missing_or_wrong_prefix_is_rejected() {
    let body = "0123456789abcdef0123456789abcdef";
    assert_rejected(body); // no prefix at all
    assert_rejected(&format!("CAT_{body}")); // wrong-case prefix
    assert_rejected(&format!("cat{body}")); // missing underscore
    assert_rejected(&format!("xcat_{body}")); // prefix not at the start
    assert_rejected(""); // empty string
}
