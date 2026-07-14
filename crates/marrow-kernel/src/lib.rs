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

// The native `DurableStore::open` constructor lives with the store handle so the
// CLI can provision a redb-backed store; the in-memory engine backs the kernel's
// differential proving ground.
impl durable::DurableStore<marrow_store::NativeEngine> {
    /// Open (creating if needed) a write-capable native store at `path`. CLI-only
    /// caller at T01; dies at D00.
    pub fn open(
        path: &std::path::Path,
        schema: durable::StoreSchema,
        sites: Vec<durable::SiteSpec>,
    ) -> Result<Self, marrow_store::StoreError> {
        Ok(Self::from_engine(
            marrow_store::NativeEngine::open(path)?,
            schema,
            sites,
        ))
    }

    /// Open an existing native store read-only, never creating the file.
    pub fn open_read_only(
        path: &std::path::Path,
        schema: durable::StoreSchema,
        sites: Vec<durable::SiteSpec>,
    ) -> Result<Self, marrow_store::StoreError> {
        Ok(Self::from_engine(
            marrow_store::NativeEngine::open_read_only(path)?,
            schema,
            sites,
        ))
    }
}
