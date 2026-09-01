//! The one writer of the `.marrow/ids` text a fixture hands the compiler.
//!
//! The format is the identity ledger's, not a test's: five suites had hand-rolled
//! copies of the same header, `id <kind> <path> <hex>` line, and trailer, so a
//! change to the format would have been five separate edits and any one of them
//! could have drifted into agreeing with nothing.

#![allow(dead_code)]

use std::collections::BTreeMap;

use marrow_compile::{CompileFailure, compile};
use marrow_project::{IdentityAnchor, ProjectInput};

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

/// The ledger a project needs, minted from the gaps the compiler itself reports.
///
/// A hand-written anchor list would encode a second opinion about which anchors a
/// shape declares; this resolves the typed gaps until the compiler reports none, so
/// the fixture and the compiler cannot disagree about the anchor set.
pub fn minted(capture: impl Fn(Option<&[u8]>) -> ProjectInput) -> ProjectInput {
    let minted = converge(&capture);
    capture(Some(serialize(&minted).as_bytes()))
}

/// The complete `(kind, path)` anchor set the compiler demands of `capture`'s
/// project, in the ledger's own canonical order.
///
/// Same convergence as [`minted`], reporting the anchors rather than the ledger:
/// the set a durable declaration mints is committed identity, so a fixture that
/// compares it is comparing what `.marrow/ids` will hold.
pub fn minted_anchors(capture: impl Fn(Option<&[u8]>) -> ProjectInput) -> Vec<IdentityAnchor> {
    converge(&capture).into_keys().collect()
}

/// One convergence, both artifacts: the complete anchor set in canonical order and
/// the serialized ledger bytes that satisfy it. For a suite whose tests share one
/// corpus, this is the run they share instead of each converging again.
pub fn converged(
    capture: impl Fn(Option<&[u8]>) -> ProjectInput,
) -> (Vec<IdentityAnchor>, Vec<u8>) {
    let minted = converge(&capture);
    let ledger = serialize(&minted).into_bytes();
    (minted.into_keys().collect(), ledger)
}

fn converge(capture: &impl Fn(Option<&[u8]>) -> ProjectInput) -> BTreeMap<IdentityAnchor, String> {
    let mut minted: BTreeMap<IdentityAnchor, String> = BTreeMap::new();
    for _ in 0..64 {
        let text = serialize(&minted);
        let project = capture(Some(text.as_bytes()));
        let gaps: Vec<IdentityAnchor> = match compile(&project) {
            Ok(_) => Vec::new(),
            Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
                .into_iter()
                .filter_map(|row| row.identity_gap().map(marrow_compile::IdentityGap::anchor))
                .collect(),
            Err(other) => panic!("expected diagnostics while minting, got {other:?}"),
        };
        let mut fresh = false;
        for anchor in gaps {
            if !minted.contains_key(&anchor) {
                let id = format!("{:032x}", minted.len() + 1);
                minted.insert(anchor, id);
                fresh = true;
            }
        }
        if !fresh {
            return minted;
        }
    }
    panic!("minting a complete identity ledger did not converge");
}

fn serialize(minted: &BTreeMap<IdentityAnchor, String>) -> String {
    let anchors: Vec<String> = minted
        .iter()
        .map(|(anchor, id)| format!("id {} {} {id}", anchor.kind.keyword(), anchor.path))
        .collect();
    let mut text = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for line in &anchors {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&format!("high-water {}\nend\n", minted.len()));
    text
}
