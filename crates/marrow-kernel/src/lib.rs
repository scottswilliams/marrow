//! Marrow's path kernel.
//!
//! The kernel owns the runtime representation of durable data — the logical key and
//! value codecs ([`codec`]), their equality and order ([`equality`]) — and the typed
//! path every logical read and write passes ([`durable`]). It sits below the language
//! surface: it consumes verified sites and typed scalars, never `.mw` source.
//!
//! The language's own scalar classification is owned by the compiler; the image type
//! tags are the frozen bridge between the two.

pub mod codec;
pub mod durable;
pub mod equality;
