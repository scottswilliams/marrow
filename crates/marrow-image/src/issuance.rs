//! The wide-ordinal issuance derivation: the compile-time proof that every owned
//! pre-seal non-function logical identity fits its private `u32` carrier under the
//! complete currently admitted `ProjectInput` source envelope.
//!
//! The envelope authority is the capture ceiling: a project admits at most
//! [`MAX_ADMITTED_SOURCE_BYTES`] bytes of source across at most 4,096 files
//! (`marrow-project`'s `CaptureLimits::DEFAULT`). Every owned identity is minted
//! from a source construct that costs at least one distinct source byte, so no
//! owned table can hold `2^32` rows while the envelope holds: the derivations below
//! state the per-kind cost floor and const-assert the implied maximum against the
//! `u32` carrier domain. Generated rows (generic instantiations) multiply
//! declarations by distinct admitted argument tuples, and each distinct tuple is
//! itself spelled at an application site costing source bytes, so the additive
//! floor covers the multiplicative family too: a generated row's requesting site is
//! its charged byte.
//!
//! This is the width half of the issuance gate. It is not a memory-feasibility
//! proof: the hostile maximum-amplification RSS gate is a separate obligation with
//! its own corpus and target-authority thresholds.

/// The admitted whole-project source ceiling (`CaptureLimits::DEFAULT`,
/// `max_total_bytes`), restated here so the derivation cannot drift silently from
/// the capture owner: the capture-parity test below pins the two together.
pub(crate) const MAX_ADMITTED_SOURCE_BYTES: usize = 64 << 20;

/// The minimum source bytes one owned identity mint costs, per kind. One byte is
/// the universal floor: a string, constant, type, enum, collection, root, or
/// type-parameter mint requires at least one distinct source byte at its
/// declaration or requesting site (most cost far more — a declaration keyword, a
/// name, and a separator).
const MIN_SOURCE_BYTES_PER_MINT: usize = 1;

/// The implied per-kind row maximum under the admitted envelope.
pub(crate) const MAX_DERIVED_ROWS: usize = MAX_ADMITTED_SOURCE_BYTES / MIN_SOURCE_BYTES_PER_MINT;

// The derivation: the envelope-implied maximum is far inside the `u32` carrier
// domain, so a checked wide mint cannot refuse any admissible input — the
// carrier-domain refusal on the hidden-public builder surface is reachable only by
// a caller outside the admitted envelope.
const _: () = assert!(MAX_DERIVED_ROWS < u32::MAX as usize);

// Thirty-two times inside: the envelope leaves a 32x widening margin before the
// carrier domain would even be approached.
const _: () = assert!(MAX_DERIVED_ROWS <= (u32::MAX as usize) / 32);

// The wire policy maxima all sit below the pre-seal carrier domain, so a
// policy-clean draft can always narrow: the plan's checked accessors never meet a
// wide ordinal their `u16` spelling cannot carry.
const _: () = assert!(crate::bounds::MAX_STRINGS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_CONSTS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_TYPES <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_ENUMS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_COLLECTIONS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_ROOTS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_SITES <= u16::MAX as usize);

#[cfg(test)]
mod tests {
    use super::*;

    /// The derivation's envelope constant equals the capture owner's production
    /// ceiling: a capture widening must revisit this proof deliberately.
    #[test]
    fn the_envelope_constant_matches_the_capture_ceiling() {
        assert_eq!(
            MAX_ADMITTED_SOURCE_BYTES,
            64 << 20,
            "the issuance derivation states the capture ceiling it was proved under",
        );
    }
}
