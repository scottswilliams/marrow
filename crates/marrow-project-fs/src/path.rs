//! The per-capture native-path budget, its non-`Clone` active/terminal leases, and
//! the live path/spelling owners.
//!
//! `PathBudget` owns one shared retained (live) counter and one monotone work
//! counter. A `reserve` mints an [`ActiveLease`] the caller's live owner holds; the
//! lease releases its live charge when it drops, unless it is first consumed into a
//! [`TerminalLease`] that a failure holds — so a refusal keeps its live charge until
//! the failure drops. Work never decreases. The active lease is non-`Send` and can
//! live only in `CanonicalRoot`, `DirectoryAdmission`, `DirectoryFrame`,
//! `LiveNativePath`, or `SourceSpelling`; the terminal lease and the terminal
//! [`OperationalPath`] admitted by a failure are `Send + Sync`.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The unit count of a native path: its platform `OsStr` byte length. On the
/// required Linux/macOS targets this is the exact `OsStrExt::as_bytes().len()`; no
/// character count or lossy conversion substitutes for it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn native_units(path: &Path) -> usize {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len()
}

/// A path-budget refusal. The caller maps it to the role-specific pathless
/// `Bound`/`Io(OutOfMemory)` tuple.
#[derive(Debug)]
pub(crate) enum ReserveError {
    /// The prospective live retained total would exceed its limit.
    Retained { limit: usize, actual: usize },
    /// The prospective monotone work total would exceed its limit.
    Work { limit: usize, actual: usize },
    /// A checked add overflowed.
    Overflow,
}

/// The shared retained-counter charge one lease owns.
struct LeaseCore {
    retained: Arc<AtomicUsize>,
    charge: usize,
}

impl LeaseCore {
    /// Release the charge with a checked update that never wraps.
    fn release(&self) {
        let _ = self
            .retained
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_sub(self.charge)
            });
    }
}

/// A live retained-path charge. Non-`Send` and non-`Clone`: it lives only in the
/// adapter's live owners and releases its charge on drop, unless first consumed into
/// a terminal lease.
pub(crate) struct ActiveLease {
    core: Option<LeaseCore>,
    _not_send: PhantomData<*const ()>,
}

impl ActiveLease {
    /// Consume the active lease into a terminal lease, transferring its exact charge
    /// without releasing it; the terminal lease releases it later.
    fn into_terminal(mut self) -> TerminalLease {
        let core = self
            .core
            .take()
            .expect("an active lease holds its charge until consumed");
        TerminalLease { core: Some(core) }
    }
}

impl Drop for ActiveLease {
    fn drop(&mut self) {
        if let Some(core) = self.core.take() {
            core.release();
        }
    }
}

/// A terminal retained-path charge held in failure evidence. `Send + Sync`: it
/// releases its charge only when the failure drops.
pub(crate) struct TerminalLease {
    core: Option<LeaseCore>,
}

impl Drop for TerminalLease {
    fn drop(&mut self) {
        if let Some(core) = self.core.take() {
            core.release();
        }
    }
}

/// The one per-capture native-path budget: a shared retained (live) counter and a
/// monotone work counter. It is not poisonable and not cloneable.
pub(crate) struct PathBudget {
    retained: Arc<AtomicUsize>,
    work: usize,
}

impl PathBudget {
    pub(crate) fn new() -> Self {
        Self {
            retained: Arc::new(AtomicUsize::new(0)),
            work: 0,
        }
    }

    /// The committed monotone work total.
    pub(crate) fn work(&self) -> usize {
        self.work
    }

    /// The current live retained total.
    pub(crate) fn retained(&self) -> usize {
        self.retained.load(Ordering::Relaxed)
    }

    /// Charge monotone work only — the caller-root spelling, before canonicalization.
    /// No live charge and no lease: work never releases.
    pub(crate) fn charge_work(
        &mut self,
        units: usize,
        work_limit: usize,
    ) -> Result<(), ReserveError> {
        let prospective = self.work.checked_add(units).ok_or(ReserveError::Overflow)?;
        if prospective > work_limit {
            return Err(ReserveError::Work {
                limit: work_limit,
                actual: prospective,
            });
        }
        self.work = prospective;
        Ok(())
    }

