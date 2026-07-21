//! The session-opening seam shared by the two production attachment kinds.
//!
//! An export invocation opens exactly one session bounded by `demand ∩ ceiling ∩ grant`.
//! Both the ephemeral-memory attachment (E01) and the persistent native store (F02) open
//! that session the same way, so the executor drives an export against either kind through
//! one generic path rather than a duplicated read/write branch per attachment kind. This is
//! the "same session machinery over a durable engine" the native attachment reuses.

use marrow_store::ByteEngine;

use super::attach::EphemeralAttachment;
use super::store::{DurableStore, ReadSession, TxnSession};
use super::{DemandCoverage, InvocationGrant, SessionError};

/// A live attachment that opens a read or transaction session for one invocation after
/// resolving effective authority. Implemented by the ephemeral-memory attachment and by the
/// native durable store, so one executor path serves both.
pub trait SessionHost {
    /// The ordered-byte engine this host's sessions run over.
    type Engine: ByteEngine;

    /// Open a read session for a read-only invocation.
    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError>;

    /// Open a transaction session for a mutating invocation.
    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError>;
}

impl SessionHost for EphemeralAttachment {
    type Engine = marrow_store::MemoryEngine;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        EphemeralAttachment::read_session(self, grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        EphemeralAttachment::txn_session(self, grant, demand)
    }
}

impl<E: ByteEngine> SessionHost for DurableStore<E> {
    type Engine = E;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        DurableStore::read_session(self, grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        DurableStore::txn_session(self, grant, demand)
    }
}

/// A boxed host is itself a host, forwarding through the box. `marrow_vm::mint_ephemeral`
/// hands the caller a `Box<EphemeralAttachment>` (the attachment owns a whole store schema and
/// is far larger than the other variants), so this lets it drive `run_export` without an
/// explicit reborrow.
impl<H: SessionHost> SessionHost for Box<H> {
    type Engine = H::Engine;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        (**self).read_session(grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        (**self).txn_session(grant, demand)
    }
}
