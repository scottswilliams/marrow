//! The poison-latch reopen path.
//!
//! An indeterminate commit poisons the in-memory store handle, and the kernel then refuses
//! every further session open on it with `SessionError::Poisoned` — its state is unknown until
//! reclassified. The recovery the refusal points to is here: close the poisoned handle and
//! reopen the store fresh (a new handle starts unpoisoned), then classify the interrupted
//! commit by reading the witness cell — complete-new when the intended witness token landed,
//! complete-old when it did not. Nothing is ever retried; the reopen classifies, and the
//! caller resumes from the confirmed state.

use std::path::Path;

use marrow_kernel::durable::{Reopen, SiteSpec, StoreSchema};

use crate::provision::{OpenError, open};

/// Reopen the store at `dir` and classify an interrupted commit identified by `token`. A fresh
/// open yields an unpoisoned handle; [`classify`](marrow_kernel::durable::NativeStore::classify)
/// reads the witness cell and reports [`Reopen::CompleteNew`] when the commit's witness landed
/// or [`Reopen::CompleteOld`] when it did not. This is the recovery the kernel's poisoned-handle
/// refusal directs to — a reclassification, never a replay.
pub fn reopen_and_classify(
    dir: &Path,
    token: [u8; 16],
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
) -> Result<Reopen, OpenError> {
    let opened = open(dir, schemas, sites)?;
    opened.store.classify(token).map_err(OpenError::Store)
}
