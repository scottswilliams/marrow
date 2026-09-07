//! The fixture ledger writer shared by library and integration tests.

/// A ledger over `anchors`, each spelled `"<kind> <path>"` and given a distinct
/// seeded id in list order. The caller lists exactly the anchors its shape
/// declares; nothing is aligned by hand.
pub fn ledger(anchors: &[&str]) -> Vec<u8> {
    let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for (seed, anchor) in anchors.iter().enumerate() {
        out.push_str(&format!("id {anchor} {:032x}\n", seed as u128 + 1));
    }
    out.push_str("high-water 0\nend\n");
    out.into_bytes()
}
