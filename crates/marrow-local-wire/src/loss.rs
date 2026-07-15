//! The closed loss classification for a call whose reply never arrived.
//!
//! When the runner process dies, the client's verdict about an outstanding call is
//! one of exactly three classes. The class is a function of how far the request had
//! progressed at the boundary the peer died on — not of any reply, which by
//! definition did not arrive — so a supervisor that tracks the [`HandoffStage`] of
//! each call derives its class deterministically. No class is ever retried: a lost
//! reply is reported, never replayed, because a mutating call whose outcome is
//! unknown must not run twice.

/// A caller's verdict about a call whose reply was lost to peer death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossClass {
    /// The call provably never ran: the connection or handshake failed, or the
    /// request had not yet been sent, before the peer died. Safe to consider
    /// undone, but still never silently replayed.
    NotStarted,
    /// The request was accepted by the supervisor but the worker died before it
    /// began executing the call. The call did not start; it is reported, not
    /// resubmitted.
    Interrupted,
    /// The request had been dispatched to the worker when the peer died, so the
    /// call may have run — wholly or partly — and its outcome is unknowable from
    /// this side. Never replayed.
    OutcomeUnknown,
}

/// How far a request had progressed when the peer died. A supervisor advances a
/// call through these stages and reads its [`LossClass`] from the last one reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffStage {
    /// The request has not been written to the worker (including a connection or
    /// handshake that never completed).
    BeforeSend,
    /// The request was admitted to the supervisor's bounded queue but not yet
    /// handed to the serial worker.
    Queued,
    /// The request was handed to the serial worker.
    Dispatched,
}

/// The loss class for a peer death at `stage`.
pub const fn classify(stage: HandoffStage) -> LossClass {
    match stage {
        HandoffStage::BeforeSend => LossClass::NotStarted,
        HandoffStage::Queued => LossClass::Interrupted,
        HandoffStage::Dispatched => LossClass::OutcomeUnknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{HandoffStage, LossClass, classify};

    #[test]
    fn each_stage_maps_to_its_class() {
        assert_eq!(classify(HandoffStage::BeforeSend), LossClass::NotStarted);
        assert_eq!(classify(HandoffStage::Queued), LossClass::Interrupted);
        assert_eq!(
            classify(HandoffStage::Dispatched),
            LossClass::OutcomeUnknown
        );
    }
}
