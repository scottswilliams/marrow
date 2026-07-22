//! Marrow's path kernel.
//!
//! The kernel owns the runtime representation of durable data — the logical
//! key and value codecs — and, as the durable runtime lands lane by lane, the
//! typed path over which every logical read and write passes. It sits below the
//! language surface: it consumes verified sites and typed scalars, never `.mw`
//! source.
//!
//! At the tracer stage the kernel hosts only the relocated logical codecs
//! ([`codec`]). These are the runtime representation of keys and values; the
//! language's own scalar classification is owned by the compiler, and the image
//! type tags are the frozen bridge between the two. Only `int`, `bool`, and
//! `string` are exercised today; the remaining scalar encodings are preserved
//! as known-answer-tested seeds and are not a frozen public value domain.

pub mod codec;
pub mod durable;
pub mod equality;
