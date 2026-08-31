//! The generic-owner composite guard: one admitted batch over the compiler's type
//! registry and the image draft, and the inverse that restores both.

use marrow_image::{DraftTxn, ImageDraft};

use super::{ArgumentDomain, DiagnosticCollector, GenericInvariant, TypeRegistry};

/// The state one admitted generic-owner batch must restore, captured before the batch's
/// first mutation and consumed by [`TypeRegistry::restore_generic_owners`].
///
/// Every entry is a length, a marker, or a whole swapped owner, so restoration allocates
/// nothing, indexes nothing, and cannot fail. The append-only instantiation owners are
/// described by their lengths alone because the settled prefix is immutable across a
/// batch: a dependency edge is recorded only for a row of the active fill, settlement
/// clears the active suffix, and admission proves no fill is open, so every row a batch
/// can reach lies at or above the length captured here.
#[must_use = "an admitted generic-owner batch must restore or disarm its inverse"]
pub(super) struct RegistryInverse {
    pub(super) type_insts: usize,
    pub(super) collections: usize,
    pub(super) fn_insts: usize,
    pub(super) fn_queue: usize,
    /// The reserved image function base. It is ordinary registry state rather than
    /// diagnostic custody, so a batch that moves it restores it.
    pub(super) fn_base: u16,
    pub(super) build_invariant: Option<GenericInvariant>,
    pub(super) prior_argument_domain: ArgumentDomain,
    pub(super) entry_records: usize,
    pub(super) entry_enums: usize,
    /// Whether the reused metadata row directory existed at admission. Rewinding an
    /// extant directory to its captured ceilings is not the inverse of *creating* one:
    /// a batch that opened the first directory must leave the registry with none.
    pub(super) row_directory_present: bool,
    /// Present only for an isolated template proof: the owners the proof runs without,
    /// swapped whole out of the registry at admission and re-seated whole when the
    /// proof's effects are erased. An ordinary batch swaps nothing — the shared
    /// instantiation-limit owner and the ordered diagnostic buffer belong to the
    /// diagnostic substrate's custody, never to this inverse.
    pub(super) isolation: Option<ProofIsolation>,
}

/// The two `Monomorph` owners this inverse deliberately does not restore, and why.
///
/// `limit` and `collection_payloads` are the instantiation-limit terminal and the
/// ordered collection-payload diagnostic buffer. Both are diagnostic payload, which the
/// phase places exclusively in the predecessor substrate's custody: `DraftTxn` never
/// owns, copies, journals, or exposes it, and this inverse mirrors that boundary rather
/// than opening a second custody for the same rows.
///
/// That exclusion is only sound because something else decides visibility: the staged-body
/// guard owns this guard and its still-private diagnostic and fact payloads together. A
/// batch that ends in an invariant drops that aggregate and publishes nothing regardless
/// of what these two owners hold. Journaling them here as well would be a second custody
/// over rows this inverse is not entitled to own.
///
/// An isolated template proof is the one exception, and it is not a restoration of
/// these owners either: it *swaps them out* at admission so the throwaway pass cannot
/// reach the live ones at all, then puts the live owners back whole.
///
/// This is stated here because [`TypeRegistry::admit_generic_owners`] destructures
/// `Monomorph` exhaustively: a new owner cannot be added without a decision recorded at
/// one of these two places.
#[cfg(test)]
pub(super) const UNRESTORED_DIAGNOSTIC_OWNERS: [&str; 2] = ["limit", "collection_payloads"];

/// The live owners an isolated template proof runs without.
pub(super) struct ProofIsolation {
    pub(super) prior_payloads: DiagnosticCollector,
}

/// The generic-owner composite guard: one admitted batch over the real registry (held
/// by exclusive `&mut`) and the real draft (held through an armed [`DraftTxn`]).
///
/// Holding the registry exclusively is what makes the guard's restoration
/// borrow-panic-free: `Drop` has `&mut self` and so reaches every interior owner
/// through `RefCell::get_mut`, with no runtime borrow flag to conflict with. It is also
/// what makes a metadata session or interior borrow derived inside the batch die before
/// the guard can drop — a guard dropped under a live session is a compile error rather
/// than an unwind abort.
///
/// Both owners are restored on **every** armed exit — an ordinary refusal, an early
/// invariant, or an unwind — registry inverse first, then the still-armed draft guard,
/// exactly once. [`Self::commit`] is the only path that keeps a batch's effects; a
/// template proof is throwaway and never takes it.
pub(crate) struct GenericOwnerTxn<'r, 'd> {
    registry: &'r mut TypeRegistry,
    inverse: Option<RegistryInverse>,
    /// The armed draft transaction. Dropping it is the whole draft restoration; the
    /// guard's own `Drop` body restores the registry inverse before taking it.
    draft: Option<DraftTxn<'d>>,
}

