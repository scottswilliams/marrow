//! Affine capacity credits: move-only tokens that bound concurrent server work.
//!
//! Each credit type is non-`Clone` and `#[must_use]`. A fixed number exists; a holder
//! moves a credit through its states and it is destroyed (not reissued) at a terminal
//! outcome. The credits are the type-level enforcement of the affine-topology and
//! retained-capacity laws: work that needs a credit cannot begin without acquiring one,
//! and the pools mint exactly the frozen counts.
//!
//! - [`WorkerCredit`]: exactly one. Permits ready-snapshot service, attaching a waiter
//!   to the active revision, or starting one analysis. No per-request compile queue
//!   exists — the single credit serializes analysis.
//! - [`OutboundCredit`]: exactly [`OUTBOUND_CREDITS`] (`W`). Every response, error,
//!   null-id protocol frame, and `showMessage` acquires one before construction.
//! - [`SnapshotCredit`]: exactly [`MAX_RETAINED_SNAPSHOTS`]. Bounds the distinct
//!   revision-owned snapshot records retained at once.
//! - [`PublicationPlanCredit`]: exactly one. Held from plan construction through the
//!   final delivery receipt so the delivered ledger cannot drift under a precomputed
//!   union.

use crate::capacities::{MAX_RETAINED_SNAPSHOTS, OUTBOUND_CREDITS};

/// The single analysis-worker credit. Non-`Clone`: only its holder may drive analysis.
#[must_use]
pub struct WorkerCredit(());

/// One outbound-frame credit. Non-`Clone`: acquired before a frame is constructed,
/// counted, or encoded, and returned only when the writer's delivery receipt is
/// consumed.
#[must_use]
pub struct OutboundCredit(());

/// One retained-snapshot credit. Non-`Clone`: bounds distinct retained revisions.
#[must_use]
pub struct SnapshotCredit(());

/// The single exclusive publication-plan credit. Non-`Clone`: held for the whole life
/// of one diagnostic publication set.
#[must_use]
pub struct PublicationPlanCredit(());

/// A fixed-capacity pool of move-only credits. It mints exactly `capacity` tokens over
/// its whole life: acquisition removes one, release returns one, and the pool never
/// exceeds its capacity.
pub struct CreditPool<T> {
    available: Vec<T>,
    capacity: usize,
}

impl<T> CreditPool<T> {
    fn new(mint: impl Fn() -> T, capacity: usize) -> Self {
        let mut available = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            available.push(mint());
        }
        Self {
            available,
            capacity,
        }
    }

    /// Acquire one credit, or `None` when all are outstanding.
    pub fn acquire(&mut self) -> Option<T> {
        self.available.pop()
    }

    /// Return a credit to the pool. A credit can only exist if it came from a pool of
    /// this type, and the pool never holds more than its capacity.
    pub fn release(&mut self, credit: T) {
        debug_assert!(
            self.available.len() < self.capacity,
            "returned more credits than the pool minted"
        );
        self.available.push(credit);
    }

    /// The number of credits currently available.
    pub fn available(&self) -> usize {
        self.available.len()
    }

    /// The fixed capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl CreditPool<WorkerCredit> {
    /// The single-credit worker pool.
    pub fn worker() -> Self {
        Self::new(|| WorkerCredit(()), 1)
    }
}

impl CreditPool<OutboundCredit> {
    /// The `W`-credit outbound pool.
    pub fn outbound() -> Self {
        Self::new(|| OutboundCredit(()), OUTBOUND_CREDITS)
    }
}

impl CreditPool<SnapshotCredit> {
    /// The retained-snapshot pool.
    pub fn snapshot() -> Self {
        Self::new(|| SnapshotCredit(()), MAX_RETAINED_SNAPSHOTS)
    }
}

impl CreditPool<PublicationPlanCredit> {
    /// The single publication-plan pool.
    pub fn publication() -> Self {
        Self::new(|| PublicationPlanCredit(()), 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_pool_has_exactly_one_credit() {
        let mut pool = CreditPool::worker();
        assert_eq!(pool.capacity(), 1);
        let credit = pool.acquire().expect("first acquire");
        assert!(pool.acquire().is_none(), "no second worker credit exists");
        pool.release(credit);
        assert_eq!(pool.available(), 1);
    }

    #[test]
    fn outbound_pool_mints_exactly_w_credits() {
        let mut pool = CreditPool::outbound();
        assert_eq!(pool.capacity(), OUTBOUND_CREDITS);
        let mut held = Vec::new();
        for _ in 0..OUTBOUND_CREDITS {
            held.push(pool.acquire().expect("credit within capacity"));
        }
        assert!(pool.acquire().is_none(), "credits are exhausted at W");
        for credit in held {
            pool.release(credit);
        }
        assert_eq!(pool.available(), OUTBOUND_CREDITS);
    }

    #[test]
    fn snapshot_pool_bounds_retained_revisions() {
        let mut pool = CreditPool::snapshot();
        assert_eq!(pool.capacity(), MAX_RETAINED_SNAPSHOTS);
        let mut held = Vec::new();
        for _ in 0..MAX_RETAINED_SNAPSHOTS {
            held.push(pool.acquire().unwrap());
        }
        assert!(pool.acquire().is_none());
        drop(held);
    }

    #[test]
    fn publication_pool_is_exclusive() {
        let mut pool = CreditPool::publication();
        let credit = pool.acquire().unwrap();
        assert!(pool.acquire().is_none(), "publication credit is exclusive");
        pool.release(credit);
    }
}
