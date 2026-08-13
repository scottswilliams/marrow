//! The bind-then-request site protocol: bind one occurrence, one canonical declaration
//! path, and the target that node admits, then request the site the binding names — all
//! under the construction budget the caller was admitted for.
//!
//! This is shared because it is the protocol, not a per-fixture convenience. Every test
//! across the workspace that names an operation site must perform the same two steps in
//! the same order — the binder is the only producer path to a site, so a site a test names
//! is always one the draft answered for.
//!
//! [`PlannedSiteRef`] is the draft instruction IR's one site carrier, minted only under
//! an admitted transaction. One copy of this protocol is one place to keep honest; nine
//! copies would be nine places, each free to drift before a change reached it.

use marrow_image::{
    CanonicalDeclarationPathSelector, DraftTxn, PlannedSiteRef, RootOccurrenceSelector,
    SemanticTarget,
};

/// Bind one canonical declaration path of `root` to the target that node admits and mint
/// its operation site.
///
/// Neither step takes a construction budget: both selectors were published by an admitted
/// construction, and the site table is its own bounded owner.
pub fn site(
    draft: &mut DraftTxn<'_>,
    root: &RootOccurrenceSelector,
    path: &CanonicalDeclarationPathSelector,
    target: SemanticTarget,
) -> PlannedSiteRef {
    let handle = draft
        .bind_occurrence_site(root, path, target)
        .expect("the path is a canonical path of this occurrence");
    draft.request_site(&handle).expect("the binding is live")
}
