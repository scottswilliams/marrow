//! Shared per-function checking context: Ctx, FnSig, and the Effects accumulator.

use super::borrow_two;
use super::call_targets;
use super::durable_op_class;
use super::durable_site;
use super::flow_successors;
use super::is_mutation;
use super::reject;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    RetShape, SealedCollectionType, SealedEnumType, SealedFunction, SealedIndex, SealedInstr,
    SealedRecordType, SealedRoot, SealedSite,
};
use marrow_image::{DemandAtom, ExportDemand, ImageType, SemanticPath};
use std::collections::BTreeSet;

/// The sealed tables the per-function checks consult.
pub(super) struct Ctx<'a> {
    pub(super) types: &'a [SealedRecordType],
    pub(super) enums: &'a [SealedEnumType],
    pub(super) collections: &'a [SealedCollectionType],
    pub(super) roots: &'a [SealedRoot],
    pub(super) sites: &'a [SealedSite],
    pub(super) indexes: &'a [SealedIndex],
    pub(super) signatures: &'a [FnSig],
}

/// A callee's signature, consulted by the per-function `Call` type check.
pub(super) struct FnSig {
    pub(super) params: Vec<ImageType>,
    pub(super) ret: RetShape,
}

/// Phase 4/5 durable-demand closure and the transaction-flow lattice (design §E).
///
/// The single effects owner: it reconstructs each function's durable-access atom set
/// over its whole acyclic call closure from the sealed sites its opcodes reference,
/// and the boolean mutate/read coverage the transaction-flow lattice and the store
/// ceiling consume are projected from that atom set — there is no second effects
/// model.
pub(super) struct Effects {
    /// Per function: the durable-access atoms it or a transitive callee performs.
    pub(super) atoms_closure: Vec<BTreeSet<DemandAtom>>,
    /// Per function: the image-local site indices it or a transitive callee reaches.
    pub(super) sites_closure: Vec<BTreeSet<u16>>,
    /// Per function: whether its atom closure mutates (write/erase). Projected from
    /// the atom set; consumed by the transaction-flow lattice and the export effect
    /// class.
    pub(super) mutates_closure: Vec<bool>,
    /// Per function: whether it contains a `TxnBegin` (a transaction owner).
    pub(super) has_begin: Vec<bool>,
    /// Per function: whether it contains a `TxnCommit`.
    pub(super) has_commit: Vec<bool>,
}

