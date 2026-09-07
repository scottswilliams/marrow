//! Phase 3+ call-graph and presence-flow checking over sealed functions.

use super::context::{Ctx, Effects};
use super::decode_code::decode_code;
use super::decode_code::resolve_jumps;
use super::flow::{Frame, branch_key_columns, check_flow};
use super::model::{DecodedFunction, DecodedImage};
use super::reject;
use super::spans::map_spans;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{SealedFunction, SealedInstr, SealedSite, SealedSiteTarget};
use crate::vtype::VType;
use marrow_image::SemanticPath;
use std::collections::BTreeSet;

/// Phase 4: reject any cycle in the direct-call graph (recursion is not admitted).
/// A three-colour DFS over the recorded calls; a back edge to a node on the current
/// stack is a cycle.
pub(super) fn reject_call_cycles(functions: &[SealedFunction]) -> Result<(), VerifyRejection> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Colour {
        White,
        Gray,
        Black,
    }
    let mut colour = vec![Colour::White; functions.len()];
    // Iterative DFS: a frame is (node, next-child-cursor).
    for start in 0..functions.len() {
        if colour[start] != Colour::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        colour[start] = Colour::Gray;
        while let Some(&(node, cursor)) = stack.last() {
            let callees: Vec<usize> = call_targets(&functions[node]);
            if cursor < callees.len() {
                stack.last_mut().expect("frame present").1 += 1;
                let next = callees[cursor];
                match colour[next] {
                    Colour::Gray => {
                        return Err(reject(
                            VerifyPhase::Closure,
                            "the call graph contains a cycle",
                        ));
                    }
                    Colour::White => {
                        colour[next] = Colour::Gray;
                        stack.push((next, 0));
                    }
                    Colour::Black => {}
                }
            } else {
                colour[node] = Colour::Black;
                stack.pop();
            }
        }
    }
    Ok(())
}

/// The direct-call targets of a sealed function, in tape order.
pub(super) fn call_targets(function: &SealedFunction) -> Vec<usize> {
    function
        .instrs()
        .iter()
        .filter_map(|instr| match instr {
            SealedInstr::Call(target) => Some(*target as usize),
            _ => None,
        })
        .collect()
}

/// The control-flow successors of the sealed instruction at `index`.
pub(super) fn flow_successors(code: &[SealedInstr], index: usize) -> Vec<usize> {
    match &code[index] {
        SealedInstr::Return | SealedInstr::Unreachable(_) | SealedInstr::Todo(_) => Vec::new(),
        SealedInstr::Jump(target) => vec![*target],
        SealedInstr::JumpIfFalse(target)
        | SealedInstr::BranchPresent(target)
        | SealedInstr::IntAddChecked(target)
        | SealedInstr::IntSubChecked(target)
        | SealedInstr::IntMulChecked(target)
        | SealedInstr::IntNegChecked(target)
        | SealedInstr::IntDivChecked(target)
        | SealedInstr::IntRemChecked(target) => {
            vec![*target, index + 1]
        }
        _ => vec![index + 1],
    }
}

