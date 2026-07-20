//! Private root-relative path evidence and the adapter's path-budget seam.
//!
//! `OperationalPath` owns the root-relative evidence a [`PhysicalFailure`] may
//! retain; a consumer reaches its rendered form only through the presentation
//! facade, never as a raw path. `SourceSpelling` owns the forward-slash UTF-8
//! spelling of a selected source before it becomes a `CapturedFile`. `PathBudget`
//! is the current-behavior seam for the live/aggregate native-path accounting the
//! target adapter enforces: in this baseline it charges nothing and never fails,
//! and it is deliberately insufficient against the target law. The checked lease
//! lifecycle replaces it in target hardening.
//!
//! [`PhysicalFailure`]: crate::PhysicalFailure

use std::path::{Path, PathBuf};

/// Owned root-relative path evidence, private to the adapter. Not `Clone`: a
/// selected path is moved into failure evidence, never duplicated for a possible
/// future failure.
pub(crate) struct OperationalPath(PathBuf);

impl OperationalPath {
    /// Retain one root-relative path as failure evidence.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// The root-relative path.
    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

/// The forward-slash UTF-8 root-relative spelling of a selected source file. Not
/// `Clone`: the spelling is moved into the `CapturedFile` handed to the pure
/// owner.
pub(crate) struct SourceSpelling(String);

impl SourceSpelling {
    /// Retain one forward-slash UTF-8 root-relative spelling.
    pub(crate) fn new(spelling: String) -> Self {
        Self(spelling)
    }

    /// The forward-slash spelling.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the owner, yielding the spelling for the pure `CapturedFile`.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// The current-behavior path-budget seam. In this baseline it tracks nothing and
/// never refuses; the target adapter replaces it with the checked live/aggregate
/// native-path budget and non-`Clone` leases.
pub(crate) struct PathBudget {
    _private: (),
}

impl PathBudget {
    /// A fresh budget. The caller-root and canonical-root charges the target law
    /// requires are absent in this baseline.
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Charge native-path work. In this baseline the charge always succeeds and
    /// is discarded; the target adapter makes this fallible and checked.
    pub(crate) fn charge(&mut self, _units: usize) {}
}
