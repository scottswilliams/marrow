//! Affine capacity credits: move-only tokens that bound concurrent server work.
//!
//! Each credit type is non-`Clone` and `#[must_use]`. A fixed number exists; a holder
//! moves a credit through its states and it is destroyed (not reissued) at a terminal
//! outcome. The credits are the type-level enforcement of the outbound and publication
//! bounds: work that needs a credit cannot begin without acquiring one, and the pools
//! mint exactly the frozen counts.
//!
//! - [`OutboundCredit`]: exactly [`OUTBOUND_CREDITS`] (`W`). Every response, error,
//!   null-id protocol frame, `showMessage`, and diagnostic frame acquires one before it
//!   is handed to the writer, and it returns only when the writer's delivery receipt is
//!   consumed — so no more than `W` frames are ever outstanding toward the writer.
//! - [`PublicationPlanCredit`]: exactly one. Held from plan construction through the
//!   final delivery receipt so the delivered ledger cannot drift under a precomputed
//!   union.
//!
//! Two other bounds the design frames as credits are enforced by simpler owned state and
//! are not credit types here: single-analysis serialization is the coordinator's
//! `worker_busy` flag over its cap-one work channel, and retained-snapshot count is the
//! coordinator's current-plus-pending snapshot `Option`s (at most
//! [`MAX_RETAINED_SNAPSHOTS`]).

use crate::capacities::OUTBOUND_CREDITS;

/// One outbound-frame credit. Non-`Clone`: acquired before a frame is constructed,
/// counted, or encoded, and returned only when the writer's delivery receipt is
/// consumed.
#[must_use]
pub struct OutboundCredit(());

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
    #[cfg(test)]
    pub fn available(&self) -> usize {
        self.available.len()
    }

    /// The fixed capacity.
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl CreditPool<OutboundCredit> {
    /// The `W`-credit outbound pool.
    pub fn outbound() -> Self {
        Self::new(|| OutboundCredit(()), OUTBOUND_CREDITS)
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
    fn publication_pool_is_exclusive() {
        let mut pool = CreditPool::publication();
        let credit = pool.acquire().unwrap();
        assert!(pool.acquire().is_none(), "publication credit is exclusive");
        pool.release(credit);
    }
}
