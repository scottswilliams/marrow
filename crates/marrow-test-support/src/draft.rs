//! The draft construction seam: the budget a fixture is admitted under, the transaction
//! it builds in, and the bind-then-request protocol that mints an operation site.

use marrow_image::{
    AdmittedGraphInputPlan, CanonicalDeclarationPathSelector, DraftTxn, ImageDraft, PlannedSiteRef,
    RootOccurrenceSelector, SemanticTarget, bounds,
};

/// The image's own admitted-intake ceilings, as one plan.
///
/// A fixture states a census the way an admission owner does: a plan minted before
/// construction, whose terms `admit` checks against what a ProgramImage can hold. These
/// fixtures build small graphs, so the census is the image's own ceilings rather than a
/// second, narrower policy stated per file — what the plan closes is unadmitted intake,
/// not fixture size.
pub fn admitted_plan() -> AdmittedGraphInputPlan {
    AdmittedGraphInputPlan::admit(
        bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
        bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
        bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    )
}

/// The armed transaction over `owner`: the one admission every fixture opens.
pub fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner.begin_transaction()
}

/// Bind one canonical declaration path of `root` to the target that node admits and mint
/// its operation site.
///
/// [`PlannedSiteRef`] is the draft instruction IR's one site carrier, minted only under an
/// admitted transaction, and the binder is the only producer path to it — so a site a test
/// names is always one the draft answered for. Neither step takes a construction budget:
/// both selectors were published by an admitted construction, and the site table is its
/// own bounded owner.
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
