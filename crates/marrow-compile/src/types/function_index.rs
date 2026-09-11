//! Generic function instances retain the identity reserved by the image draft.

use marrow_image::{DraftTxn, FuncId};

use super::{
    FnInst, GArg, GenericCacheInvariant, GenericInvariant, InstantiationLimit, MAX_INSTANTIATIONS,
    MintSite, ResolveError, ResolveRefusal, TypeRegistry,
};

#[cfg(test)]
use super::bump_scaling;

impl TypeRegistry {
    /// Reserve the image function index for `(fn template, args)`, minting and
    /// enqueuing a fresh instance on first request and reusing it thereafter. A shared
    /// bound refusal records the first coherent mint site and returns `Err(Limit)`.
    pub(crate) fn reserve_fn_instance(
        &mut self,
        draft: &mut DraftTxn<'_>,
        template: usize,
        args: Vec<GArg>,
        site: MintSite<'_>,
    ) -> Result<FuncId, ResolveError> {
        self.validate_type_arguments(&args)?;
        let mut generics = self.generics.borrow_mut();
        // Reservation-dedup reuse probe: a keyed lookup into the append-only secondary
        // index. The reserved image function index is read from the named row (the
        // authority), and a row that does not carry the looked-up key is drift.
        #[cfg(test)]
        bump_scaling(|counts| counts.fn_inst_scan_steps += 1);
        if let Some(&row) = generics.fn_index.get(&(template, args.clone())) {
            let reused = generics
                .fn_insts
                .get(row)
                .filter(|inst| inst.template == template && inst.args == args);
            let Some(inst) = reused else {
                return Err(
                    GenericInvariant::CacheState(GenericCacheInvariant::MintIndexDrift).into(),
                );
            };
            return Ok(inst.func);
        }
        if generics.type_insts.len() + generics.fn_insts.len() >= MAX_INSTANTIATIONS {
            drop(generics);
            self.record_limit(site, InstantiationLimit::Count);
            return Err(ResolveRefusal::Limit.into());
        }
        let row = generics.fn_insts.len();
        let func = draft.reserve_function()?;
        let inst = FnInst {
            template,
            args,
            func,
        };
        // Keep the lookup-only reuse index in lockstep with its authority. A reserve
        // only appends on a dedup miss, so this key is new; a pre-existing entry means
        // the dedup probe and the index disagree. Reject it as a typed invariant on the
        // same terms as the type mint: the append below reserves an image function index
        // and queues a body for it, so a duplicate key would mint a second reservation
        // and a second lowering for one instantiation.
        let displaced = generics
            .fn_index
            .insert((inst.template, inst.args.clone()), row);
        if displaced.is_some() {
            return Err(
                GenericInvariant::CacheState(GenericCacheInvariant::MintKeyAlreadyPresent).into(),
            );
        }
        generics.fn_insts.push(inst.clone());
        generics.fn_queue.push_back(inst);
        Ok(func)
    }

    /// The next generic function instance awaiting body lowering: its template index,
    /// concrete arguments, and reserved image function index.
    ///
    /// This *reads* the front entry and leaves the queue alone. Removing it is
    /// [`Self::consume_fn_pending`], which the drain driver calls only once the batch that
    /// lowered the entry has settled. The split is what makes the queue invertible: an
    /// inverse that captures a length can undo the batch's appends, but it cannot put
    /// back a front entry the driver removed before the batch was even admitted, and
    /// reinstating one would mean an allocating call on the restore path.
    pub(crate) fn peek_fn_pending(&self) -> Option<(usize, Vec<GArg>, FuncId)> {
        self.generics
            .borrow()
            .fn_queue
            .front()
            .map(|inst| (inst.template, inst.args.clone(), inst.func))
    }

    /// Remove the entry [`Self::peek_fn_pending`] reported, after its batch settled.
    ///
    /// A batch only ever appends to the back, so the front entry after settlement is
    /// still the one that was lowered.
    pub(crate) fn consume_fn_pending(&mut self) {
        self.generics.get_mut().fn_queue.pop_front();
    }
}
