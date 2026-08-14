//! The wide-ordinal population derivation: how many rows of each owned pre-seal
//! non-function kind an admitted `ProjectInput` can drive into the draft, and why that
//! count stays inside the private `u32` carrier.
//!
//! This is the half of the issuance derivation that needs both the source-capture
//! ceiling and the compiler's own generic-instantiation bound, so it lives here rather
//! than beside the carrier facts in `marrow-image`.
//!
//! # Two populations, bounded two different ways
//!
//! **Declared rows.** A string, constant, record, enum, collection, root, site, or type
//! parameter written in source is minted from a construct occupying at least one distinct
//! source byte at its declaration site. The capture owner admits at most
//! [`MAX_ADMITTED_SOURCE_BYTES`] bytes across a whole project, so declared rows of any one
//! kind cannot exceed that count.
//!
//! **Generated rows.** Generic type and function instantiations are *not* charged to
//! distinct source bytes, and it is important that this derivation not claim they are: a
//! single syntactic call can generate instances without limit, because a generic that
//! recurses over an ever-growing type instantiates itself afresh at each step. One call
//! site, unbounded generated rows.
//!
//! What bounds them is the compiler's own hard ceiling: every mint is refused once
//! `type_insts.len() + fn_insts.len()` reaches [`MAX_INSTANTIATIONS`], checked *before*
//! the row is appended and reported as a located `check.instantiation_limit`. The
//! divergent-monomorphization corpus is what exercises that ceiling. So generated rows of
//! all kinds together cannot exceed [`MAX_INSTANTIATIONS`].
//!
//! # Why the two do not multiply
//!
//! An instantiated body is lowered once per instance, and lowering interns. If interning
//! appended a row per lowering, the population would be instances times body size and the
//! carrier would not hold it. It does not: `intern_string` and the `intern_int`/
//! `intern_text` family are **keyed**, so a repeated value returns the id already held and
//! mutates nothing. Two instances of one template lower the same body and intern the same
//! values, yielding the same rows.
//!
//! The rows a lowering can therefore add are one per *distinct value*, and a distinct
//! value is either spelled in source (already charged to its source bytes) or synthesized
//! per instantiation — a monomorphized type-name spelling, at most one per instance. Both
//! terms are already counted, so the population of any owned kind is additive:
//!
//! ```text
//! rows(kind) <= MAX_ADMITTED_SOURCE_BYTES + MAX_INSTANTIATIONS
//! ```
//!
//! # The carrier holds it
//!
//! The bound above is asserted at compile time against the `u32` domain with a wide
//! margin, so a checked wide mint cannot refuse any admissible input: the carrier-domain
//! refusal on the hidden-public builder surface is reachable only by a caller outside the
//! admitted envelope, which is exactly why the production compiler maps it to invariant.
//!
//! `Layout` and lossless-widening facts for the carrier itself are asserted in
//! `marrow-image`'s `issuance` module, on both supported targets.

use marrow_project::CaptureLimits;

use crate::types::MAX_INSTANTIATIONS;

/// The admitted whole-project source ceiling, read from the capture owner rather than
/// restated, so a capture widening cannot drift past this derivation silently.
const MAX_ADMITTED_SOURCE_BYTES: usize = CaptureLimits::DEFAULT.max_total_bytes();

/// The admitted whole-project file ceiling, for the per-file terms above.
const MAX_ADMITTED_FILES: usize = CaptureLimits::DEFAULT.max_files();

/// The derived per-kind row maximum: declared rows charged to distinct source bytes, plus
/// the generated rows the instantiation ceiling admits.
const MAX_DERIVED_ROWS: usize = MAX_ADMITTED_SOURCE_BYTES + MAX_INSTANTIATIONS;

/// The derivation's conclusion, as one named predicate: the population an admitted
/// project can drive fits the wide carrier, with margin.
///
/// The three conjuncts are (i) the envelope-implied maximum is inside the `u32` carrier
/// domain, so a checked wide mint cannot refuse an admissible input; (ii) it is at least
/// thirty-two times inside, so widening the capture ceiling by an order of magnitude
/// would still not approach the carrier and this proof is not sitting on its own edge;
/// and (iii) a project cannot admit more files than bytes, which is what lets the
/// per-file terms fold into the whole-project byte ceiling.
const fn population_fits_the_wide_carrier() -> bool {
    MAX_DERIVED_ROWS < u32::MAX as usize
        && MAX_DERIVED_ROWS <= (u32::MAX as usize) / 32
        && MAX_ADMITTED_FILES <= MAX_ADMITTED_SOURCE_BYTES
}

const _: () = assert!(population_fits_the_wide_carrier());

#[cfg(test)]
mod tests {
    use super::*;

    /// The derivation reads the capture owner's live ceiling. Spelling the number here
    /// instead is how the previous derivation drifted: it compared one local literal to
    /// another and would have stayed green through any capture change.
    #[test]
    fn the_envelope_is_the_capture_owners_live_ceiling() {
        assert_eq!(
            MAX_ADMITTED_SOURCE_BYTES,
            CaptureLimits::DEFAULT.max_total_bytes(),
        );
        assert_eq!(MAX_ADMITTED_FILES, CaptureLimits::DEFAULT.max_files());
    }

    /// The generated-row term is the compiler's live instantiation ceiling, not a copy.
    #[test]
    fn the_generated_row_term_is_the_live_instantiation_ceiling() {
        assert_eq!(
            MAX_DERIVED_ROWS - MAX_ADMITTED_SOURCE_BYTES,
            MAX_INSTANTIATIONS,
        );
    }
}