/// Phase 5 (presence): the place-slot presence lattice (design §D). A present-form
/// instruction — the field set and the group read that key off place slots — asserts
/// its containing entry is present; this recheck proves that independently of the
/// compiler, so a forged or mis-lowered present-form op whose graph cannot imply its
/// payload is refused.
///
/// The lattice state at each program point is the set of proven-present entries, each
/// its family and its key-path slots. A fact is *established* by a guard that tests the
/// entry keyed by the slots — `LocalGet(S…); DurExists(entry); JumpIfFalse` on its
/// present (fallthrough) edge, or `LocalGet(S…); DurReadEntry; BranchPresent` on its
/// present edge — or by a whole-entry `DurCreateEntry` keyed by those slots (create
/// leaves the entry present whether it was created or already present). It is *killed*
/// by any entry erase of the fact's family, whatever key the erase names; by a call
/// whose demand closure writes the family; and by any `LocalSet` of a slot the fact
/// reads (a `place` key slot is bind-once, so a rebind never fires on compiler output —
/// it hardens the recheck against a mutated tape). Facts join by intersection at
/// merges: an entry is present only if it holds on every incoming edge.
pub(super) fn check_presence_flow(
    function: &SealedFunction,
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    effects: &Effects,
    site_paths: &[SemanticPath],
) -> Result<(), VerifyRejection> {
    let code = function.instrs();
    if !code.iter().any(|instr| {
        matches!(
            instr,
            SealedInstr::DurSetField { .. } | SealedInstr::DurReadGroupPresent { .. }
        )
    }) {
        return Ok(());
    }
    let mut entry: Vec<Option<BTreeSet<PresenceFact>>> = vec![None; code.len()];
    entry[0] = Some(BTreeSet::new());
    let mut worklist = vec![0usize];
    while let Some(index) = worklist.pop() {
        let present = entry[index]
            .clone()
            .expect("worklist only enqueues reached instructions");
        if let SealedInstr::DurSetField { site, key_slots }
        | SealedInstr::DurReadGroupPresent { site, key_slots } = &code[index]
        {
            // The present form is proven only if a dominating fact names the exact
            // containing entry — its family and its whole key-path — not merely a
            // matching slot tuple (sibling branches of equal arity share slot tuples).
            let (root, branch) = payload_site_family(ctx, *site).ok_or(reject(
                VerifyPhase::Flow,
                "a present-entry operation does not resolve to a field or group site",
            ))?;
            if !present.contains(&(root, branch, key_slots.clone())) {
                return Err(reject(
                    VerifyPhase::Flow,
                    "a present-entry operation is not dominated by a presence fact on its containing entry",
                ));
            }
        }
        for (successor, set) in presence_edges(
            code,
            ctx,
            non_fallthrough_entries,
            effects,
            site_paths,
            index,
            &present,
        ) {
            if successor >= code.len() {
                return Err(reject(VerifyPhase::Flow, "presence edge out of range"));
            }
            match &mut entry[successor] {
                None => {
                    entry[successor] = Some(set);
                    worklist.push(successor);
                }
                Some(existing) => {
                    let merged: BTreeSet<PresenceFact> =
                        existing.intersection(&set).cloned().collect();
                    if merged.len() != existing.len() {
                        *existing = merged;
                        worklist.push(successor);
                    }
                }
            }
        }
    }
    Ok(())
}

/// A proven-present containing entry in the presence-flow lattice: the root index it
/// lives under, the entry's branch path (empty for the root itself), and its whole
/// key-path as pre-evaluated local slots (root-first). Keying on the root — not the
/// slot tuple or branch path alone — keeps entries under distinct roots distinct even
/// when they share a key slot; keying on the branch path distinguishes sibling
/// branches of equal key arity that share slot values under one root. The first two
/// components are the entry's *family*, the unit an erase or a family-writing call
/// ends proofs over.
type PresenceFact = (u16, Vec<u16>, Vec<u16>);

