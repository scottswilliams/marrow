//! The image function index: the one carrier every function ordinal narrows onto,
//! and the generic-instance reservations that consume it.
//!
//! Five operations compute a function index — the declared signature's slot ordinal,
//! the monomorphic slot count, the test-body count, the generic base those two sum
//! to, and the base plus an instance row. Each accumulates in a carrier wider than
//! the `u16` the image spells and narrows through [`narrow_function_index`] exactly
//! where its value is consumed, so no producer ever holds a value it cannot
//! represent and the whole family has one refusal.

use super::{
    FnInst, GArg, GenericCacheInvariant, GenericInvariant, MAX_INSTANTIATIONS, MintSite,
    ResolveError, ResolveRefusal, TypeRegistry,
};

#[cfg(test)]
use super::bump_scaling;

/// Narrow a computed function ordinal onto the carrier the image spells.
///
/// A value the carrier cannot hold is a fact about the compiler's own counting
/// rather than about any one construct the source wrote, so it aborts at the
/// invariant boundary with no span. This replaces the debug overflow and the release
/// wrap the unwidened arithmetic reached at the first unrepresentable value; it adds
/// no source diagnostic, moves no bound, and changes no function order, slot,
/// signature, export, or lookup projection.
pub(crate) fn narrow_function_index(wide: u64) -> Result<u16, GenericInvariant> {
    u16::try_from(wide).map_err(|_| GenericInvariant::FunctionIndexDomain)
}

impl TypeRegistry {
    /// Set the base image function index for generic function instantiations, once
    /// every monomorphic function and test has consumed its index. The caller sums
    /// the two counts wide; this is the boundary that narrows the sum.
    pub(crate) fn set_fn_base(&mut self, base: u64) -> Result<(), GenericInvariant> {
        self.generics.get_mut().fn_base = narrow_function_index(base)?;
        Ok(())
    }

    /// Reserve the image function index for `(fn template, args)`, minting and
    /// enqueuing a fresh instance on first request and reusing it thereafter. A shared
    /// bound refusal records the first coherent mint site and returns `Err(Limit)`.
    pub(crate) fn reserve_fn_instance(
        &mut self,
        template: usize,
        args: Vec<GArg>,
        site: MintSite<'_>,
    ) -> Result<u16, ResolveError> {
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
            self.record_limit(
                site,
                "a generic function likely recurses over an ever-growing type",
            );
            return Err(ResolveRefusal::Limit.into());
        }
        let row = generics.fn_insts.len();
        // Summed wide and narrowed here, where the instance takes it.
        let func = narrow_function_index(u64::from(generics.fn_base) + row as u64)?;
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
    pub(crate) fn peek_fn_pending(&self) -> Option<(usize, Vec<GArg>, u16)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The generic base is the last index a declared body took plus one; a project
    /// whose monomorphic functions and tests together fill the carrier leaves no
    /// index for an instance, and the narrowing says so instead of wrapping.
    #[test]
    fn the_generic_base_admits_the_last_representable_index_and_refuses_one_past_it() {
        assert_eq!(narrow_function_index(u64::from(u16::MAX)), Ok(u16::MAX));
        assert_eq!(
            narrow_function_index(u64::from(u16::MAX) + 1),
            Err(GenericInvariant::FunctionIndexDomain)
        );
        assert_eq!(
            narrow_function_index(u64::from(u16::MAX) + 2),
            Err(GenericInvariant::FunctionIndexDomain),
            "the refusal does not alias slot one"
        );
    }
}
