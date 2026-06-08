//! Shared binding-index setup for the resolution and rename-safety suites.
//!
//! Every binding test drives the editor-facing `analyze_project` path and builds
//! the project-wide binding index over the resulting snapshot. This module owns
//! that setup so the focused suites assert against one index builder, not their
//! own copies.
//!
//! Each test binary includes this module, so not every binary exercises every
//! helper; the `dead_code` allowance keeps the shared surface intact.

#![allow(dead_code)]

use std::path::PathBuf;

use marrow_check::binding::{BindingIndex, SymbolKind, SymbolRef};
use marrow_check::build_binding_index;

use crate::support::analyze_overlay;

/// Analyze a set of `(relative-path, source)` files written under `src` and build
/// the binding index over the resulting snapshot. Returns the index and the
/// absolute paths of the written files, in the given order.
pub fn analyze(name: &str, files: &[(&str, &str)]) -> (BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = analyze_overlay(name, files);
    (build_binding_index(&snapshot), paths)
}

/// Like [`analyze`], but first assert the analyzed sources check cleanly, for the
/// navigation tests whose fixture is meant to type without diagnostics so the index
/// resolves against a well-formed program.
pub fn checked_index(name: &str, files: &[(&str, &str)]) -> (BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = analyze_overlay(name, files);
    assert!(
        !snapshot.report.has_errors(),
        "source should check cleanly: {:#?}",
        snapshot.report.diagnostics
    );
    (build_binding_index(&snapshot), paths)
}

/// The byte offset of the `n`-th (0-based) occurrence of `needle` in `source`,
/// plus one so the cursor lands inside the token rather than at its edge.
pub fn nth_offset(source: &str, needle: &str, n: usize) -> usize {
    let mut start = 0;
    for _ in 0..n {
        let found = source[start..].find(needle).expect("needle present") + start;
        start = found + needle.len();
    }
    source[start..].find(needle).expect("needle present") + start + 1
}

/// Assert `def` is an enum member whose definition span covers the declaration of
/// `decl` (e.g. `"active\n"`) in `source`, the repeated match-arm-navigation check.
pub fn assert_def_covers_member(def: &SymbolRef, source: &str, decl: &str) {
    assert_eq!(def.kind, SymbolKind::EnumMember, "{def:?}");
    let member_decl = source
        .find(decl)
        .unwrap_or_else(|| panic!("{decl:?} declaration in source"));
    assert!(
        def.span.start_byte <= member_decl && member_decl <= def.span.end_byte,
        "definition span covers the enum member declaration: {def:?}",
    );
}