    /// Reserve one live retained native path plus its monotone work, returning the
    /// active lease the owner holds. Retained wins when both bounds would be
    /// exceeded; work commits only after the live commit succeeds.
    pub(crate) fn reserve(
        &mut self,
        units: usize,
        retained_limit: usize,
        work_limit: usize,
    ) -> Result<ActiveLease, ReserveError> {
        let prospective_work = self.work.checked_add(units).ok_or(ReserveError::Overflow)?;
        let snapshot = self.retained.load(Ordering::Relaxed);
        let prospective_live = snapshot.checked_add(units).ok_or(ReserveError::Overflow)?;
        if prospective_live > retained_limit {
            return Err(ReserveError::Retained {
                limit: retained_limit,
                actual: prospective_live,
            });
        }
        if prospective_work > work_limit {
            return Err(ReserveError::Work {
                limit: work_limit,
                actual: prospective_work,
            });
        }
        loop {
            let current = self.retained.load(Ordering::Relaxed);
            let next = current.checked_add(units).ok_or(ReserveError::Overflow)?;
            if next > retained_limit {
                return Err(ReserveError::Retained {
                    limit: retained_limit,
                    actual: next,
                });
            }
            if self
                .retained
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        self.work = prospective_work;
        Ok(self.mint_lease(units))
    }

    /// Reserve one live retained path without touching work — the private
    /// reservation an atomic directory batch uses per staged carrier after it has
    /// committed the batch's aggregate work once. The retained bound is rechecked in
    /// the checked compare-exchange loop.
    pub(crate) fn reserve_live(
        &mut self,
        units: usize,
        retained_limit: usize,
    ) -> Result<ActiveLease, ReserveError> {
        loop {
            let current = self.retained.load(Ordering::Relaxed);
            let next = current.checked_add(units).ok_or(ReserveError::Overflow)?;
            if next > retained_limit {
                return Err(ReserveError::Retained {
                    limit: retained_limit,
                    actual: next,
                });
            }
            if self
                .retained
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        Ok(self.mint_lease(units))
    }

    /// Commit monotone work once for a settled atomic batch's aggregate. This is the
    /// batch's single work commit site.
    pub(crate) fn commit_work(&mut self, units: usize) -> Result<(), ReserveError> {
        self.work = self.work.checked_add(units).ok_or(ReserveError::Overflow)?;
        Ok(())
    }

    fn mint_lease(&self, units: usize) -> ActiveLease {
        ActiveLease {
            core: Some(LeaseCore {
                retained: Arc::clone(&self.retained),
                charge: units,
            }),
            _not_send: PhantomData,
        }
    }
}

/// The canonical physical root, charged to both counters, holding its active lease.
pub(crate) struct CanonicalRoot {
    path: PathBuf,
    _lease: ActiveLease,
}

impl CanonicalRoot {
    pub(crate) fn new(path: PathBuf, lease: ActiveLease) -> Self {
        Self {
            path,
            _lease: lease,
        }
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }
}

/// A live retained native path, holding its active lease. Non-`Send`.
pub(crate) struct LiveNativePath {
    path: PathBuf,
    lease: ActiveLease,
}

impl LiveNativePath {
    pub(crate) fn new(path: PathBuf, lease: ActiveLease) -> Self {
        Self { path, lease }
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }

    /// Transfer this live path into terminal failure evidence: its charge moves into
    /// the terminal lease and releases only when the failure drops.
    pub(crate) fn into_operational(self) -> OperationalPath {
        OperationalPath {
            path: self.path,
            _lease: Some(self.lease.into_terminal()),
        }
    }
}

impl LiveNativePath {
    /// Turn this live path into a captured-source guard whose native-path lease stays
    /// live across the synchronous pure-capture call.
    pub(crate) fn into_guard(self) -> SourceGuard {
        SourceGuard { _live: self }
    }
}

/// One live guard for a captured source: its native-path lease stays live across the
/// synchronous pure-capture call, one guard per file. Non-`Send`.
pub(crate) struct SourceGuard {
    _live: LiveNativePath,
}

/// Owned root-relative path evidence carried by a failure. `Send + Sync`: the
/// optional terminal lease releases its live charge only when the failure drops. A
/// fixed-role or pre-lease pathless spelling carries no lease.
pub(crate) struct OperationalPath {
    path: PathBuf,
    _lease: Option<TerminalLease>,
}

impl OperationalPath {
    /// An unleased operational path: a fixed-role literal joined by the facade, or a
    /// path not drawn from the live budget.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, _lease: None }
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }
}

// The transferable failure evidence and the terminal lease are `Send + Sync`; the
// active lease and live owners are not (they carry `PhantomData<*const ()>`).
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<OperationalPath>();
    assert_send_sync_static::<TerminalLease>();
};