impl Effects {
    /// Reconstruct demand over the acyclic call graph. `site_paths[s]` is the
    /// semantic path of the node site `s` addresses (parallel to the sealed sites).
    pub(super) fn compute(functions: &[SealedFunction], site_paths: &[SemanticPath]) -> Self {
        let count = functions.len();
        // Each function's own atoms and reached sites, before closure.
        let mut atoms_closure: Vec<BTreeSet<DemandAtom>> = functions
            .iter()
            .map(|function| {
                let mut set = BTreeSet::new();
                for instr in function.instrs() {
                    if let (Some(site), Some(class)) =
                        (durable_site(instr), durable_op_class(instr))
                    {
                        set.insert(DemandAtom::new(site_paths[site as usize].clone(), class));
                    }
                }
                set
            })
            .collect();
        let mut sites_closure: Vec<BTreeSet<u16>> = functions
            .iter()
            .map(|function| function.instrs().iter().filter_map(durable_site).collect())
            .collect();
        let has_begin: Vec<bool> = functions
            .iter()
            .map(|function| {
                function
                    .instrs()
                    .iter()
                    .any(|instr| matches!(instr, SealedInstr::TxnBegin))
            })
            .collect();
        let has_commit: Vec<bool> = functions
            .iter()
            .map(|function| {
                function
                    .instrs()
                    .iter()
                    .any(|instr| matches!(instr, SealedInstr::TxnCommit))
            })
            .collect();
        let callees: Vec<Vec<usize>> = functions.iter().map(call_targets).collect();

        // Fixpoint over the acyclic call graph: a caller's closure unions each
        // callee's closure. The graph is acyclic (recursion is rejected), so
        // iterating `count` times converges; the monotone growth stops earlier. The
        // caller index `f` also indexes the two closures a split borrow updates in
        // place, so a range loop is used deliberately.
        #[allow(clippy::needless_range_loop)]
        for _ in 0..count {
            let mut changed = false;
            for f in 0..count {
                for callee_index in 0..callees[f].len() {
                    let callee = callees[f][callee_index];
                    // Split the borrow: a call graph edge never self-loops (no
                    // recursion), so `f != callee`.
                    let (dst, src) = borrow_two(&mut atoms_closure, f, callee);
                    for atom in src.iter() {
                        if dst.insert(atom.clone()) {
                            changed = true;
                        }
                    }
                    let (dst_sites, src_sites) = borrow_two(&mut sites_closure, f, callee);
                    for &site in src_sites.iter() {
                        if dst_sites.insert(site) {
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mutates_closure: Vec<bool> = atoms_closure
            .iter()
            .map(|atoms| atoms.iter().any(|atom| atom.class().mutates()))
            .collect();

        Self {
            atoms_closure,
            sites_closure,
            mutates_closure,
            has_begin,
            has_commit,
        }
    }

    /// The verifier-reconstructed durable demand of the entry at `func`: its stable
    /// atom set over its whole call closure.
    pub(super) fn demand(&self, func: u16) -> ExportDemand {
        ExportDemand::from_atoms(self.atoms_closure[func as usize].iter().cloned())
    }

    /// The image-local operation sites the entry at `func` can reach, ascending.
    pub(super) fn reachable_sites(&self, func: u16) -> Vec<u16> {
        self.sites_closure[func as usize].iter().copied().collect()
    }

    /// Phase 5: validate one function's transaction flow. A transaction owner (a
    /// function that mutates in closure and contains `TxnBegin`) runs the
    /// {BeforeBegin, InTxn, AfterCommit} lattice; every other function must contain
    /// no transaction marker; and no function may call a transaction owner.
    pub(super) fn check_transaction_flow(
        &self,
        index: usize,
        function: &SealedFunction,
        is_export_entry: bool,
        is_test_entry: bool,
    ) -> Result<(), VerifyRejection> {
        // A function containing `TxnBegin` is a transaction owner and may never be
        // called — except from a test-entry driver, where each export call is its own
        // invocation boundary (a terminal-style driver). The test-entry phase
        // separately refuses a driver that also performs a direct durable op, which no
        // single session could run.
        if !is_test_entry {
            for &callee in &call_targets(function) {
                if self.has_begin[callee] {
                    return Err(reject(
                        VerifyPhase::Flow,
                        "a transaction owner may not be called",
                    ));
                }
            }
        }

        // A transaction owns durable work: a `transaction` block whose closure performs
        // no durable operation is a no-op region. It commits nothing, so the runtime
        // opens no session for it (its demand is empty) and its `TxnCommit` would have
        // no session to consume. Refuse it here rather than admit a region that cannot
        // run. A region that reads carries read demand and is admitted below.
        if is_export_entry && self.has_begin[index] && self.atoms_closure[index].is_empty() {
            return Err(reject(
                VerifyPhase::Flow,
                "a transaction performs no durable operation",
            ));
        }

        // An export entry that owns a transaction runs the lattice: it must begin and
        // commit on every path with every mutation inside. A read-only region (a
        // transaction whose closure only reads) is admitted here — the read demand
        // inside the owned region is coherent — while a mutating export with no begin is
        // still caught, since the lattice rejects a mutation before begin.
        if is_export_entry && (self.mutates_closure[index] || self.has_begin[index]) {
            return self.check_owner_lattice(function);
        }

        // Every other function is a read-only function or a mutating helper (wholly
        // inside its caller's transaction). Neither may carry a transaction marker.
        if self.has_begin[index] || self.has_commit[index] {
            return Err(reject(
                VerifyPhase::Flow,
                "a transaction marker sits outside its owning export",
            ));
        }
        Ok(())
    }

    /// The three-state transaction lattice over a transaction owner's CFG.
    fn check_owner_lattice(&self, function: &SealedFunction) -> Result<(), VerifyRejection> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            BeforeBegin,
            InTxn,
            AfterCommit,
        }
        let code = function.instrs();
        let mut entry: Vec<Option<State>> = vec![None; code.len()];
        entry[0] = Some(State::BeforeBegin);
        let mut worklist = vec![0usize];
        while let Some(index) = worklist.pop() {
            let state = entry[index].expect("worklist only enqueues reached instructions");
            let instr = &code[index];
            let next_state = match instr {
                SealedInstr::TxnBegin => {
                    if state != State::BeforeBegin {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "the transaction is begun more than once",
                        ));
                    }
                    State::InTxn
                }
                SealedInstr::TxnCommit => {
                    if state != State::InTxn {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a transaction is committed outside its region",
                        ));
                    }
                    State::AfterCommit
                }
                SealedInstr::Return => {
                    if state != State::AfterCommit {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a path returns without committing the transaction",
                        ));
                    }
                    continue; // no successors
                }
                _ => {
                    let mutating_here = is_mutation(instr)
                        || matches!(instr, SealedInstr::Call(target) if self.mutates_closure[*target as usize]);
                    if mutating_here && state != State::InTxn {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a mutation sits outside the transaction region",
                        ));
                    }
                    // The commit consumes the session's engine transaction, so no
                    // durable operation — read or write, direct or through a callee's
                    // closure — may follow it. A mutating export observes the store
                    // inside its region and returns values it captured there; a read
                    // after commit is refused here so the runtime never reaches a
                    // consumed transaction.
                    let durable_here = durable_op_class(instr).is_some()
                        || matches!(instr, SealedInstr::Call(target) if !self.atoms_closure[*target as usize].is_empty());
                    if durable_here && state == State::AfterCommit {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a durable operation follows the transaction's commit",
                        ));
                    }
                    state
                }
            };
            for successor in flow_successors(code, index) {
                match entry[successor] {
                    None => {
                        entry[successor] = Some(next_state);
                        worklist.push(successor);
                    }
                    Some(existing) if existing == next_state => {}
                    Some(_) => {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "transaction state disagrees at a merge",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