/// The presence-set carried on each successor edge of the instruction at `index`.
/// Most instructions pass the set through unchanged; guards split the set (adding the
/// proven entry only on the present edge); create adds; an erase, a family-writing
/// call, and a slot rebind remove.
fn presence_edges(
    code: &[SealedInstr],
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    effects: &Effects,
    site_paths: &[SemanticPath],
    index: usize,
    present: &BTreeSet<PresenceFact>,
) -> Vec<(usize, BTreeSet<PresenceFact>)> {
    match &code[index] {
        SealedInstr::JumpIfFalse(target) => {
            match exists_guard_fact(code, ctx, non_fallthrough_entries, index) {
                // The present (true) edge falls through into the guarded block; the false
                // edge (target) is the absent branch.
                Some(fact) => {
                    let mut present_edge = present.clone();
                    present_edge.insert(fact);
                    vec![(*target, present.clone()), (index + 1, present_edge)]
                }
                None => flow_successors(code, index)
                    .into_iter()
                    .map(|s| (s, present.clone()))
                    .collect(),
            }
        }
        SealedInstr::BranchPresent(target) => {
            match read_entry_guard_fact(code, ctx, non_fallthrough_entries, index) {
                Some(fact) => {
                    let mut present_edge = present.clone();
                    present_edge.insert(fact);
                    vec![(*target, present.clone()), (index + 1, present_edge)]
                }
                None => flow_successors(code, index)
                    .into_iter()
                    .map(|s| (s, present.clone()))
                    .collect(),
            }
        }
        SealedInstr::DurCreateEntry(site) => {
            let mut next = present.clone();
            if let Some((root, branch, _)) = entry_site(ctx, *site)
                && branch.is_empty()
                && let Some(slot) = entry_write_key_slot(code, non_fallthrough_entries, index)
            {
                next.insert((root, Vec::new(), vec![slot]));
            }
            vec![(index + 1, next)]
        }
        SealedInstr::DurEraseEntry(site) => {
            let mut next = present.clone();
            // An entry erase ends every fact of the erased family whatever key operand
            // it names — a slot, another slot, or a constant — because the lattice does
            // not reason about key equality. Facts of other families survive: an erase
            // touches only the entry's own payload, so a child family's entry outlives
            // its parent's erase.
            if let Some((root, branch, _)) = entry_site(ctx, *site) {
                next.retain(|(fact_root, fact_branch, _)| {
                    (*fact_root, fact_branch) != (root, &branch)
                });
            }
            vec![(index + 1, next)]
        }
        SealedInstr::Call(callee) => {
            let mut next = present.clone();
            // A call whose demand closure writes a family — creates, replaces, or
            // erases an entry of it — ends every fact of that family: the callee may
            // have erased the proven entry. A call that only reads, or only updates
            // fields of present entries, leaves every fact in place.
            for atom in &effects.atoms_closure[*callee as usize] {
                if !atom.class().mutates() {
                    continue;
                }
                if let Some((root, branch)) = family_of_path(ctx, site_paths, atom.path()) {
                    next.retain(|(fact_root, fact_branch, _)| {
                        (*fact_root, fact_branch) != (root, &branch)
                    });
                }
            }
            vec![(index + 1, next)]
        }
        SealedInstr::LocalSet(slot) => {
            let mut next = present.clone();
            // A rebind of any key-path slot invalidates every fact that reads it.
            next.retain(|(_, _, keys)| !keys.contains(slot));
            vec![(index + 1, next)]
        }
        _ => flow_successors(code, index)
            .into_iter()
            .map(|s| (s, present.clone()))
            .collect(),
    }
}

/// The family `(root, branch path)` of the entry site whose semantic path is `path`, or
/// `None` when no entry site carries that path (a field, group, or index atom names no
/// family of its own).
fn family_of_path(
    ctx: &Ctx,
    site_paths: &[SemanticPath],
    path: &SemanticPath,
) -> Option<(u16, Vec<u16>)> {
    site_paths
        .iter()
        .zip(0u16..)
        .find(|(site_path, _)| *site_path == path)
        .and_then(|(_, site)| entry_site(ctx, site))
        .map(|(root, branch, _)| (root, branch))
}

/// The containing entry a flat entry (whole-payload or branch-entry) `site` names: the
/// root index it lives under, its branch path (empty for the root), and its whole
/// key-path column arity. `None` for a non-entry site (a field leaf or index), which
/// names no entry to prove present.
fn entry_site(ctx: &Ctx, site: u16) -> Option<(u16, Vec<u16>, usize)> {
    let SealedSite::Flat {
        root: root_index,
        target,
    } = ctx.sites.get(site as usize)?
    else {
        return None;
    };
    let root = ctx.roots.get(*root_index as usize)?;
    match target {
        SealedSiteTarget::WholePayload => Some((*root_index, Vec::new(), root.keys.len())),
        SealedSiteTarget::BranchEntry(path) => {
            let extra = branch_key_columns(root, path).ok()?;
            Some((*root_index, path.to_vec(), root.keys.len() + extra.len()))
        }
        SealedSiteTarget::FieldLeaf(_)
        | SealedSiteTarget::BranchField { .. }
        | SealedSiteTarget::GroupEntry(_)
        | SealedSiteTarget::IndexScan(_)
        | SealedSiteTarget::IndexLookup(_) => None,
    }
}

