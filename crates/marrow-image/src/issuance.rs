//! The wide-ordinal carrier derivation: the compile-time facts about the private `u32`
//! carrier every owned pre-seal non-function logical identity uses.
//!
//! This module proves what `marrow-image` can prove alone: the carrier's `Layout` on both
//! supported targets, the losslessness of the widening every owned identity performs, and
//! the implication that a policy-clean draft can always narrow to its `u16` wire spelling.
//!
//! It deliberately does **not** state the population bound. How many rows an owned table
//! can hold under the admitted `ProjectInput` envelope depends on the source-capture
//! ceiling and on the compiler's generic-instantiation bound, and `marrow-image` knows
//! neither — it never imports the compiler, and it does not depend on the capture owner.
//! That half of the issuance derivation lives with those owners, in
//! `marrow-compile`'s `issuance` module, and the cross-crate parity test there is what
//! keeps the two halves describing one proof.
//!
//! The width proof is not a memory-feasibility proof. The hostile
//! maximum-amplification RSS gate is a separate obligation with its own corpus and
//! target-authority ceiling.

/// The widening every owned identity performs is lossless on both supported targets: an
/// ordinal is held as `u32` and indexed as `usize`, and `usize` is at least as wide as
/// the carrier on `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu` alike.
const _: () = assert!(size_of::<usize>() >= size_of::<u32>());

/// The carrier's exact `Layout`. A row's byte cost is derived from it, so a carrier
/// change moves this fact rather than costing silently.
const _: () = assert!(size_of::<u32>() == 4);
const _: () = assert!(align_of::<u32>() == 4);

/// The wire policy maxima all sit below the pre-seal carrier domain, so a policy-clean
/// draft can always narrow: the plan's checked accessors never meet a wide ordinal their
/// `u16` spelling cannot carry. This is the implication the temporary legacy plan rests
/// on, and it is a property of the bounds alone.
const _: () = assert!(crate::bounds::MAX_STRINGS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_CONSTS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_TYPES <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_ENUMS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_COLLECTIONS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_ROOTS <= u16::MAX as usize);
const _: () = assert!(crate::bounds::MAX_SITES <= u16::MAX as usize);
