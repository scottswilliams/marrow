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
//! What bounds *the instance rows themselves* is the compiler's own hard ceiling: a mint is
//! refused once `type_insts.len() + fn_insts.len()` reaches [`MAX_INSTANTIATIONS`], checked
//! *before* the row is appended and reported as a located `check.instantiation_limit`.
//!
//! **That ceiling counts two populations and no others.** It is the sum of the generic type
//! instances and the generic function instances. It is not a bound on generated rows of
//! every kind, and this derivation previously said it was.
//!
//! # The two do multiply, and this is the term
//!
//! Interning is keyed — `intern_string` and the `intern_int`/`intern_text` family return
//! the id already held and mutate nothing — so two instances of one template that lower the
//! same body intern the same values. That much is additive, and it is what the previous
//! derivation generalized from.
//!
//! It does not generalize. Filling one instance body copies the template's *declared* shape
//! into the draft: a struct fill appends one field entry per declared field, an enum fill
//! appends one variant entry per declared variant and one payload leaf per declared leaf,
//! and each is retained in the instance body the registry holds. None of those entries is a
//! repeated value that keying can absorb — each belongs to a distinct instance row — so for
//! a template of declared width `W` the population is instances times `W`.
//!
//! A collection application inside a filled body is the sharper case. `List<T>`/`Map<K,V>`
//! dedup by their *source* element type, and a divergent instance carries a different
//! element type at every step, so each instantiation mints fresh collection rows. The mint
//! consults no ceiling at all: [`marrow_image::bounds::MAX_COLLECTIONS`] is a policy verdict
//! the encoder draws, and a compile that exhausts the instantiation ceiling never reaches
//! the encoder. So the collection kind is bounded during compilation by nothing this
//! derivation can name.
//!
//! The honest per-kind statement is therefore
//!
//! ```text
//! rows(kind) <= MAX_ADMITTED_SOURCE_BYTES + MAX_INSTANTIATIONS * per_instance(kind)
//! ```
//!
//! where `per_instance(kind)` is the declared width the fill copies — `1` for the two
//! counted instance kinds, the template's field or variant/payload width for the draft
//! shape kinds, and the count of distinct collection applications in the template body for
//! the collection kind.
//!
//! # What this derivation does and does not establish
//!
//! For `per_instance(kind) == 1` — the generic type instance and generic function instance
//! kinds, and every declared kind charged to distinct source bytes — the additive bound
//! below holds and the carrier conclusion follows.
//!
//! For the width-carried kinds it does not. A template's declared width is itself charged
//! to source bytes, so `per_instance(kind)` is bounded only by the whole-project source
//! ceiling, and the product of that ceiling with [`MAX_INSTANTIATIONS`] is far outside the
//! `u32` carrier domain. **No compile-time ceiling closes that gap**: the record-field,
//! variant, payload, and collection bounds are all drawn by the encoder's policy walk, and
//! the hostile compile refuses with a source diagnostic before encoding. What exhausts
//! first in practice is memory, and memory is not a compiler-enforced bound — the issuance
//! RSS gate measures a hostile compile already standing above the declared owned-heap
//! ceiling, which is the open question this gap belongs to.
//!
//! This is recorded rather than closed. The conclusion asserted below is scoped to the
//! kinds it covers, and the width-carried kinds are named as unestablished rather than
//! folded in.
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

/// The derived per-kind row maximum **for the kinds whose per-instance width is one**:
/// declared rows charged to distinct source bytes, plus the generated instance rows the
/// instantiation ceiling admits.
///
/// The width-carried kinds are outside this term by construction — see the module header.
/// Restating this constant as a whole-population bound is the false premise that derivation
/// carried twice.
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
