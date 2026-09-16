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
    pub(super) build_invariant: Option<GenericInvariant>,
    pub(super) prior_argument_domain: ArgumentDomain,
    pub(super) entry_records: usize,
    pub(super) entry_enums: usize,
    /// Whether the reused metadata row directory existed at admission. Rewinding an
    /// extant directory to its captured ceilings is not the inverse of *creating* one:
    /// a batch that opened the first directory must leave the registry with none.
    pub(super) row_directory_present: bool,
    /// Present only for an isolated template proof; see [`ProofIsolation`]. An ordinary
    /// batch swaps nothing.
    pub(super) isolation: Option<ProofIsolation>,
}

/// The live owners an isolated template proof runs without: `Monomorph`'s `limit` and
/// `collection_payloads`, swapped whole out of the registry at admission so the
/// throwaway pass cannot reach them, then re-seated whole.
///
/// This inverse never *restores* those two. Both are diagnostic payload, which stays in
/// the predecessor substrate's sole custody; the staged-body guard owns this guard and
/// those still-private payloads together, and a batch that ends in an invariant drops
/// the aggregate whole. [`TypeRegistry::admit_generic_owners`] destructures `Monomorph`
/// exhaustively, so a new owner cannot be added without a decision recorded here.
pub(super) struct ProofIsolation {
    pub(super) prior_payloads: DiagnosticCollector,
}

/// The generic-owner composite guard: one admitted batch over the real registry (held
/// by exclusive `&mut`) and the real draft (held through an armed [`DraftTxn`]).
///
/// Holding the registry exclusively makes restoration borrow-panic-free: `Drop` has
/// `&mut self` and reaches every interior owner through `RefCell::get_mut`, with no
/// runtime borrow flag to conflict with. It also forces any metadata session or interior
/// borrow derived inside the batch to die before the guard drops, so a guard dropped
/// under a live session is a compile error rather than an unwind abort.
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
    /// The explicit spelling of the isolated template proof's exit, whose product is its
    /// diagnostics and editor facts rather than the throwaway image work it emits. The
    /// order matches `Drop`'s: no path may retain draft rows whose registry rows were
    /// erased.
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
    fn drop(&mut self) {
        if let Some(inverse) = self.inverse.take() {
            self.registry.restore_generic_owners(inverse);
        }
        drop(self.draft.take());
    }
}