/// The family of the entry a flat payload `site` belongs to: the root index and the
/// branch path of a field leaf (empty for a root field, the branch placement path for a
/// branch field), or the root and an empty path for a root-level group. `None` for a
/// site that is not a field or group.
fn payload_site_family(ctx: &Ctx, site: u16) -> Option<(u16, Vec<u16>)> {
    let SealedSite::Flat { root, target } = ctx.sites.get(site as usize)? else {
        return None;
    };
    match target {
        SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::GroupEntry(_) => {
            Some((*root, Vec::new()))
        }
        SealedSiteTarget::BranchField { branch, .. } => Some((*root, branch.to_vec())),
        _ => None,
    }
}

/// The `arity` key-path slots pushed immediately before position `at` (root-first): each
/// must be a `LocalGet`, or the guard establishes no fact. `at` is the position of the
/// consuming `DurExists`/`DurReadEntry`.
fn read_key_path_before(code: &[SealedInstr], at: usize, arity: usize) -> Option<Vec<u16>> {
    if arity == 0 || at < arity {
        return None;
    }
    let mut keys = Vec::with_capacity(arity);
    for offset in 0..arity {
        let SealedInstr::LocalGet(slot) = &code[at - arity + offset] else {
            return None;
        };
        keys.push(*slot);
    }
    Some(keys)
}

/// Entry at the first load is valid; entry into each later instruction must come
/// from its immediate predecessor. Type flow supplies the mask for this tape.
fn window_has_only_fallthrough(entries: &[bool], first: usize, last: usize) -> bool {
    !entries[first + 1..=last].iter().any(|entry| *entry)
}

/// The presence fact an `exists`-guard proves at a `JumpIfFalse`: `LocalGet(S0); …;
/// LocalGet(Sn); DurExists(entry site); JumpIfFalse`. The fact is the entry site's
/// branch path paired with the whole key-path it reads. `None` when the shape does not
/// match (a non-entry site, a non-local key, or an unrelated condition).
fn exists_guard_fact(
    code: &[SealedInstr],
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    index: usize,
) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurExists(site) = &code[index - 1] else {
        return None;
    };
    let (root, branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    if !window_has_only_fallthrough(non_fallthrough_entries, index - arity - 1, index) {
        return None;
    }
    Some((root, branch, keys))
}

/// The presence fact an `if const x = p` guard proves at a `BranchPresent`:
/// `LocalGet(S0); …; LocalGet(Sn); DurReadEntry(entry site); BranchPresent`.
fn read_entry_guard_fact(
    code: &[SealedInstr],
    ctx: &Ctx,
    non_fallthrough_entries: &[bool],
    index: usize,
) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurReadEntry(site) = &code[index - 1] else {
        return None;
    };
    let (root, branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    if !window_has_only_fallthrough(non_fallthrough_entries, index - arity - 1, index) {
        return None;
    }
    Some((root, branch, keys))
}

/// The key slot below a locally loaded create record. Only a root create uses
/// this one-slot fact; a composite root cannot consume it as a whole-key proof.
/// Every instruction after the key load must execute by fallthrough.
fn entry_write_key_slot(
    code: &[SealedInstr],
    non_fallthrough_entries: &[bool],
    index: usize,
) -> Option<u16> {
    if index < 2 {
        return None;
    }
    let SealedInstr::LocalGet(_) = &code[index - 1] else {
        return None;
    };
    let SealedInstr::LocalGet(slot) = &code[index - 2] else {
        return None;
    };
    if !window_has_only_fallthrough(non_fallthrough_entries, index - 2, index) {
        return None;
    }
    Some(*slot)
}

