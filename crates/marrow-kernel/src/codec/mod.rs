//! Runtime logical codecs: keys and values.
//!
//! A [`key::KeyScalar`] is a typed key value with an order-preserving byte
//! encoding; a [`value::RuntimeScalar`] is a typed value with a canonical,
//! non-order-preserving byte encoding. Both are the kernel's runtime
//! representation, distinct from the compiler's language-level scalar
//! classification. The proleptic-Gregorian calendar and the canonical temporal
//! text codec these share live in the pure `marrow-temporal` crate, so the
//! storeless compiler consumes the same owner without a store dependency.

pub mod key;
pub mod value;
pub(crate) mod varint;
