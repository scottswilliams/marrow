//! The declared owned-heap ceiling, in one place.
//!
//! `H_owned` is the fixed retained-capacity ceiling every retention and transient term in
//! the analysis layer is sized against. Gates consume it; none of them may author it,
//! because a gate that sets its own ceiling from its own sample proves only that the
//! implementation equals itself.

/// The declared owned-heap ceiling, in bytes.
pub const H_OWNED_BYTES: u64 = 640 * 1024 * 1024;
