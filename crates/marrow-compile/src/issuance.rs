//! The wide-ordinal population derivation: how many rows of each owned pre-seal
//! non-function kind an admitted `ProjectInput` can drive into the draft, and why that
//! count stays inside the private `u32` carrier.
//!
//! This is the half of the issuance derivation that needs both the source-capture
//! ceiling and the compiler's own generic-instantiation bound, so it lives here rather
//! than beside the carrier facts in `marrow-image`.
//!
//! # Two populations, bounded together
//!
//! **Declared rows.** A string, constant, record, enum, collection, root, site, or type
//! parameter written in source is minted from a construct occupying at least one distinct
//! source byte at its declaration site. The capture owner admits at most
//! [`MAX_ADMITTED_SOURCE_BYTES`] bytes across a whole project, so declared rows of any one
//! kind cannot exceed that count. Rows minted at most once per admitted file are bounded
//! by the capture owner's file count, which is itself no larger than that byte ceiling.
//!
//! **Generated rows.** Generic type and function instantiations are *not* charged to
//! distinct source bytes. A single syntactic call can keep generating fresh instances when
//! a generic recurses over an ever-growing type. The compiler instead refuses the mint once
//! `type_insts.len() + fn_insts.len()` reaches [`MAX_INSTANTIATIONS`], before appending the
//! row, and reports the located `check.instantiation_limit` diagnostic.
//!
//! # The two do multiply, and this is the term
//!
//! Interning is keyed — `intern_string` and the `intern_int`/`intern_text` family return
//! the id already held and mutate nothing — so two instances of one template that lower the
//! same body intern the same values. That much is additive, and it is what the previous
//! derivation generalized from.
//!
//! It does not generalize. Filling one instance materializes the template's *declared*
//! shape into per-instance draft and registry rows: a struct fill appends one field entry
//! per declared field, an enum fill appends one variant entry per declared variant and one
//! payload leaf per declared leaf, and each is retained in the instance body the registry
//! holds. None of those entries is a repeated value that keying can absorb — each belongs
//! to a distinct instance row — so for a template of declared width `W` the population is
//! instances times `W`.
//!
//! A collection application inside a filled body is the sharper case. `List<T>`/`Map<K,V>`
//! dedup by their *source* element type, and a divergent instance carries a different
//! element type at every step, so each instantiation can mint fresh collection rows. The
//! collection mint has no direct pre-mint ceiling; [`marrow_image::bounds::MAX_COLLECTIONS`]
//! is a later image-policy verdict. Carrier safety therefore cannot use that lower policy
//! maximum.
//!
//! It does not need to. Every source template belongs to exactly one admitted file, and
//! every field, variant, payload leaf, or distinct collection application materialized
//! from that template occupies syntax in that file. The materialized width of any one
//! source instance is therefore at most [`MAX_PARSED_FILE_BYTES`]. The fileless built-in
//! `Option` and `Result` templates have a current maximum per-kind width of two; that
//! code-owned shape cannot be const-read, so the named fixed term below records it and the
//! const derivation asserts that the file ceiling covers it. All generated type and
//! function instances share [`MAX_INSTANTIATIONS`], so summing the width over every
//! generated instance gives the honest per-kind statement
//!
//! ```text
//! rows(kind) <= MAX_ADMITTED_SOURCE_BYTES
//!            + MAX_INSTANTIATIONS * MAX_PARSED_FILE_BYTES
//! ```
//!
//! This covers the width-one instance rows, record fields, enum variants and payloads,
//! value-shape nodes, and distinct collection applications without pretending that the
//! instantiation count alone bounds those populations.
//!
//! # The carrier holds it
//!
//! The bound above is asserted at compile time against the `u32` domain using the live
//! capture, parse, and instantiation owners. A widening of any one of them that invalidates
//! the derivation breaks the build. A checked wide mint therefore cannot refuse admitted
//! compiler input: the carrier-domain refusal on the hidden-public builder surface is
//! reachable only by a caller outside that envelope, which is why the production compiler
//! maps it to invariant.
//!
//! This carrier proof is not a per-kind image-admission proof. The direct table ceilings,
//! including [`marrow_image::bounds::MAX_COLLECTIONS`], remain policy verdicts applied by
//! the image measure/encode path after construction. Nor is it a peak-memory proof: no
//! compiler-enforced working-set ceiling exists, and the separate issuance RSS gate records
//! that evidence. Function instructions are outside this non-function identity population;
//! their encoded byte length is accumulated wide and bounded separately by
//! [`marrow_image::bounds::MAX_CODE_BYTES`] as
//! [`marrow_image::ImageBuildError::CodeTooLong`].
//!
//! `Layout` and lossless-widening facts for the carrier itself are asserted in
//! `marrow-image`'s `issuance` module, on both supported targets.

