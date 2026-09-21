//! The per-capture native-path budget and the lease that returns a live charge.
//!
//! `PathBudget` owns one live retained counter and one monotone work counter. A
//! `reserve` charges both and mints a [`PathLease`] that the path's owner holds
//! beside the `PathBuf`; the lease returns its live charge when it drops. Work
//! never decreases. The counter is shared through an `Rc`, so a lease cannot
//! leave the capturing thread.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

/// The unit count of a native path: its platform `OsStr` byte length. On the
/// required Linux/macOS targets this is the exact `OsStrExt::as_bytes().len()`; no
/// character count or lossy conversion substitutes for it.
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

/// A live retained-path charge, returned to the budget when it drops.
pub(crate) struct PathLease {
    retained: Rc<Cell<usize>>,
    units: usize,
}

impl Drop for PathLease {
    fn drop(&mut self) {
        // The charge was added by the budget that minted this lease, so it is
        // always present to subtract.
        self.retained.set(self.retained.get() - self.units);
    }
}

/// The one per-capture native-path budget: a live retained counter and a monotone
/// work counter.
pub(crate) struct PathBudget {
    retained: Rc<Cell<usize>>,
    work: usize,
}

impl PathBudget {
    pub(crate) fn new() -> Self {
        Self {
            retained: Rc::new(Cell::new(0)),
            work: 0,
        }
    }

    /// The committed monotone work total.
    pub(crate) fn work(&self) -> usize {
        self.work
    }

    /// The current live retained total.
    pub(crate) fn retained(&self) -> usize {
        self.retained.get()
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

    /// Reserve one live retained native path plus its monotone work. Retained wins
    /// when both bounds would be exceeded; neither counter moves on a refusal.
    pub(crate) fn reserve(
        &mut self,
        units: usize,
        retained_limit: usize,
        work_limit: usize,
    ) -> Result<PathLease, ReserveError> {
        let prospective_work = self.work.checked_add(units).ok_or(ReserveError::Overflow)?;
        let prospective_live = self
            .retained
            .get()
            .checked_add(units)
            .ok_or(ReserveError::Overflow)?;
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
        self.work = prospective_work;
        Ok(self.mint(units, prospective_live))
    }

    /// Reserve one live retained path without touching work — the reservation an
    /// atomic directory batch makes per staged carrier after it has committed the
    /// batch's aggregate work once.
    pub(crate) fn reserve_live(
        &mut self,
        units: usize,
        retained_limit: usize,
    ) -> Result<PathLease, ReserveError> {
        let prospective_live = self
            .retained
            .get()
            .checked_add(units)
            .ok_or(ReserveError::Overflow)?;
        if prospective_live > retained_limit {
            return Err(ReserveError::Retained {
                limit: retained_limit,
                actual: prospective_live,
            });
        }
        Ok(self.mint(units, prospective_live))
    }

    /// Commit monotone work once for a settled atomic batch's aggregate. This is the
    /// batch's single work commit site.
    pub(crate) fn commit_work(&mut self, units: usize) -> Result<(), ReserveError> {
        self.work = self.work.checked_add(units).ok_or(ReserveError::Overflow)?;
        Ok(())
    }

    fn mint(&self, units: usize, live: usize) -> PathLease {
        self.retained.set(live);
        PathLease {
            retained: Rc::clone(&self.retained),
            units,
        }
    }
}
