//! The read-only publication-marker probe every front door passes through.
//!
//! The probe opens nothing, creates nothing, takes no lock, and requires no
//! qualified platform: it is two no-follow stats of fixed names. That matters
//! because it runs on every capture, including the language server's, where a
//! probe that admitted directories or took a lock would turn a read into a
//! write.

use std::io;
use std::path::Path;

use super::{LEDGER_NAME, META_DIR};

/// The two fixed marker spellings the pending journal derives from the ledger's
/// entry name. They are spelled here rather than derived through
/// [`marrow_fs_journal::PendingName`] because the probe must not admit a
/// directory to ask the question; the layout test pins the two spellings
/// against that derivation so they cannot drift apart.
const CLAIM_SUFFIX: &str = ".pending.create";
const PENDING_SUFFIX: &str = ".pending";

/// A live `.marrow/ids` publication marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdsPublicationMarker {
    /// `.marrow/ids.pending` exists: a publication was durably claimed. The
    /// committed ledger is whichever generation recovery settles on.
    Claimed,
    /// Only `.marrow/ids.pending.create` exists: a publication was created and
    /// never durably claimed. The ledger is untouched, but the entry is
    /// retained rather than swept, so it keeps gating until an operator removes
    /// it.
    Unclaimed,
}

/// Probe the project rooted at `root`, failing closed.
pub(super) fn probe(root: &Path) -> Option<IdsPublicationMarker> {
    let meta = root.join(META_DIR);
    if present(&meta.join(format!("{LEDGER_NAME}{PENDING_SUFFIX}"))) {
        return Some(IdsPublicationMarker::Claimed);
    }
    if present(&meta.join(format!("{LEDGER_NAME}{CLAIM_SUFFIX}"))) {
        return Some(IdsPublicationMarker::Unclaimed);
    }
    None
}

/// Whether `path` names something. Only a definite `NotFound` counts as
/// absence: an entry the probe cannot classify keeps the gate closed rather
/// than admitting a read of an indeterminate ledger.
fn present(path: &Path) -> bool {
    !matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
}
