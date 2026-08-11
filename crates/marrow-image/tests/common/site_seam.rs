//! The bind-then-request site protocol: bind one occurrence, one canonical declaration
//! path, and the target that node admits, then request the site the binding names — all
//! under the construction budget the caller was admitted for.
//!
//! This is shared because it is the protocol, not a per-fixture convenience. Every test
//! across the workspace that names an operation site must perform the same two steps in
//! the same order — the binder is the only producer path to a site, so a site a test names
//! is always one the draft answered for.
//!
//! [`LegacyDraftSiteOperand`] is a declared replacement target: it is the existing draft
//! instruction IR's operand, replaced wholesale when the planned site reference lands under
//! an admitted transaction. One copy is one place to migrate; nine copies would be nine
//! places, each free to drift into a different protocol before the migration reached it.

use marrow_image::{
    AdmittedGraphInputPlan, CanonicalDeclarationPathSelector, ImageDraft, LegacyDraftSiteOperand,
    RootOccurrenceSelector, SemanticTarget,
};

/// Bind one canonical declaration path of `root` to the target that node admits and mint
/// its operation site, under the construction budget `plan` admits.
///
/// The plan travels rather than being minted here: this helper is the shared protocol, not
/// an admission owner, and a helper that conjured its own capability would be exactly the
/// planless entry the plan exists to remove.
pub fn site(
    draft: &mut ImageDraft,
    plan: &AdmittedGraphInputPlan,
    root: &RootOccurrenceSelector,
    path: &CanonicalDeclarationPathSelector,
    target: SemanticTarget,
) -> LegacyDraftSiteOperand {
    let handle = draft
        .bind_occurrence_site(plan, root, path, target)
        .expect("the path is a canonical path of this occurrence");
    draft
        .request_site(plan, &handle)
        .expect("the binding is live")
}
