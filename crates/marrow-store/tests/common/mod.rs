//! Shared helpers for the store integration-test binaries.
//!
//! Each test binary that pulls this in via `mod common` uses only the subset of
//! helpers it needs, so unused-helper warnings here are expected.
#![allow(dead_code)]

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;

/// The catalog id with `hex` zero-padded into the canonical 32-character body, the
/// one construction convention the store tests build ids by.
pub fn catalog_id(hex: &str) -> CatalogId {
    CatalogId::new(format!("cat_{hex:0>32}")).unwrap()
}

/// Walk a child layer from `first` to exhaustion, following `next` from each child,
/// and collect the children in cursor order. The four store child layers differ
/// only in which first/next cursor methods they call, so they share this walk.
pub fn collect_children(
    first: impl FnOnce() -> Result<Option<SavedKey>, StoreError>,
    next: impl Fn(&SavedKey) -> Result<Option<SavedKey>, StoreError>,
) -> Vec<SavedKey> {
    let mut children = Vec::new();
    let mut cursor = first().expect("first child");
    while let Some(child) = cursor {
        cursor = next(&child).expect("next child");
        children.push(child);
    }
    children
}