use marrow_project::CaptureLimits;

use crate::{MAX_PARSED_FILE_BYTES, types::MAX_INSTANTIATIONS};

/// The admitted whole-project source ceiling, read from the capture owner rather than
/// restated, so a capture widening cannot drift past this derivation silently.
const MAX_ADMITTED_SOURCE_BYTES: usize = CaptureLimits::DEFAULT.max_total_bytes();

/// The admitted file ceiling, used to keep file-fixed rows inside the declared-row term.
const MAX_ADMITTED_FILES: usize = CaptureLimits::DEFAULT.max_files();

/// The largest number of rows of one kind materialized by either fileless reserved
/// template (`Option` or `Result`) for one instance. Their Rust definitions are not const
/// data, so this is the named audit term a change to either definition must update.
const MAX_FILELESS_TEMPLATE_WIDTH: usize = 2;

/// The derived per-kind row maximum: declared rows charged to distinct project source
/// bytes, plus every generated instance carrying the widest template one admitted file can
/// contain.
const MAX_DERIVED_ROWS: usize =
    MAX_ADMITTED_SOURCE_BYTES + MAX_INSTANTIATIONS * MAX_PARSED_FILE_BYTES;

/// The derivation's conclusion, as one named predicate: the population an admitted
/// project can drive fits the wide carrier.
///
/// The envelope-implied maximum is inside the `u32` carrier domain, so a checked wide mint
/// cannot refuse an admissible input. The multiplication deliberately sits in this const
/// derivation: widening the parser or instantiation owner past the carrier breaks the
/// build.
const fn population_fits_the_wide_carrier() -> bool {
    MAX_ADMITTED_FILES <= MAX_ADMITTED_SOURCE_BYTES
        && MAX_FILELESS_TEMPLATE_WIDTH <= MAX_PARSED_FILE_BYTES
        && MAX_DERIVED_ROWS < u32::MAX as usize
}

const _: () = assert!(population_fits_the_wide_carrier());

#[cfg(test)]
mod tests {
    use super::*;

    /// The derivation reads the capture owner's live ceiling. Spelling the number here
    /// instead is how the previous derivation drifted: it compared one local literal to
    /// another and would have stayed green through any capture change.
    #[test]
    fn the_envelope_uses_the_live_capture_and_parse_ceilings() {
        assert_eq!(
            MAX_ADMITTED_SOURCE_BYTES,
            CaptureLimits::DEFAULT.max_total_bytes(),
        );
        assert_eq!(MAX_ADMITTED_FILES, CaptureLimits::DEFAULT.max_files());
        assert_eq!(MAX_FILELESS_TEMPLATE_WIDTH, 2);
        assert!(MAX_PARSED_FILE_BYTES <= CaptureLimits::DEFAULT.max_file_bytes());
    }

    /// The generated-row term multiplies the two live owners rather than copying either.
    #[test]
    fn the_generated_row_term_is_the_live_instantiation_and_file_ceiling() {
        assert_eq!(
            MAX_DERIVED_ROWS - MAX_ADMITTED_SOURCE_BYTES,
            MAX_INSTANTIATIONS * MAX_PARSED_FILE_BYTES,
        );
    }
}
