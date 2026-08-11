// The construction budget a compiler-free fixture is admitted under.
//
// It is shared for the same reason the site seam is: seventeen copies of one census are
// seventeen places for a later ceiling change to be applied to sixteen of them. Its header
// is written as ordinary comments rather than inner doc comments so the one owner can be
// reached both as a `#[path]` module and, where a nested module has no directory to point
// at, by `include!`.

/// The image's own admitted-intake ceilings, as one plan.
///
/// A fixture states a census the way an admission owner does: a plan minted before
/// construction, whose terms `admit` checks against what a ProgramImage can hold. These
/// fixtures build small graphs, so the census is the image's own ceilings rather than a
/// second, narrower policy stated per file — what the plan closes is unadmitted intake,
/// not fixture size.
pub fn admitted_plan() -> marrow_image::AdmittedGraphInputPlan {
    marrow_image::AdmittedGraphInputPlan::admit(
        marrow_image::bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
        marrow_image::bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
        marrow_image::bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    )
    .expect("the image's own ceilings are admitted counts")
}