/// The successor edges for a two-way branch that keeps the current stack on the
/// `target` edge and pushes one value on the fallthrough edge (`index + 1`). Shared
/// by `BranchPresent` (present value) and the checked ops (int result).
pub(super) fn push_on_fallthrough(
    frame: &Frame,
    target: usize,
    index: usize,
    pushed: VType,
    max_stack: &mut usize,
) -> Result<Vec<(usize, Frame)>, VerifyRejection> {
    let mut fallthrough = frame.clone();
    fallthrough.stack.push(pushed);
    if fallthrough.stack.len() > marrow_image::bounds::MAX_STACK_DEPTH {
        return Err(reject(
            VerifyPhase::Function,
            "operand stack exceeds depth bound",
        ));
    }
    *max_stack = (*max_stack).max(fallthrough.stack.len());
    Ok(vec![(target, frame.clone()), (index + 1, fallthrough)])
}

pub(super) fn verify_function(
    function: &DecodedFunction,
    ctx: &Ctx,
    decoded: &DecodedImage,
) -> Result<(SealedFunction, Vec<bool>), VerifyRejection> {
    let mut decoded_code = decode_code(&function.code)?;
    resolve_jumps(&mut decoded_code)?;
    let (instrs, max_stack, non_fallthrough_entries) =
        check_flow(function, ctx, &decoded_code, &decoded.consts)?;
    let spans = map_spans(function, &decoded_code)?;
    Ok((
        SealedFunction {
            name: decoded.strings[function.name as usize].clone(),
            source: decoded.strings[function.source as usize].clone(),
            params: function.params.clone(),
            ret: function.ret,
            local_count: function.local_count,
            instrs,
            spans,
            max_stack,
            mutating: false,
        },
        non_fallthrough_entries,
    ))
}

#[cfg(test)]
mod presence_root_discrimination {
    //! The presence lattice keys a proven-present entry on its root, not on its
    //! key slot alone. Two whole-entry creates that share a key slot but address
    //! different roots must establish two distinct facts, so a strict sparse set
    //! over one root can never be proven by a create on another. This holds the
    //! (root, slot) discrimination structurally at the helper level, where it is
    //! observable even while the container bound admits a single root.
    use std::collections::BTreeSet;
    use std::rc::Rc;

    use marrow_image::Scalar;

    use super::super::context::{Ctx, Effects};
    use super::presence_edges;
    use crate::sealed::{SealedInstr, SealedRoot, SealedSite, SealedSiteTarget};

    fn keyed_root(name: &str) -> SealedRoot {
        SealedRoot {
            name: Rc::from(name),
            keys: vec![Scalar::Int],
            record: 0,
            has_extras: false,
            branches: Vec::new(),
            groups: Vec::new(),
        }
    }

    #[test]
    fn two_root_creates_sharing_a_slot_establish_distinct_facts() {
        let roots = [keyed_root("assets"), keyed_root("tallies")];
        let sites = [
            SealedSite::Flat {
                root: 0,
                target: SealedSiteTarget::WholePayload,
            },
            SealedSite::Flat {
                root: 1,
                target: SealedSiteTarget::WholePayload,
            },
        ];
        let ctx = Ctx {
            types: &[],
            enums: &[],
            collections: &[],
            roots: &roots,
            sites: &sites,
            indexes: &[],
            signatures: &[],
        };
        // `LocalGet(key); LocalGet(record); DurCreateEntry(site)` twice, the two
        // creates addressing different roots through the SAME key slot (7).
        let code = [
            SealedInstr::LocalGet(7),
            SealedInstr::LocalGet(3),
            SealedInstr::DurCreateEntry(0),
            SealedInstr::LocalGet(7),
            SealedInstr::LocalGet(3),
            SealedInstr::DurCreateEntry(1),
        ];
        let entries = [false; 6];
        let effects = Effects::compute(&[], &[]);
        let after_first = presence_edges(&code, &ctx, &entries, &effects, &[], 2, &BTreeSet::new())
            .into_iter()
            .find(|(successor, _)| *successor == 3)
            .expect("a create falls through to the next instruction")
            .1;
        let after_second = presence_edges(&code, &ctx, &entries, &effects, &[], 5, &after_first)
            .into_iter()
            .find(|(successor, _)| *successor == 6)
            .expect("a create falls through to the next instruction")
            .1;
        assert_eq!(
            after_second.len(),
            2,
            "creates on distinct roots must not alias to one presence fact",
        );
    }
}