impl<'r, 'd> GenericOwnerTxn<'r, 'd> {
    /// Admit an ordinary generic-owner batch: every type, collection, and function
    /// instantiation the batch mints becomes one failure-atomic unit with the draft
    /// rows it reserves. An unsettled registry refuses with both owners untouched.
    pub(crate) fn begin(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        let inverse =
            registry.admit_generic_owners(draft.record_type_count(), draft.enum_type_count())?;
        Ok(Self::armed(registry, draft, inverse))
    }

    /// Admit an isolated template proof on a settled registry, taking the proof's
    /// swapped owners on top of the ordinary batch inverse.
    pub(crate) fn enter_proof(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        let inverse =
            registry.enter_template_proof(draft.record_type_count(), draft.enum_type_count())?;
        Ok(Self::armed(registry, draft, inverse))
    }

    fn armed(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
        inverse: RegistryInverse,
    ) -> Self {
        let savepoint = draft.savepoint();
        #[expect(
            clippy::expect_used,
            reason = "admission law: the savepoint was just minted from this unarmed owner"
        )]
        let txn = draft
            .begin_transaction(savepoint)
            .expect("a fresh savepoint admits the batch");
        Self {
            registry,
            inverse: Some(inverse),
            draft: Some(txn),
        }
    }

    /// The batch body's split borrows: the registry and the armed draft transaction,
    /// reborrowed from disjoint guard fields, both exclusive.
    pub(crate) fn parts(&mut self) -> (&mut TypeRegistry, &mut DraftTxn<'d>) {
        #[expect(
            clippy::expect_used,
            reason = "guard law: the armed transaction is taken exactly once, by commit or Drop"
        )]
        (
            self.registry,
            self.draft
                .as_mut()
                .expect("the guard holds its armed transaction until it drops"),
        )
    }

    /// The registry, for reads that must precede the guard's drop.
    pub(crate) fn registry(&self) -> &TypeRegistry {
        &*self.registry
    }

    /// Keep this batch: the draft transaction commits first, then the registry inverse
    /// is disarmed, so no path can retain draft rows whose registry rows were erased.
    pub(crate) fn commit(mut self) {
        #[expect(
            clippy::expect_used,
            reason = "guard law: commit consumes the one armed draft transaction"
        )]
        self.draft
            .take()
            .expect("a live batch holds its armed draft transaction")
            .commit();
        self.inverse = None;
    }

    /// Erase this batch now — registry inverse first, then the armed draft guard's own
    /// inverse.
    ///
    /// This is the explicit spelling of the isolated template proof's exit, whose product
    /// is its diagnostics and its editor facts rather than the throwaway image work it
    /// emits. Dropping the producer-owning aggregate restores exactly the same owners and
    /// drops its still-private payloads. The order matches `Drop`'s: a path may not retain
    /// draft rows whose registry rows were erased.
    pub(crate) fn erase(mut self) {
        if let Some(inverse) = self.inverse.take() {
            self.registry.restore_generic_owners(inverse);
        }
        #[expect(
            clippy::expect_used,
            reason = "guard law: erase consumes the one armed draft transaction"
        )]
        self.draft
            .take()
            .expect("a live batch holds its armed draft transaction")
            .rollback();
    }
}

impl Drop for GenericOwnerTxn<'_, '_> {
    /// Restore the compiler registry's inverse first, then take and drop the
    /// still-armed draft guard exactly once — the drop-order law.
    fn drop(&mut self) {
        if let Some(inverse) = self.inverse.take() {
            self.registry.restore_generic_owners(inverse);
        }
        drop(self.draft.take());
    }
}
// drop-path audit sentinel: end of GenericOwnerTxn::drop
