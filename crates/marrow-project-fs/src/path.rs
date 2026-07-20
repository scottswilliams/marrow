//! Private root-relative path evidence carried by a physical failure.
//!
//! An `OperationalPath` owns the already-charged root-relative evidence a
//! [`PhysicalFailure`] may retain. It is crate-private: a consumer reaches its
//! rendered form only through the presentation facade, never as a raw path. The
//! charged native-path lease this type becomes — its budget, active/terminal
//! lifecycle, and checked release — is introduced with the physical producer in
//! the capture baseline; this declaration fixes the private evidence slot first.
//!
//! [`PhysicalFailure`]: crate::PhysicalFailure

use std::path::PathBuf;

/// Owned root-relative path evidence, private to the adapter.
// Constructed by the physical producer and read only by the presentation facade,
// both introduced in the capture baseline.
#[allow(dead_code)]
pub(crate) struct OperationalPath(PathBuf);
