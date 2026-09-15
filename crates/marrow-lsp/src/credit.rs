//! Affine capacity credits: move-only tokens that bound concurrent server work.
//!
//! A credit is non-`Clone` and `#[must_use]`, so work that needs one cannot begin
//! without acquiring it and cannot run twice on the same token.
//!
//! - [`OutboundCredit`]: exactly [`OUTBOUND_CREDITS`] exist. Every response, error,
//!   null-id protocol frame, `showMessage`, and diagnostic frame acquires one before it
//!   is handed to the writer, and it returns only when the writer's delivery receipt is
//!   consumed — so no more than that many frames are ever outstanding toward the writer.
//! - [`PublicationPlanCredit`]: exactly one, held by the coordinator in an `Option` from
//!   plan construction through the final delivery receipt, so the delivered ledger
//!   cannot drift under a precomputed union.
//!
//! Single-analysis serialization and the retained-snapshot count are owned state rather
//! than credits: the coordinator's `worker_busy` flag over its cap-one work channel, and
//! its current ready result plus the worker's arriving result.

use crate::capacities::OUTBOUND_CREDITS;

/// One outbound-frame credit. Non-`Clone`: acquired before a frame is handed to the
/// writer, and returned only when the writer's delivery receipt is consumed.
#[must_use]
pub(crate) struct OutboundCredit(());

/// The single exclusive publication-plan credit. Non-`Clone`: held for the whole life
/// of one analysis publication set, including a resource-stop notice and retractions.
#[must_use]
pub(crate) struct PublicationPlanCredit(());

impl PublicationPlanCredit {
    /// The one credit, minted once into the coordinator's slot at startup.
    pub(crate) fn mint() -> Self {
        Self(())
    }
}

/// The outbound credits not currently outstanding. The credit is a zero-sized token, so
/// the pool is the count of how many remain: it hands out at most [`OUTBOUND_CREDITS`]
/// and can never hold back more than it minted.
pub(crate) struct OutboundCredits {
    available: usize,
}

impl OutboundCredits {
    pub(crate) fn new() -> Self {
        Self {
            available: OUTBOUND_CREDITS,
        }
    }

    /// Acquire one credit, or `None` when all are outstanding.
    pub(crate) fn acquire(&mut self) -> Option<OutboundCredit> {
        self.available = self.available.checked_sub(1)?;
        Some(OutboundCredit(()))
    }

    /// Return a credit. A credit exists only because this pool minted it.
    pub(crate) fn release(&mut self, credit: OutboundCredit) {
        let OutboundCredit(()) = credit;
        debug_assert!(
            self.available < OUTBOUND_CREDITS,
            "returned more credits than the pool minted"
        );
        self.available += 1;
    }

    /// The number of credits currently available.
    pub(crate) fn available(&self) -> usize {
        self.available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_pool_mints_exactly_its_capacity() {
        let mut pool = OutboundCredits::new();
        let mut held = Vec::new();
        for _ in 0..OUTBOUND_CREDITS {
            held.push(pool.acquire().expect("credit within capacity"));
        }
        assert!(
            pool.acquire().is_none(),
            "credits are exhausted at capacity"
        );
        for credit in held {
            pool.release(credit);
        }
        assert_eq!(pool.available(), OUTBOUND_CREDITS);
    }
}
