//! Runtime logical codecs: keys, values, and the civil-date arithmetic they share.
//!
//! A [`key::KeyScalar`] is a typed key value with an order-preserving byte
//! encoding; a [`value::RuntimeScalar`] is a typed value with a canonical,
//! non-order-preserving byte encoding. Both are the kernel's runtime
//! representation, distinct from the compiler's language-level scalar
//! classification.

pub mod civil;
pub mod key;
pub mod value;
